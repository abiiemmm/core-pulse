use crate::models::{AnalyticsPoint, AnalyticsReport, AppSettings, Capability, CleaningResult, DeviceInfo, HardwareSnapshot, HistoryPoint, PerformanceProfile, TuningSession};
use chrono::{Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use uuid::Uuid;

fn err(error: impl std::fmt::Display) -> String { error.to_string() }

pub fn open(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(err)?;
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;").map_err(err)?;
    migrate(&conn)?;
    seed_profiles(&conn)?;
    Ok(conn)
}

fn seed_profiles(conn: &Connection) -> Result<(), String> {
    let at = Utc::now().to_rfc3339();
    for (id, name) in [("balanced", "Balanced"), ("gaming", "Gaming Boost"), ("saving", "Power Saving")] {
        conn.execute("INSERT OR IGNORE INTO performance_profiles(id,name,profile_type,created_at,updated_at) VALUES(?1,?2,'system',?3,?3)", params![id,name,at]).map_err(err)?;
    }
    Ok(())
}

fn migrate(conn: &Connection) -> Result<(), String> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(err)?;
    if version > 2 { return Err("Database dibuat oleh versi aplikasi yang lebih baru".into()); }
    if version == 2 { return Ok(()); }
    if version == 1 { return migrate_games(conn); }
    conn.execute_batch(r#"
    BEGIN IMMEDIATE;
    CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at TEXT NOT NULL);
    CREATE TABLE devices (id TEXT PRIMARY KEY, device_name TEXT, cpu_model TEXT, ram_total_bytes INTEGER, operating_system TEXT, os_version TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
    CREATE TABLE performance_profiles (id TEXT PRIMARY KEY, name TEXT NOT NULL, profile_type TEXT NOT NULL CHECK(profile_type IN ('system','custom')), created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
    CREATE TABLE profile_settings (id TEXT PRIMARY KEY, profile_id TEXT NOT NULL REFERENCES performance_profiles(id), setting_key TEXT NOT NULL, setting_value TEXT NOT NULL, UNIQUE(profile_id,setting_key));
    CREATE TABLE registered_games (id TEXT PRIMARY KEY, display_name TEXT NOT NULL, canonical_executable_path TEXT NOT NULL, profile_id TEXT NOT NULL REFERENCES performance_profiles(id), auto_boost INTEGER NOT NULL DEFAULT 0 CHECK(auto_boost IN (0,1)), restore_on_exit INTEGER NOT NULL DEFAULT 1 CHECK(restore_on_exit IN (0,1)), ac_only INTEGER NOT NULL DEFAULT 0 CHECK(ac_only IN (0,1)), created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
    CREATE TABLE gaming_sessions (id TEXT PRIMARY KEY, game_id TEXT NOT NULL REFERENCES registered_games(id), started_at TEXT NOT NULL, ended_at TEXT, status TEXT NOT NULL, tuning_session_id TEXT);
    CREATE TABLE device_capabilities (id TEXT PRIMARY KEY, device_id TEXT NOT NULL REFERENCES devices(id), capability_key TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('supported','unsupported','requires_permission','unknown')), reason TEXT NOT NULL, detected_at TEXT NOT NULL, UNIQUE(device_id,capability_key));
    CREATE TABLE hardware_sensors (id TEXT PRIMARY KEY, device_id TEXT NOT NULL REFERENCES devices(id), provider TEXT NOT NULL, sensor_key TEXT NOT NULL, sensor_name TEXT NOT NULL, unit TEXT, is_available INTEGER NOT NULL DEFAULT 1 CHECK(is_available IN (0,1)), UNIQUE(device_id,provider,sensor_key));
    CREATE TABLE hardware_samples (id INTEGER PRIMARY KEY AUTOINCREMENT, sensor_id TEXT NOT NULL REFERENCES hardware_sensors(id), gaming_session_id TEXT REFERENCES gaming_sessions(id), recorded_at TEXT NOT NULL, value REAL NOT NULL);
    CREATE TABLE hardware_sample_aggregates (id INTEGER PRIMARY KEY AUTOINCREMENT, sensor_id TEXT NOT NULL REFERENCES hardware_sensors(id), interval_start TEXT NOT NULL, interval_end TEXT NOT NULL, sample_count INTEGER NOT NULL, min_value REAL NOT NULL, max_value REAL NOT NULL, avg_value REAL NOT NULL, UNIQUE(sensor_id,interval_start,interval_end));
    CREATE TABLE tuning_sessions (id TEXT PRIMARY KEY, profile_id TEXT NOT NULL REFERENCES performance_profiles(id), previous_scheme_guid TEXT NOT NULL, applied_scheme_guid TEXT NOT NULL, trigger TEXT NOT NULL, status TEXT NOT NULL, started_at TEXT NOT NULL, ended_at TEXT, restored_at TEXT, error TEXT);
    CREATE TABLE cleaning_scans (id TEXT PRIMARY KEY, category_snapshot TEXT NOT NULL, estimated_bytes INTEGER NOT NULL, eligible_count INTEGER NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL, expires_at TEXT NOT NULL);
    CREATE TABLE cleaning_results (id TEXT PRIMARY KEY, scan_id TEXT NOT NULL REFERENCES cleaning_scans(id), category TEXT NOT NULL, deleted_count INTEGER NOT NULL, skipped_count INTEGER NOT NULL, error_count INTEGER NOT NULL, estimated_bytes INTEGER NOT NULL, recovered_bytes INTEGER NOT NULL, status TEXT NOT NULL, finished_at TEXT NOT NULL);
    CREATE TABLE gaming_session_metrics (id TEXT PRIMARY KEY, session_id TEXT NOT NULL UNIQUE REFERENCES gaming_sessions(id), average_fps REAL, one_percent_low_fps REAL, frame_time_p99_ms REAL, hardware_summary_json TEXT);
    CREATE TABLE activity_logs (id INTEGER PRIMARY KEY AUTOINCREMENT, event_type TEXT NOT NULL, status TEXT NOT NULL, message TEXT NOT NULL, created_at TEXT NOT NULL);
    CREATE INDEX idx_samples_sensor_time ON hardware_samples(sensor_id,recorded_at);
    CREATE INDEX idx_samples_session ON hardware_samples(gaming_session_id);
    CREATE INDEX idx_aggregates_sensor_time ON hardware_sample_aggregates(sensor_id,interval_start);
    CREATE INDEX idx_tuning_status ON tuning_sessions(status,started_at);
    CREATE INDEX idx_cleaning_finished ON cleaning_results(finished_at);
    CREATE INDEX idx_logs_created ON activity_logs(created_at);
    PRAGMA user_version=1;
    COMMIT;
    "#).map_err(err)?;
    migrate_games(conn)
}

fn migrate_games(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(r#"
    BEGIN IMMEDIATE;
    ALTER TABLE registered_games ADD COLUMN executable_identity TEXT NOT NULL DEFAULT '';
    ALTER TABLE registered_games ADD COLUMN archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1));
    UPDATE registered_games SET auto_boost=0;
    CREATE UNIQUE INDEX idx_registered_identity ON registered_games(executable_identity) WHERE archived=0 AND executable_identity!='';
    CREATE INDEX idx_gaming_started ON gaming_sessions(started_at DESC);
    CREATE INDEX idx_gaming_game_status ON gaming_sessions(game_id,status);
    PRAGMA user_version=2;
    COMMIT;
    "#).map_err(err)
}

pub fn log(conn: &Connection, event: &str, status: &str, message: &str) {
    let _ = conn.execute("INSERT INTO activity_logs(event_type,status,message,created_at) VALUES(?1,?2,?3,?4)", params![event,status,message,Utc::now().to_rfc3339()]);
}

pub fn save_device(conn: &Connection, info: &DeviceInfo, capabilities: &[Capability]) -> Result<(), String> {
    let at = Utc::now().to_rfc3339();
    conn.execute("INSERT INTO devices(id,device_name,cpu_model,ram_total_bytes,operating_system,os_version,created_at,updated_at) VALUES('local-device',?1,?2,?3,?4,?4,?5,?5) ON CONFLICT(id) DO UPDATE SET device_name=excluded.device_name,cpu_model=excluded.cpu_model,ram_total_bytes=excluded.ram_total_bytes,operating_system=excluded.operating_system,os_version=excluded.os_version,updated_at=excluded.updated_at", params![info.device_name,info.cpu_model,info.ram_total_bytes as i64,info.operating_system,at]).map_err(err)?;
    for c in capabilities {
        conn.execute("INSERT INTO device_capabilities(id,device_id,capability_key,status,reason,detected_at) VALUES(?1,'local-device',?2,?3,?4,?5) ON CONFLICT(device_id,capability_key) DO UPDATE SET status=excluded.status,reason=excluded.reason,detected_at=excluded.detected_at", params![Uuid::new_v4().to_string(),c.capability_key,c.status,c.reason,c.detected_at]).map_err(err)?;
    }
    for (index, name) in info.gpu_adapters.iter().enumerate() {
        let key = format!("gpu_adapter:{index}");
        conn.execute("INSERT INTO hardware_sensors(id,device_id,provider,sensor_key,sensor_name,unit,is_available) VALUES(?1,'local-device','Windows/CIM',?2,?3,NULL,0) ON CONFLICT(device_id,provider,sensor_key) DO UPDATE SET sensor_name=excluded.sensor_name", params![Uuid::new_v4().to_string(),key,name]).map_err(err)?;
    }
    Ok(())
}

pub fn load_settings(conn: &Connection) -> AppSettings {
    let value: Option<String> = conn.query_row("SELECT value FROM app_settings WHERE key='preferences'", [], |r| r.get(0)).optional().ok().flatten();
    value.and_then(|v| serde_json::from_str::<AppSettings>(&v).ok()).filter(|settings| settings.validate().is_ok()).unwrap_or_default()
}
pub fn save_settings(conn: &Connection, settings: &AppSettings) -> Result<(), String> {
    settings.validate()?;
    let value = serde_json::to_string(settings).map_err(err)?;
    conn.execute("INSERT INTO app_settings(key,value,updated_at) VALUES('preferences',?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at", params![value,Utc::now().to_rfc3339()]).map_err(err)?;
    Ok(())
}

pub fn profiles(conn: &Connection) -> Result<Vec<PerformanceProfile>, String> {
    let mut stmt = conn.prepare("SELECT p.id,p.name,(SELECT setting_value FROM profile_settings WHERE profile_id=p.id AND setting_key='scheme_guid'),(SELECT setting_value FROM profile_settings WHERE profile_id=p.id AND setting_key='ac_only') FROM performance_profiles p ORDER BY CASE p.id WHEN 'balanced' THEN 0 WHEN 'gaming' THEN 1 ELSE 2 END").map_err(err)?;
    let rows = stmt.query_map([], |r| { let ac: Option<String> = r.get(3)?; Ok(PerformanceProfile { id:r.get(0)?,name:r.get(1)?,scheme_guid:r.get(2)?,ac_only:ac.as_deref()==Some("true") }) }).map_err(err)?;
    rows.collect::<Result<Vec<_>,_>>().map_err(err)
}
pub fn save_profile_mapping(conn: &mut Connection, profile_id: &str, guid: &str, ac_only: bool) -> Result<(), String> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(err)?;
    let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM performance_profiles WHERE id=?1)", [profile_id], |r| r.get(0)).map_err(err)?;
    if !exists { return Err("Profil tidak dikenal".into()); }
    if unfinished_sessions(&tx)?.iter().any(|session| session.profile_id == profile_id) { return Err("Pulihkan sesi profil ini sebelum mengubah pemetaan".into()); }
    for (key,value) in [("scheme_guid",guid),("ac_only",if ac_only {"true"} else {"false"})] {
        tx.execute("INSERT INTO profile_settings(id,profile_id,setting_key,setting_value) VALUES(?1,?2,?3,?4) ON CONFLICT(profile_id,setting_key) DO UPDATE SET setting_value=excluded.setting_value", params![Uuid::new_v4().to_string(),profile_id,key,value]).map_err(err)?;
    }
    tx.commit().map_err(err)
}

pub fn unfinished_sessions(conn: &Connection) -> Result<Vec<TuningSession>, String> {
    let mut stmt = conn.prepare("SELECT id,profile_id,previous_scheme_guid,applied_scheme_guid,status,started_at,error FROM tuning_sessions WHERE status IN ('pending','active','restoring','conflict') ORDER BY started_at DESC LIMIT 20").map_err(err)?;
    let rows = stmt.query_map([], |r| Ok(TuningSession { id:r.get(0)?,profile_id:r.get(1)?,previous_guid:r.get(2)?,applied_guid:r.get(3)?,status:r.get(4)?,started_at:r.get(5)?,error:r.get(6)? })).map_err(err)?;
    rows.collect::<Result<Vec<_>,_>>().map_err(err)
}

pub fn save_samples(conn: &mut Connection, snapshot: &HardwareSnapshot) -> Result<(), String> {
    let values = [
        ("cpu_usage", &snapshot.cpu_usage), ("cpu_temperature", &snapshot.cpu_temperature), ("gpu_usage", &snapshot.gpu_usage),
        ("gpu_temperature", &snapshot.gpu_temperature), ("ram_usage", &snapshot.ram_usage), ("ram_used_gb", &snapshot.ram_used_gb),
        ("disk_used_gb", &snapshot.disk_used_gb), ("network_down_kbps", &snapshot.network_down_kbps), ("network_up_kbps", &snapshot.network_up_kbps),
    ];
    let tx = conn.transaction().map_err(err)?;
    // History on a fresh install must not depend on slow GPU discovery.
    // Reserve the logical device inside the same transaction; later discovery
    // fills its metadata without replacing the row or its sensor foreign keys.
    tx.execute("INSERT OR IGNORE INTO devices(id,created_at,updated_at) VALUES('local-device',?1,?1)", [&snapshot.recorded_at]).map_err(err)?;
    for (key, metric) in values {
        if let Some(value) = metric.value.filter(|v| v.is_finite()) {
            let sensor_id = format!("local-device:{}:{}", metric.source, key);
            tx.execute("INSERT OR IGNORE INTO hardware_sensors(id,device_id,provider,sensor_key,sensor_name,unit,is_available) VALUES(?1,'local-device',?2,?3,?3,?4,1)", params![sensor_id,metric.source,key,metric.unit]).map_err(err)?;
            tx.execute("INSERT INTO hardware_samples(sensor_id,recorded_at,value) VALUES(?1,?2,?3)", params![sensor_id,metric.recorded_at,value]).map_err(err)?;
        }
    }
    tx.commit().map_err(err)
}

pub fn history_for_gpu(conn: &Connection, sensor_key: &str, hours: u32, gpu_source: Option<&str>) -> Result<Vec<HistoryPoint>, String> {
    if !["cpu_usage","cpu_temperature","gpu_usage","gpu_temperature","ram_usage","ram_used_gb","disk_used_gb","network_down_kbps","network_up_kbps"].contains(&sensor_key) || !(1..=72).contains(&hours) { return Err("Permintaan riwayat tidak valid".into()); }
    let since = (Utc::now() - Duration::hours(hours as i64)).to_rfc3339();
    let mut stmt = conn.prepare("SELECT s.recorded_at,s.value FROM hardware_samples s JOIN hardware_sensors h ON h.id=s.sensor_id WHERE h.sensor_key=?1 AND s.recorded_at>=?2 AND (?3 IS NULL OR h.sensor_key NOT IN ('gpu_usage','gpu_temperature') OR h.provider=?3) ORDER BY s.recorded_at DESC LIMIT 500").map_err(err)?;
    let mut result: Vec<HistoryPoint> = stmt.query_map(params![sensor_key,since,gpu_source], |r| Ok(HistoryPoint { recorded_at:r.get(0)?,value:r.get(1)? })).map_err(err)?.collect::<Result<Vec<_>,_>>().map_err(err)?;
    result.reverse(); Ok(result)
}

#[cfg(test)]
fn analytics(conn: &Connection, minutes: u32) -> Result<AnalyticsReport, String> { analytics_for_gpu(conn, minutes, None) }
pub fn analytics_for_gpu(conn: &Connection, minutes: u32, gpu_source: Option<&str>) -> Result<AnalyticsReport, String> {
    let bucket_seconds = match minutes { 15 => 5, 60 => 15, 360 => 60, 1440 => 300, 4320 => 900, _ => return Err("Rentang analitik tidak valid".into()) };
    let to = Utc::now();
    let from = to - Duration::minutes(minutes as i64);
    let mut stmt = conn.prepare(r#"
        WITH observations AS (
            SELECT h.sensor_key, unixepoch(s.recorded_at) AS at,
                   s.value AS average, s.value AS minimum, s.value AS maximum, 1 AS count
            FROM hardware_samples s JOIN hardware_sensors h ON h.id=s.sensor_id
            WHERE unixepoch(s.recorded_at)>=?1 AND unixepoch(s.recorded_at)<=?2
              AND (?4 IS NULL OR h.sensor_key NOT IN ('gpu_usage','gpu_temperature') OR h.provider=?4)
            UNION ALL
            SELECT h.sensor_key, unixepoch(a.interval_start), a.avg_value, a.min_value, a.max_value, a.sample_count
            FROM hardware_sample_aggregates a JOIN hardware_sensors h ON h.id=a.sensor_id
            WHERE unixepoch(a.interval_start)>=?1 AND unixepoch(a.interval_end)<=?2
              AND (?4 IS NULL OR h.sensor_key NOT IN ('gpu_usage','gpu_temperature') OR h.provider=?4)
        )
        SELECT sensor_key, strftime('%Y-%m-%dT%H:%M:%SZ', at / ?3 * ?3, 'unixepoch'),
               SUM(average * count) / SUM(count), MIN(minimum), MAX(maximum), SUM(count)
        FROM observations WHERE count>0
        GROUP BY sensor_key, at / ?3 ORDER BY at / ?3, sensor_key
    "#).map_err(err)?;
    let points = stmt.query_map(params![from.timestamp(),to.timestamp(),bucket_seconds,gpu_source], |row| Ok(AnalyticsPoint {
        sensor_key: row.get(0)?, recorded_at: row.get(1)?, average: row.get(2)?, minimum: row.get(3)?, maximum: row.get(4)?, sample_count: row.get(5)?,
    })).map_err(err)?.collect::<Result<Vec<_>,_>>().map_err(err)?;
    Ok(AnalyticsReport { from: from.to_rfc3339(), to: to.to_rfc3339(), bucket_seconds, points })
}

pub fn maintain(conn: &Connection, hours: u32) -> Result<(), String> {
    maintain_at(conn, hours, Utc::now())
}

fn maintain_at(conn: &Connection, hours: u32, now: chrono::DateTime<Utc>) -> Result<(), String> {
    // Move only whole minutes. A moving cutoff inside a minute used to overwrite
    // that minute's already-retained samples on the next maintenance cycle.
    let seconds = (now - Duration::hours(hours as i64)).timestamp() / 60 * 60;
    let cutoff = chrono::DateTime::from_timestamp(seconds, 0).ok_or("Retensi tidak valid")?.to_rfc3339();
    // Aggregate and delete atomically: a crash must neither double-count retained
    // samples nor delete raw samples whose aggregation failed.
    let tx = conn.unchecked_transaction().map_err(err)?;
    tx.execute(r#"
        INSERT INTO hardware_sample_aggregates(sensor_id,interval_start,interval_end,sample_count,min_value,max_value,avg_value)
        SELECT sensor_id,strftime('%Y-%m-%dT%H:%M:00Z',recorded_at),
               strftime('%Y-%m-%dT%H:%M:00Z',(unixepoch(recorded_at)/60*60)+60,'unixepoch'),
               COUNT(*),MIN(value),MAX(value),AVG(value)
        FROM hardware_samples WHERE recorded_at<?1
        GROUP BY sensor_id,strftime('%Y-%m-%dT%H:%M:00Z',recorded_at)
        ON CONFLICT(sensor_id,interval_start,interval_end) DO UPDATE SET
            avg_value=(hardware_sample_aggregates.avg_value * hardware_sample_aggregates.sample_count + excluded.avg_value * excluded.sample_count)
                      / (hardware_sample_aggregates.sample_count + excluded.sample_count),
            sample_count=hardware_sample_aggregates.sample_count + excluded.sample_count,
            min_value=MIN(hardware_sample_aggregates.min_value, excluded.min_value),
            max_value=MAX(hardware_sample_aggregates.max_value, excluded.max_value)
    "#, [cutoff.as_str()]).map_err(err)?;
    tx.execute("DELETE FROM hardware_samples WHERE recorded_at<?1", [cutoff.as_str()]).map_err(err)?;
    let aggregate_cutoff = (now - Duration::days(30)).to_rfc3339();
    tx.execute("DELETE FROM hardware_sample_aggregates WHERE interval_start<?1", [aggregate_cutoff.as_str()]).map_err(err)?;
    let activity_cutoff = (now - Duration::days(90)).to_rfc3339();
    tx.execute("DELETE FROM activity_logs WHERE created_at<?1", [activity_cutoff.as_str()]).map_err(err)?;
    tx.execute("DELETE FROM cleaning_results WHERE finished_at<?1", [activity_cutoff.as_str()]).map_err(err)?;
    tx.execute("DELETE FROM cleaning_scans WHERE created_at<?1 AND id NOT IN (SELECT scan_id FROM cleaning_results)", [activity_cutoff.as_str()]).map_err(err)?;
    tx.execute("DELETE FROM tuning_sessions WHERE status IN ('restored','failed') AND ended_at<?1", [activity_cutoff.as_str()]).map_err(err)?;
    tx.commit().map_err(err)?;
    Ok(())
}

pub fn save_cleaning_result(conn: &Connection, scan_id: &str, result: &CleaningResult) -> Result<(), String> {
    let tx = conn.unchecked_transaction().map_err(err)?;
    tx.execute("INSERT INTO cleaning_results(id,scan_id,category,deleted_count,skipped_count,error_count,estimated_bytes,recovered_bytes,status,finished_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![result.id,scan_id,result.category,result.deleted_count as i64,result.skipped_count as i64,result.error_count as i64,result.estimated_bytes as i64,result.recovered_bytes as i64,result.status,result.finished_at]).map_err(err)?;
    tx.execute("UPDATE cleaning_scans SET status=?2 WHERE id=?1", params![scan_id,result.status]).map_err(err)?;
    tx.commit().map_err(err)?;
    Ok(())
}
pub fn cleaning_history(conn: &Connection) -> Result<Vec<CleaningResult>, String> {
    let mut stmt = conn.prepare("SELECT id,category,estimated_bytes,recovered_bytes,deleted_count,skipped_count,error_count,finished_at,status FROM cleaning_results ORDER BY finished_at DESC LIMIT 50").map_err(err)?;
    let rows = stmt.query_map([], |r| Ok(CleaningResult { id:r.get(0)?,category:r.get(1)?,estimated_bytes:r.get(2)?,recovered_bytes:r.get(3)?,deleted_count:r.get(4)?,skipped_count:r.get(5)?,error_count:r.get(6)?,finished_at:r.get(7)?,status:r.get(8)? })).map_err(err)?;
    rows.collect::<Result<Vec<_>,_>>().map_err(err)
}

pub fn purge_monitoring_history(conn: &mut Connection) -> Result<usize, String> {
    let tx = conn.transaction().map_err(err)?;
    let deleted = tx.execute("DELETE FROM hardware_samples", []).map_err(err)?;
    tx.execute("DELETE FROM hardware_sample_aggregates", []).map_err(err)?;
    tx.commit().map_err(err)?;
    log(conn,"history","purged",&format!("{deleted} raw samples purged by user"));
    Ok(deleted)
}

#[cfg(test)]
mod tests {

    #[test]
    fn version_one_upgrade_preserves_settings_history_and_power_recovery_and_disarms_unverified_games() {
        let conn=rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute_batch(include_str!("../../tests/fixtures/schema-v1.sql")).unwrap();
        super::seed_profiles(&conn).unwrap();
        let preferences=crate::models::AppSettings {theme:"light".into(),..Default::default()};
        super::save_settings(&conn,&preferences).unwrap();
        conn.execute("INSERT INTO registered_games(id,display_name,canonical_executable_path,profile_id,auto_boost,created_at,updated_at) VALUES('legacy','Legacy','C:/game.exe','gaming',1,'2026-01-01','2026-01-01')",[]).unwrap();
        conn.execute("INSERT INTO gaming_sessions(id,game_id,started_at,status) VALUES('past','legacy','2026-01-01','completed')",[]).unwrap();
        conn.execute("INSERT INTO tuning_sessions(id,profile_id,previous_scheme_guid,applied_scheme_guid,trigger,status,started_at) VALUES('recovery','gaming','prior','target','manual','active','2026-01-01')",[]).unwrap();
        super::migrate(&conn).unwrap();
        assert_eq!(super::load_settings(&conn).theme,"light");
        let game=crate::games::get_games(&conn).unwrap().remove(0);
        assert!(!game.auto_boost && game.executable_identity.is_empty());
        assert_eq!(super::unfinished_sessions(&conn).unwrap()[0].id,"recovery");
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM gaming_sessions",[],|row|row.get::<_,usize>(0)).unwrap(),1);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check",[],|row|row.get::<_,usize>(0)).unwrap(),0);
        assert_eq!(conn.query_row("PRAGMA user_version",[],|row|row.get::<_,u32>(0)).unwrap(),2);
        super::migrate(&conn).unwrap();
        conn.execute_batch("PRAGMA user_version=3;").unwrap();
        assert!(super::migrate(&conn).is_err());
        assert_eq!(super::load_settings(&conn).theme,"light");
    }
    use super::*;

    #[test]
    fn fresh_install_history_can_save_before_device_discovery_finishes() {
        let mut conn = open(Path::new(":memory:")).unwrap();
        let info = DeviceInfo { device_name: "fixture PC".into(), operating_system: "Windows fixture".into(), cpu_model: "fixture CPU".into(), cpu_cores: 8, ram_total_bytes: 16 * 1024 * 1024 * 1024, gpu_adapters: vec![], power_source: "AC power".into() };
        let mut snapshot = crate::empty_snapshot(&info);
        snapshot.cpu_usage = crate::models::Metric::measured(0.0, "%", "fixture", &snapshot.recorded_at);
        save_samples(&mut conn, &snapshot).unwrap();
        assert_eq!(conn.query_row("SELECT COUNT(*),MIN(value) FROM hardware_samples", [], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))).unwrap(), (1, 0.0));
        save_device(&conn, &info, &[]).unwrap();
        save_samples(&mut conn, &snapshot).unwrap();
        assert_eq!(conn.query_row("SELECT device_name FROM devices WHERE id='local-device'", [], |row| row.get::<_, String>(0)).unwrap(), "fixture PC");
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM hardware_samples", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM hardware_sensors", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    }

    fn history_fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute("INSERT INTO devices(id,created_at,updated_at) VALUES('local-device','now','now')", []).unwrap();
        conn.execute("INSERT INTO hardware_sensors(id,device_id,provider,sensor_key,sensor_name) VALUES('cpu','local-device','test','cpu_usage','CPU')", []).unwrap();
        conn
    }
    #[test]
    fn gpu_history_and_analytics_do_not_mix_adapters_in_raw_or_retained_data() {
        let conn = history_fixture();
        let at = (Utc::now() - Duration::minutes(2)).to_rfc3339();
        let end = (Utc::now() - Duration::minutes(1)).to_rfc3339();
        for (id, provider, value) in [("gpu-a", "provider-a", 0.0), ("gpu-b", "provider-b", 99.0)] {
            conn.execute("INSERT INTO hardware_sensors(id,device_id,provider,sensor_key,sensor_name) VALUES(?1,'local-device',?2,'gpu_usage','GPU')", params![id, provider]).unwrap();
            conn.execute("INSERT INTO hardware_samples(sensor_id,recorded_at,value) VALUES(?1,?2,?3)", params![id, at, value]).unwrap();
            conn.execute("INSERT INTO hardware_sample_aggregates(sensor_id,interval_start,interval_end,sample_count,min_value,max_value,avg_value) VALUES(?1,?2,?3,2,?4,?4,?4)", params![id, at, end, value]).unwrap();
        }
        conn.execute("INSERT INTO hardware_samples(sensor_id,recorded_at,value) VALUES('cpu',?1,50)", [&at]).unwrap();
        assert_eq!(history_for_gpu(&conn, "gpu_usage", 1, Some("provider-a")).unwrap().iter().map(|p| p.value).collect::<Vec<_>>(), [0.0]);
        assert!(history_for_gpu(&conn, "gpu_usage", 1, Some("missing")).unwrap().is_empty());
        for (source, expected) in [("provider-a", 0.0), ("provider-b", 99.0)] {
            let report = analytics_for_gpu(&conn, 15, Some(source)).unwrap();
            let gpu: Vec<_> = report.points.iter().filter(|p| p.sensor_key == "gpu_usage").collect();
            assert_eq!(gpu.iter().map(|p| p.sample_count).sum::<u64>(), 3);
            assert!(gpu.iter().all(|p| p.average == expected && p.minimum == expected && p.maximum == expected));
            assert!(report.points.iter().any(|p| p.sensor_key == "cpu_usage" && p.average == 50.0));
        }
    }

    #[test]
    fn retention_keeps_whole_minutes_and_merges_late_samples_without_loss() {
        let conn = history_fixture();
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-05T12:00:30Z").unwrap().with_timezone(&Utc);
        for (at, value) in [("2026-10-05T05:59:10+00:00", 20.0), ("2026-10-05T06:00:10+00:00", 40.0), ("2026-10-05T06:00:50+00:00", 80.0)] {
            conn.execute("INSERT INTO hardware_samples(sensor_id,recorded_at,value) VALUES('cpu',?1,?2)", params![at, value]).unwrap();
        }
        maintain_at(&conn, 6, now).unwrap();
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM hardware_samples", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
        // A delayed write for an already-aggregated minute is merged, not replaced.
        conn.execute("INSERT INTO hardware_samples(sensor_id,recorded_at,value) VALUES('cpu','2026-10-05T05:59:20+00:00',60)", []).unwrap();
        maintain_at(&conn, 6, now + Duration::seconds(60)).unwrap();
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM hardware_samples", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        let retained: (i64, f64, f64, f64) = conn.query_row("SELECT SUM(sample_count),SUM(avg_value * sample_count)/SUM(sample_count),MIN(min_value),MAX(max_value) FROM hardware_sample_aggregates", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).unwrap();
        assert_eq!(retained, (4, 50.0, 20.0, 80.0));
        maintain_at(&conn, 6, now + Duration::seconds(120)).unwrap();
        assert_eq!(conn.query_row("SELECT SUM(sample_count) FROM hardware_sample_aggregates", [], |row| row.get::<_, i64>(0)).unwrap(), 4);
    }

    #[test]
    fn failed_retention_rolls_back_aggregation_and_preserves_raw_data() {
        let conn = history_fixture();
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-05T12:00:30Z").unwrap().with_timezone(&Utc);
        conn.execute("INSERT INTO hardware_samples(sensor_id,recorded_at,value) VALUES('cpu','2026-10-05T05:00:00+00:00',25)", []).unwrap();
        conn.execute_batch("CREATE TRIGGER simulate_failure BEFORE DELETE ON hardware_samples BEGIN SELECT RAISE(ABORT,'simulated disk failure'); END;").unwrap();
        assert!(maintain_at(&conn, 6, now).is_err());
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM hardware_samples", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM hardware_sample_aggregates", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        conn.execute_batch("DROP TRIGGER simulate_failure;").unwrap();
        maintain_at(&conn, 6, now).unwrap();
        assert_eq!(conn.query_row("SELECT SUM(sample_count) FROM hardware_sample_aggregates", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    }

    #[test]
    fn retention_removes_expired_history_but_keeps_unfinished_recovery() {
        let conn = history_fixture();
        seed_profiles(&conn).unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-05T12:00:30Z").unwrap().with_timezone(&Utc);
        conn.execute("INSERT INTO hardware_sample_aggregates(sensor_id,interval_start,interval_end,sample_count,min_value,max_value,avg_value) VALUES('cpu','2026-01-01T00:00:00Z','2026-01-01T00:01:00Z',1,25,25,25)", []).unwrap();
        conn.execute("INSERT INTO activity_logs(event_type,status,message,created_at) VALUES('test','completed','old','2026-01-01T00:00:00Z')", []).unwrap();
        conn.execute("INSERT INTO tuning_sessions(id,profile_id,previous_scheme_guid,applied_scheme_guid,trigger,status,started_at) VALUES('unfinished','balanced','previous','applied','manual','active','2026-01-01T00:00:00Z')", []).unwrap();
        maintain_at(&conn, 6, now).unwrap();
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM hardware_sample_aggregates", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM activity_logs", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(unfinished_sessions(&conn).unwrap().len(), 1);
    }

    #[test]
    fn analytics_combines_raw_and_retained_samples_with_weighted_statistics() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute("INSERT INTO devices(id,created_at,updated_at) VALUES('local-device','now','now')", []).unwrap();
        conn.execute("INSERT INTO hardware_sensors(id,device_id,provider,sensor_key,sensor_name) VALUES('cpu','local-device','test','cpu_usage','CPU')", []).unwrap();
        let at = (Utc::now() - Duration::minutes(10)).timestamp() / 900 * 900;
        let start = chrono::DateTime::from_timestamp(at,0).unwrap().to_rfc3339();
        let end = chrono::DateTime::from_timestamp(at+60,0).unwrap().to_rfc3339();
        conn.execute("INSERT INTO hardware_sample_aggregates(sensor_id,interval_start,interval_end,sample_count,min_value,max_value,avg_value) VALUES('cpu',?1,?2,9,20,80,50)", params![start,end]).unwrap();
        let raw_at = chrono::DateTime::from_timestamp(at+65,0).unwrap().to_rfc3339();
        conn.execute("INSERT INTO hardware_samples(sensor_id,recorded_at,value) VALUES('cpu',?1,0)", [raw_at]).unwrap();
        let report = analytics(&conn,4320).unwrap();
        let cpu = &report.points[0];
        assert_eq!(cpu.sample_count,10);
        assert_eq!(cpu.average,45.0);
        assert_eq!(cpu.minimum,0.0);
        assert_eq!(cpu.maximum,80.0);
        assert_eq!(report.points.len(),1);
        assert!(analytics(&conn,0).is_err());
    }

    #[test]
    fn existing_preferences_keep_their_values_when_language_is_added() {
        let settings: AppSettings = serde_json::from_str(r#"{"refresh_seconds":5,"theme":"light","history_retention_hours":72,"monitor_in_background":true}"#).unwrap();
        assert_eq!(settings.language,"id");
        assert_eq!(settings.theme,"light");
        assert_eq!(settings.refresh_seconds,5);
        assert!(settings.validate().is_ok());
        let mut unsupported = settings;
        unsupported.language = "unknown".into();
        assert!(unsupported.validate().is_err());
    }

    #[test]
    fn migration_enforces_foreign_keys_and_seeds_profiles() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrate(&conn).unwrap();
        seed_profiles(&conn).unwrap();
        assert_eq!(profiles(&conn).unwrap().len(), 3);
        assert!(conn.execute("INSERT INTO hardware_samples(sensor_id,recorded_at,value) VALUES('missing','2026-01-01T00:00:00Z',1)", []).is_err());
        conn.execute("INSERT INTO devices(id,created_at,updated_at) VALUES('local-device','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')", []).unwrap();
        conn.execute("INSERT INTO hardware_sensors(id,device_id,provider,sensor_key,sensor_name) VALUES('sensor','local-device','test','cpu_usage','CPU')", []).unwrap();
        conn.execute("INSERT INTO hardware_samples(sensor_id,recorded_at,value) VALUES('sensor','2026-01-01T00:00:00Z',0)", []).unwrap();
        assert_eq!(purge_monitoring_history(&mut conn).unwrap(), 1);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM hardware_samples", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(profiles(&conn).unwrap().len(), 3);
    }
}
