use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize)]
pub struct ProcessEntry {
    pub pid: u32,
    pub started_at: u64,
    pub name: String,
    pub executable: Option<String>,
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
}
#[derive(Clone, Serialize)]
pub struct ProcessSnapshot {
    pub recorded_at: String,
    pub total_count: usize,
    pub truncated: bool,
    pub processes: Vec<ProcessEntry>,
}

#[derive(Clone, Serialize)]
pub struct Metric {
    pub value: Option<f64>,
    pub unit: String,
    pub source: String,
    pub status: String,
    pub recorded_at: String,
}
impl Metric {
    pub fn measured(value: f64, unit: &str, source: &str, at: &str) -> Self {
        Self { value: value.is_finite().then_some(value), unit: unit.into(), source: source.into(), status: if value.is_finite() { "supported" } else { "unknown" }.into(), recorded_at: at.into() }
    }
    pub fn unavailable(unit: &str, source: &str, at: &str) -> Self {
        Self { value: None, unit: unit.into(), source: source.into(), status: "unsupported".into(), recorded_at: at.into() }
    }
}

#[derive(Clone, Serialize)]
pub struct HardwareSnapshot {
    pub recorded_at: String,
    pub cpu_usage: Metric,
    pub cpu_temperature: Metric,
    pub gpu_usage: Metric,
    pub gpu_temperature: Metric,
    pub ram_usage: Metric,
    pub ram_used_gb: Metric,
    pub ram_total_gb: Metric,
    pub disk_used_gb: Metric,
    pub disk_total_gb: Metric,
    pub network_down_kbps: Metric,
    pub network_up_kbps: Metric,
    pub cpu_name: String,
    pub gpu_name: String,
    pub disk_name: String,
    pub power_source: String,
}

#[derive(Clone, Serialize)]
pub struct DeviceInfo {
    pub device_name: String,
    pub operating_system: String,
    pub cpu_model: String,
    pub cpu_cores: usize,
    pub ram_total_bytes: u64,
    pub gpu_adapters: Vec<String>,
    pub power_source: String,
}

#[derive(Clone, Serialize)]
pub struct Capability {
    pub capability_key: String,
    pub status: String,
    pub reason: String,
    pub detected_at: String,
}

#[derive(Clone, Serialize)]
pub struct PowerPlan {
    pub guid: String,
    pub name: String,
    pub active: bool,
}

#[derive(Clone, Serialize)]
pub struct PerformanceProfile {
    pub id: String,
    pub name: String,
    pub scheme_guid: Option<String>,
    pub ac_only: bool,
}

#[derive(Clone, Serialize)]
pub struct TuningSession {
    pub id: String,
    pub profile_id: String,
    pub previous_guid: String,
    pub applied_guid: String,
    pub status: String,
    pub started_at: String,
    pub error: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct CleanerScan {
    pub plan_id: String,
    pub expires_at: String,
    pub category: String,
    pub estimated_bytes: u64,
    pub eligible_count: usize,
    pub skipped_count: usize,
    pub warnings: Vec<String>,
}

#[derive(Clone, Serialize)]
pub struct PersistenceStatus {
    pub status: String,
    pub message: Option<String>,
    pub last_saved_at: Option<String>,
    pub dropped_samples: usize,
    pub updated_at: String,
}

#[derive(Clone, Serialize)]
pub struct CleanupProgress {
    pub plan_id: String,
    pub processed_count: usize,
    pub total_count: usize,
    pub deleted_count: usize,
    pub skipped_count: usize,
    pub error_count: usize,
    pub recovered_bytes: u64,
    pub status: String,
    pub timestamp: String,
}

#[derive(Clone, Serialize)]
pub struct CleaningResult {
    pub id: String,
    pub category: String,
    pub estimated_bytes: u64,
    pub recovered_bytes: u64,
    pub deleted_count: usize,
    pub skipped_count: usize,
    pub error_count: usize,
    pub finished_at: String,
    pub status: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub refresh_seconds: u8,
    pub theme: String,
    #[serde(default = "default_language")]
    pub language: String,
    pub history_retention_hours: u32,
    pub monitor_in_background: bool,
    #[serde(default)]
    pub gpu_adapter_id: Option<String>,
}
impl Default for AppSettings {
    fn default() -> Self {
        Self { refresh_seconds: 1, theme: "dark".into(), language: default_language(), history_retention_hours: 24, monitor_in_background: false, gpu_adapter_id: None }
    }
}
impl AppSettings {
    pub fn validate(&self) -> Result<(), String> {
        if ![1, 2, 5].contains(&self.refresh_seconds) { return Err("Refresh interval harus 1, 2, atau 5 detik".into()); }
        if !["dark", "light", "system"].contains(&self.theme.as_str()) { return Err("Tema tidak dikenal".into()); }
        if !["id", "en", "es"].contains(&self.language.as_str()) { return Err("Bahasa tidak dikenal".into()); }
        if ![6, 24, 72].contains(&self.history_retention_hours) { return Err("Retensi tidak didukung".into()); }
        if self.gpu_adapter_id.as_ref().is_some_and(|id| !crate::sensors::valid_adapter_id(id)) { return Err("Identitas adapter GPU tidak valid".into()); }
        Ok(())
    }
}

fn default_language() -> String { "id".into() }

#[derive(Clone, Serialize)]
pub struct AnalyticsPoint {
    pub sensor_key: String,
    pub recorded_at: String,
    pub average: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub sample_count: u64,
}

#[derive(Clone, Serialize)]
pub struct AnalyticsReport {
    pub from: String,
    pub to: String,
    pub bucket_seconds: u32,
    pub points: Vec<AnalyticsPoint>,
}

#[derive(Clone, Serialize)]
pub struct HistoryPoint {
    pub recorded_at: String,
    pub value: f64,
}
