//! Bounded history persistence, independent of the live hardware sampler.
use crate::{db, models::{HardwareSnapshot, PersistenceStatus}};
use chrono::Utc;
use rusqlite::Connection;
use std::{sync::{atomic::{AtomicU32, AtomicU64, Ordering}, mpsc::{self, SyncSender, TrySendError, RecvTimeoutError}, Arc, Mutex}, thread, time::{Duration, Instant}};

const QUEUE_CAPACITY: usize = 12;
type StatusCallback = Arc<dyn Fn(PersistenceStatus) + Send + Sync>;

struct Health {
    value: PersistenceStatus,
    write_failed: bool,
    maintenance_failed: bool,
    queue_full: bool,
    last_notice: Option<Instant>,
}
enum Update { Saved, WriteFailed, MaintenanceFailed, MaintenancePassed, QueueFull, QueueAvailable }
struct Shared { health: Mutex<Health>, retention: AtomicU32, generation: AtomicU64, callback: StatusCallback }
struct QueuedSample { snapshot: HardwareSnapshot, generation: u64 }

impl Shared {
    fn update(&self, update: Update) {
        let event = {
            let mut health = self.health.lock().unwrap_or_else(|error| error.into_inner());
            let before = (health.value.status.clone(), health.value.message.clone());
            let saved = matches!(update, Update::Saved);
            match update {
                Update::Saved => { health.write_failed = false; health.value.last_saved_at = Some(Utc::now().to_rfc3339()); }
                Update::WriteFailed => { health.write_failed = true; health.value.dropped_samples = health.value.dropped_samples.saturating_add(1); }
                Update::MaintenanceFailed => { health.maintenance_failed = true; }
                Update::MaintenancePassed => { health.maintenance_failed = false; }
                Update::QueueFull => { health.queue_full = true; health.value.dropped_samples = health.value.dropped_samples.saturating_add(1); }
                Update::QueueAvailable => { health.queue_full = false; }
            }
            health.value.message = if health.write_failed { Some("Riwayat gagal disimpan. Monitoring tetap berjalan.".into()) }
                else if health.maintenance_failed { Some("Pemeliharaan riwayat gagal. Data yang ada tetap dipertahankan.".into()) }
                else if health.queue_full { Some("Penyimpanan sibuk; sebagian sampel riwayat dilewati.".into()) }
                else { None };
            health.value.status = if health.value.message.is_some() { "degraded" } else if health.value.last_saved_at.is_some() { "healthy" } else { "waiting" }.into();
            health.value.updated_at = Utc::now().to_rfc3339();
            let changed = before != (health.value.status.clone(), health.value.message.clone());
            let repeat_error = health.value.status == "degraded" && health.last_notice.map(|last| last.elapsed() >= Duration::from_secs(60)).unwrap_or(true);
            if changed || saved || repeat_error {
                if health.value.status == "degraded" { health.last_notice = Some(Instant::now()); }
                Some(health.value.clone())
            } else { None }
        };
        // Never call into Tauri/listeners while holding the status mutex.
        if let Some(event) = event { (self.callback)(event); }
    }
}

pub struct HistoryWriter { sender: SyncSender<QueuedSample>, shared: Arc<Shared> }
impl HistoryWriter {
    pub fn start(database: Arc<Mutex<Connection>>, retention_hours: u32, callback: impl Fn(PersistenceStatus) + Send + Sync + 'static) -> Self {
        Self::start_with_interval(database, retention_hours, Arc::new(callback), Duration::from_secs(60))
    }
    fn start_with_interval(database: Arc<Mutex<Connection>>, retention_hours: u32, callback: StatusCallback, maintenance_interval: Duration) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<QueuedSample>(QUEUE_CAPACITY);
        let shared = Arc::new(Shared { health: Mutex::new(Health { value: PersistenceStatus { status: "waiting".into(), message: None, last_saved_at: None, dropped_samples: 0, updated_at: Utc::now().to_rfc3339() }, write_failed: false, maintenance_failed: false, queue_full: false, last_notice: None }), retention: AtomicU32::new(retention_hours), generation: AtomicU64::new(0), callback });
        let worker = shared.clone();
        thread::spawn(move || {
            let mut last_maintenance = Instant::now();
            loop {
                match receiver.recv_timeout(Duration::from_secs(1).min(maintenance_interval)) {
                    Ok(sample) => {
                        worker.update(Update::QueueAvailable);
                        let result = database.lock().map_err(|_| "Database unavailable".to_string()).and_then(|mut conn| {
                            // Purge invalidates pending samples while holding this
                            // same DB mutex, so old queued history cannot reappear.
                            if sample.generation != worker.generation.load(Ordering::SeqCst) { return Ok(None); }
                            db::save_samples(&mut conn, &sample.snapshot).map(|()| Some(()))
                        });
                        match result { Ok(Some(())) => worker.update(Update::Saved), Err(_) => worker.update(Update::WriteFailed), Ok(None) => {} }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                // Retention continues when monitoring is paused or minimized.
                if last_maintenance.elapsed() >= maintenance_interval {
                    last_maintenance = Instant::now();
                    let hours = worker.retention.load(Ordering::Relaxed);
                    let result = database.lock().map_err(|_| "Database unavailable".to_string()).and_then(|conn| db::maintain(&conn, hours));
                    worker.update(if result.is_ok() { Update::MaintenancePassed } else { Update::MaintenanceFailed });
                }
            }
        });
        Self { sender, shared }
    }
    pub fn enqueue(&self, snapshot: HardwareSnapshot) {
        let sample = QueuedSample { snapshot, generation: self.shared.generation.load(Ordering::SeqCst) };
        match self.sender.try_send(sample) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => self.shared.update(Update::QueueFull),
            Err(TrySendError::Disconnected(_)) => self.shared.update(Update::WriteFailed),
        }
    }
    pub fn status(&self) -> PersistenceStatus { self.shared.health.lock().unwrap_or_else(|error| error.into_inner()).value.clone() }
    pub fn set_retention(&self, hours: u32) { self.shared.retention.store(hours, Ordering::Relaxed); }
    pub fn invalidate_pending(&self) { self.shared.generation.fetch_add(1, Ordering::SeqCst); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DeviceInfo, Metric};
    use std::path::Path;

    fn fixture() -> (Arc<Mutex<Connection>>, HardwareSnapshot) {
        let conn = db::open(Path::new(":memory:")).unwrap();
        let info = DeviceInfo { device_name: "test".into(), operating_system: "test".into(), cpu_model: "test".into(), cpu_cores: 1, ram_total_bytes: 0, gpu_adapters: vec![], power_source: "Unknown".into() };
        db::save_device(&conn, &info, &[]).unwrap();
        let mut sample = crate::empty_snapshot(&info);
        sample.cpu_usage = Metric::measured(0.0, "%", "test", &sample.recorded_at);
        (Arc::new(Mutex::new(conn)), sample)
    }
    fn wait_for(receiver: &mpsc::Receiver<PersistenceStatus>, status: &str) -> PersistenceStatus {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let event = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap();
            if event.status == status { return event; }
        }
    }
    #[test]
    fn failed_writes_are_visible_and_recover_without_losing_measurement_zero() {
        let (database, sample) = fixture();
        database.lock().unwrap().execute_batch("CREATE TRIGGER reject_samples BEFORE INSERT ON hardware_samples BEGIN SELECT RAISE(ABORT,'simulated full disk'); END;").unwrap();
        let (sender, receiver) = mpsc::channel();
        let writer = HistoryWriter::start(database.clone(), 6, move |event| { sender.send(event).unwrap(); });
        writer.enqueue(sample.clone());
        let failed = wait_for(&receiver, "degraded");
        assert_eq!(failed.dropped_samples, 1);
        assert!(failed.last_saved_at.is_none());
        database.lock().unwrap().execute_batch("DROP TRIGGER reject_samples;").unwrap();
        writer.enqueue(sample);
        let recovered = wait_for(&receiver, "healthy");
        assert!(recovered.last_saved_at.is_some());
        assert_eq!(recovered.dropped_samples, 1);
        assert!(recovered.message.is_none());
        assert_eq!(database.lock().unwrap().query_row("SELECT value FROM hardware_samples", [], |row| row.get::<_, f64>(0)).unwrap(), 0.0);
    }
    #[test]
    fn busy_storage_cannot_block_sampler_and_queue_remains_bounded() {
        let (database, sample) = fixture();
        let guard = database.lock().unwrap();
        let writer = HistoryWriter::start(database.clone(), 6, |_| {});
        let started = Instant::now();
        for _ in 0..64 { writer.enqueue(sample.clone()); }
        assert!(started.elapsed() < Duration::from_secs(1));
        let status = writer.status();
        assert_eq!(status.status, "degraded");
        assert!(status.dropped_samples >= 64 - QUEUE_CAPACITY - 1);
        drop(guard);
    }
    #[test]
    fn retention_runs_without_monitoring_and_reports_failure_then_recovery() {
        let (database, sample) = fixture();
        let old = (Utc::now() - chrono::Duration::hours(12)).to_rfc3339();
        {
            let mut conn = database.lock().unwrap();
            db::save_samples(&mut conn, &sample).unwrap();
            conn.execute("UPDATE hardware_samples SET recorded_at=?1", [old]).unwrap();
            conn.execute_batch("CREATE TRIGGER reject_retention BEFORE DELETE ON hardware_samples BEGIN SELECT RAISE(ABORT,'simulated retention failure'); END;").unwrap();
        }
        let (sender, receiver) = mpsc::channel();
        let writer = HistoryWriter::start_with_interval(database.clone(), 6, Arc::new(move |event| { let _ = sender.send(event); }), Duration::from_millis(25));
        let failed = wait_for(&receiver, "degraded");
        assert_eq!(failed.dropped_samples, 0);
        assert_eq!(database.lock().unwrap().query_row("SELECT COUNT(*) FROM hardware_samples", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
        database.lock().unwrap().execute_batch("DROP TRIGGER reject_retention;").unwrap();
        wait_for(&receiver, "waiting");
        assert_eq!(database.lock().unwrap().query_row("SELECT COUNT(*) FROM hardware_samples", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(database.lock().unwrap().query_row("SELECT SUM(sample_count) FROM hardware_sample_aggregates", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
        drop(writer);
    }
    #[test]
    fn purged_pending_samples_do_not_reappear_when_storage_unblocks() {
        let (database, old) = fixture();
        let guard = database.lock().unwrap();
        let (sender, receiver) = mpsc::channel();
        let writer = HistoryWriter::start(database.clone(), 6, move |event| { let _ = sender.send(event); });
        writer.enqueue(old.clone());
        writer.invalidate_pending();
        let mut fresh = old;
        fresh.cpu_usage.value = Some(9.0);
        writer.enqueue(fresh);
        drop(guard);
        wait_for(&receiver, "healthy");
        assert_eq!(database.lock().unwrap().query_row("SELECT COUNT(*),MIN(value) FROM hardware_samples", [], |row| Ok((row.get::<_, i64>(0)?,row.get::<_, f64>(1)?))).unwrap(), (1,9.0));
    }
}
