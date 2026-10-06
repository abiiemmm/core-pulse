mod cleaner;
mod db;
mod hardware;
mod models;
mod power;
mod windows_command;
mod persistence;
mod processes;
mod sensors;

use crate::models::*;
use chrono::Utc;
use rusqlite::Connection;
use std::{collections::HashMap, path::PathBuf, sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex}, thread, time::{Duration, Instant}};
use tauri::{Emitter, Manager, State};

struct Inner {
    db: Arc<Mutex<Connection>>,
    info: Mutex<DeviceInfo>,
    capabilities: Mutex<Vec<Capability>>,
    snapshot: Mutex<HardwareSnapshot>,
    settings: Mutex<AppSettings>,
    settings_update: Mutex<()>,
    detection: Mutex<()>,
    processes: Mutex<processes::Collector>,
    sensors: sensors::Service,
    history_writer: persistence::HistoryWriter,
    monitoring: AtomicBool,
    cleaner_plans: Mutex<HashMap<String, cleaner::Plan>>,
    cancel_cleanup: AtomicBool,
    cleanup_running: AtomicBool,
    power: power::Controller,
    #[allow(dead_code)] db_path: PathBuf,
}
struct AppState { inner: Arc<Inner> }
struct CleanupTaskGuard(Arc<Inner>);
impl Drop for CleanupTaskGuard {
    fn drop(&mut self) { self.0.cleanup_running.store(false, Ordering::SeqCst); }
}

fn lock_error() -> String { "State aplikasi sedang tidak tersedia".into() }
fn empty_snapshot(info: &DeviceInfo) -> HardwareSnapshot {
    let at = Utc::now().to_rfc3339();
    let blank = |unit: &str| Metric::unavailable(unit, "Menunggu sampel", &at);
    HardwareSnapshot { recorded_at: at.clone(), cpu_usage:blank("%"),cpu_temperature:blank("°C"),gpu_usage:blank("%"),gpu_temperature:blank("°C"),ram_usage:blank("%"),ram_used_gb:blank("GB"),ram_total_gb:blank("GB"),disk_used_gb:blank("GB"),disk_total_gb:blank("GB"),network_down_kbps:blank("KB/s"),network_up_kbps:blank("KB/s"),cpu_name:info.cpu_model.clone(),gpu_name:info.gpu_adapters.first().cloned().unwrap_or_else(|| "GPU unavailable".into()),disk_name:"Storage".into(),power_source:info.power_source.clone() }
}

fn start_sampler(inner: Arc<Inner>, app: tauri::AppHandle) {
    thread::spawn(move || {
        let mut collector = hardware::Collector::new();
        let mut last_sample = Instant::now() - Duration::from_secs(5);
        let mut last_persist = Instant::now() - Duration::from_secs(5);
        loop {
            thread::sleep(Duration::from_millis(200));
            let settings = match inner.settings.lock() { Ok(s) => s.clone(), Err(_) => continue };
            let minimized = app.get_webview_window("main").and_then(|w| w.is_minimized().ok()).unwrap_or(false);
            let enabled = inner.monitoring.load(Ordering::Relaxed) && (!minimized || settings.monitor_in_background);
            inner.sensors.configure(enabled, settings.refresh_seconds);
            if !enabled { continue; }
            if last_sample.elapsed() < Duration::from_secs(settings.refresh_seconds as u64) { continue; }
            last_sample = Instant::now();
            let info = match inner.info.lock() { Ok(i) => i.clone(), Err(_) => continue };
            let mut sample = collector.sample(&info);
            inner.sensors.merge(&mut sample, settings.gpu_adapter_id.as_deref(), settings.refresh_seconds);
            if let Ok(mut latest) = inner.snapshot.lock() { *latest = sample.clone(); }
            let _ = app.emit("hardware:update", &sample);
            if last_persist.elapsed() >= Duration::from_secs(5) {
                last_persist = Instant::now();
                inner.history_writer.enqueue(sample);
            }
        }
    });
}

// Safety watchdog is independent of sampling, UI pause, and window visibility.
// Only this runtime's successful AC-only activation is eligible for auto restore.
fn start_power_watchdog(inner: Arc<Inner>, app: tauri::AppHandle) {
    thread::spawn(move || {
        let mut last_attempt: Option<(String, Instant)> = None;
        loop {
            thread::sleep(Duration::from_secs(1));
            let Some(session_id) = inner.power.owned_ac_session() else { last_attempt = None; continue; };
            if hardware::power_source() == "AC power" { continue; }
            if last_attempt.as_ref().is_some_and(|(id, at)| id == &session_id && at.elapsed() < Duration::from_secs(30)) { continue; }
            last_attempt = Some((session_id.clone(), Instant::now()));
            match inner.power.restore(&inner.db, &session_id, false) {
                Ok(restored) => { let _ = app.emit("tuning:changed", restored); }
                Err(error) => { let _ = app.emit("app:error", serde_json::json!({"status":"error","timestamp":Utc::now().to_rfc3339(),"message":error})); }
            }
        }
    });
}

async fn database_work<T: Send + 'static>(inner: Arc<Inner>, operation: impl FnOnce(&mut Connection) -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut conn = inner.db.lock().map_err(|_| lock_error())?;
        operation(&mut conn)
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
fn get_system_information(state: State<'_, AppState>) -> Result<DeviceInfo, String> { state.inner.info.lock().map(|i| i.clone()).map_err(|_| lock_error()) }

#[tauri::command]
async fn detect_hardware(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<DeviceInfo, String> {
    let inner = state.inner.clone();
    tauri::async_runtime::spawn_blocking(move || refresh_device(&inner, &app)).await.map_err(|e| e.to_string())?
}

fn refresh_device(inner: &Inner, app: &tauri::AppHandle) -> Result<DeviceInfo, String> {
    let _detection = inner.detection.lock().map_err(|_| lock_error())?;
    let info = hardware::discover();
    let capabilities = hardware::capabilities(&info, power::plans().map(|p| !p.is_empty()).unwrap_or(false));
    *inner.info.lock().map_err(|_| lock_error())? = info.clone();
    *inner.capabilities.lock().map_err(|_| lock_error())? = capabilities.clone();
    // Live metadata remains available even if persistence is temporarily broken.
    let _ = app.emit("device:changed", &info);
    let conn = inner.db.lock().map_err(|_| lock_error())?;
    db::save_device(&conn, &info, &capabilities).map_err(|_| "Informasi perangkat terbaca, tetapi gagal disimpan".to_string())?;
    Ok(info)
}

#[tauri::command]
fn get_device_capabilities(state: State<'_, AppState>) -> Result<Vec<Capability>, String> {
    let mut capabilities = state.inner.capabilities.lock().map(|c| c.clone()).map_err(|_| lock_error())?;
    let settings = state.inner.settings.lock().map_err(|_| lock_error())?.clone();
    let mut sample = state.inner.snapshot.lock().map_err(|_| lock_error())?.clone();
    state.inner.sensors.merge(&mut sample, settings.gpu_adapter_id.as_deref(), settings.refresh_seconds);
    for (key, metric) in [("cpu_temperature", sample.cpu_temperature), ("gpu_temperature", sample.gpu_temperature), ("gpu_usage", sample.gpu_usage)] {
        if let Some(capability) = capabilities.iter_mut().find(|capability| capability.capability_key == key) {
            capability.status = metric.status;
            capability.reason = if key == "cpu_temperature" { "Pembacaan suhu CPU menunggu verifikasi akses driver".into() } else if metric.value.is_some() { metric.source } else { "Sensor belum tersedia dari adapter yang dipilih".into() };
            capability.detected_at = Utc::now().to_rfc3339();
        }
    }
    Ok(capabilities)
}
#[tauri::command]
fn get_sensor_inventory(state: State<'_, AppState>) -> sensors::Inventory { state.inner.sensors.inventory() }
#[tauri::command]
fn get_hardware_snapshot(state: State<'_, AppState>) -> Result<HardwareSnapshot, String> {
    let settings = state.inner.settings.lock().map_err(|_| lock_error())?.clone();
    let mut sample = state.inner.snapshot.lock().map_err(|_| lock_error())?.clone();
    state.inner.sensors.merge(&mut sample, settings.gpu_adapter_id.as_deref(), settings.refresh_seconds);
    Ok(sample)
}
#[tauri::command]
async fn get_process_snapshot(state: State<'_, AppState>) -> Result<ProcessSnapshot, String> {
    let inner = state.inner.clone();
    tauri::async_runtime::spawn_blocking(move || inner.processes.lock().map(|mut collector| collector.sample()).map_err(|_| lock_error())).await.map_err(|error| error.to_string())?
}
#[tauri::command]
fn start_monitoring(state: State<'_, AppState>, app: tauri::AppHandle) { state.inner.monitoring.store(true,Ordering::Relaxed); let _ = app.emit("monitoring:status",serde_json::json!({"status":"running","timestamp":Utc::now().to_rfc3339()})); }
#[tauri::command]
fn stop_monitoring(state: State<'_, AppState>, app: tauri::AppHandle) { state.inner.monitoring.store(false,Ordering::Relaxed); let _ = app.emit("monitoring:status",serde_json::json!({"status":"stopped","timestamp":Utc::now().to_rfc3339()})); }

#[tauri::command]
async fn get_hardware_history(state: State<'_, AppState>, sensor_key: String, hours: u32) -> Result<Vec<HistoryPoint>, String> {
    let source = current_gpu_source(&state.inner)?;
    database_work(state.inner.clone(), move |conn| db::history_for_gpu(conn,&sensor_key,hours,Some(&source))).await
}
fn current_gpu_source(inner: &Inner) -> Result<String, String> {
    let selected = inner.settings.lock().map_err(|_| lock_error())?.gpu_adapter_id.clone();
    Ok(inner.sensors.selected_gpu_source(selected.as_deref()).unwrap_or_else(|| "unresolved-gpu".into()))
}

#[tauri::command]
async fn get_power_plans() -> Result<Vec<PowerPlan>, String> { tauri::async_runtime::spawn_blocking(power::plans).await.map_err(|e| e.to_string())? }
#[tauri::command]
async fn get_analytics_report(state: State<'_, AppState>, minutes: u32) -> Result<AnalyticsReport, String> {
    let source = current_gpu_source(&state.inner)?;
    database_work(state.inner.clone(), move |conn| db::analytics_for_gpu(conn,minutes,Some(&source))).await
}
#[tauri::command]
async fn get_active_power_plan() -> Result<PowerPlan, String> {
    tauri::async_runtime::spawn_blocking(|| power::plans()?.into_iter().find(|p| p.active).ok_or("Power plan aktif tidak ditemukan".into())).await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn get_performance_profiles(state: State<'_, AppState>) -> Result<Vec<PerformanceProfile>, String> {
    database_work(state.inner.clone(), |conn| db::profiles(conn)).await
}
#[tauri::command]
async fn update_profile_mapping(state: State<'_, AppState>, profile_id: String, guid: String, ac_only: bool) -> Result<PerformanceProfile, String> {
    let inner = state.inner.clone();
    tauri::async_runtime::spawn_blocking(move || {
        inner.power.map_profile(&inner.db, &profile_id, &guid, ac_only)
    }).await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn get_unfinished_tuning_sessions(state: State<'_, AppState>) -> Result<Vec<TuningSession>, String> {
    database_work(state.inner.clone(), |conn| db::unfinished_sessions(conn)).await
}
#[tauri::command]
async fn activate_performance_profile(state: State<'_, AppState>, app: tauri::AppHandle, profile_id: String) -> Result<TuningSession, String> {
    let inner = state.inner.clone();
    let result = tauri::async_runtime::spawn_blocking(move || inner.power.activate(&inner.db,&profile_id)).await.map_err(|e| e.to_string())?;
    if let Ok(ref session) = result { let _ = app.emit("tuning:changed",session); }
    result
}
#[tauri::command]
async fn restore_previous_profile(state: State<'_, AppState>, app: tauri::AppHandle, session_id: String, force: bool) -> Result<TuningSession, String> {
    let inner = state.inner.clone();
    let result = tauri::async_runtime::spawn_blocking(move || inner.power.restore(&inner.db,&session_id,force)).await.map_err(|e| e.to_string())?;
    if let Ok(ref session) = result { let _ = app.emit("tuning:changed",session); }
    result
}

#[tauri::command]
async fn scan_cleanable_files(state: State<'_, AppState>, categories: Vec<String>) -> Result<CleanerScan, String> {
    let inner = state.inner.clone();
    tauri::async_runtime::spawn_blocking(move || cleaner::scan(&inner.db,&inner.cleaner_plans,&categories)).await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn execute_cleanup(state: State<'_, AppState>, app: tauri::AppHandle, plan_id: String, categories: Vec<String>) -> Result<CleaningResult, String> {
    let inner = state.inner.clone();
    if inner.cleanup_running.swap(true,Ordering::SeqCst) { return Err("Pembersihan lain sedang berlangsung".into()); }
    inner.cancel_cleanup.store(false,Ordering::SeqCst);
    let worker = inner.clone();
    let progress_app = app.clone();
    let guard = CleanupTaskGuard(inner.clone());
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        cleaner::execute(&worker.db,&worker.cleaner_plans,&worker.cancel_cleanup,&plan_id,&categories, |progress| { let _ = progress_app.emit("cleanup:progress", progress); })
    }).await;
    let result = result.map_err(|e| e.to_string())?;
    if let Ok(ref cleaning) = result { let _ = app.emit("cleanup:completed",cleaning); }
    result
}
#[tauri::command]
fn cancel_cleanup(state: State<'_, AppState>) { state.inner.cancel_cleanup.store(true,Ordering::SeqCst); }
#[tauri::command]
async fn get_cleaning_history(state: State<'_, AppState>) -> Result<Vec<CleaningResult>, String> {
    database_work(state.inner.clone(), |conn| db::cleaning_history(conn)).await
}
#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, String> { state.inner.settings.lock().map(|s| s.clone()).map_err(|_| lock_error()) }
#[tauri::command]
async fn update_settings(state: State<'_, AppState>, settings: AppSettings) -> Result<AppSettings, String> {
    settings.validate()?;
    let inner = state.inner.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Serialize durable preferences and their cache without blocking sampling.
        let _update = inner.settings_update.lock().map_err(|_| lock_error())?;
        let previous = inner.settings.lock().map_err(|_| lock_error())?.clone();
        if settings.gpu_adapter_id != previous.gpu_adapter_id {
            if let Some(id) = &settings.gpu_adapter_id {
                if !inner.sensors.inventory().devices.iter().any(|device| device.kind == "gpu" && &device.id == id) { return Err("Adapter GPU belum terdeteksi oleh provider sensor".into()); }
            }
        }
        { let conn = inner.db.lock().map_err(|_| lock_error())?; db::save_settings(&conn,&settings)?; }
        inner.history_writer.set_retention(settings.history_retention_hours);
        *inner.settings.lock().map_err(|_| lock_error())? = settings.clone();
        Ok(settings)
    }).await.map_err(|error| error.to_string())?
}
#[tauri::command]
async fn purge_monitoring_history(state: State<'_, AppState>) -> Result<usize, String> {
    let inner = state.inner.clone();
    let writer = inner.clone();
    database_work(inner, move |conn| { writer.history_writer.invalidate_pending(); db::purge_monitoring_history(conn) }).await
}
#[tauri::command]
fn get_monitoring_persistence(state: State<'_, AppState>) -> PersistenceStatus { state.inner.history_writer.status() }

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join("performance.db");
            let conn = db::open(&db_path).map_err(std::io::Error::other)?;
            let settings = db::load_settings(&conn);
            let info = hardware::initial_information();
            let capabilities = hardware::capabilities(&info,power::plans().map(|p| !p.is_empty()).unwrap_or(false));
            let initial = empty_snapshot(&info);
            let database = Arc::new(Mutex::new(conn));
            let persistence_app = app.handle().clone();
            let history_writer = persistence::HistoryWriter::start(database.clone(), settings.history_retention_hours, move |status| { let _ = persistence_app.emit("monitoring:persistence", status); });
            let sensor_app = app.handle().clone();
            let sensor_path = app.path().resource_dir()?.join("sensors").join("CorePulse.SensorHost.exe");
            #[cfg(debug_assertions)]
            let sensor_path = if sensor_path.is_file() { sensor_path } else { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../sidecar/publish/CorePulse.SensorHost.exe") };
            let sensors = sensors::Service::start(sensor_path, move |status| { let _ = sensor_app.emit("sensors:status", status); });
            let inner = Arc::new(Inner { db:database,info:Mutex::new(info),capabilities:Mutex::new(capabilities),snapshot:Mutex::new(initial),settings:Mutex::new(settings),settings_update:Mutex::new(()),detection:Mutex::new(()),processes:Mutex::new(processes::Collector::default()),sensors,history_writer,monitoring:AtomicBool::new(false),cleaner_plans:Mutex::new(HashMap::new()),cancel_cleanup:AtomicBool::new(false),cleanup_running:AtomicBool::new(false),power:power::Controller::default(),db_path });
            start_sampler(inner.clone(), app.handle().clone());
            start_power_watchdog(inner.clone(), app.handle().clone());
            app.manage(AppState { inner: inner.clone() });
            let discovery_app = app.handle().clone();
            thread::spawn(move || {
                if let Err(error) = refresh_device(&inner, &discovery_app) {
                    let _ = discovery_app.emit("app:error", serde_json::json!({"status":"error","timestamp":Utc::now().to_rfc3339(),"message":error}));
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_sensor_inventory,get_process_snapshot,get_system_information,detect_hardware,get_device_capabilities,get_hardware_snapshot,start_monitoring,stop_monitoring,get_hardware_history,get_analytics_report,get_power_plans,get_active_power_plan,get_performance_profiles,update_profile_mapping,get_unfinished_tuning_sessions,activate_performance_profile,restore_previous_profile,scan_cleanable_files,execute_cleanup,cancel_cleanup,get_cleaning_history,get_settings,update_settings,purge_monitoring_history,get_monitoring_persistence])
        .run(tauri::generate_context!())
        .expect("Core Pulse failed to start");
}
