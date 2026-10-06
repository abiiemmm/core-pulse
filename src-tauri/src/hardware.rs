use crate::models::{Capability, DeviceInfo, HardwareSnapshot, Metric};
use chrono::Utc;
use std::time::Instant;
use sysinfo::{Disks, Networks, System};

#[repr(C)]
struct SystemPowerStatus {
    ac_line_status: u8,
    battery_flag: u8,
    battery_life_percent: u8,
    system_status_flag: u8,
    battery_life_time: u32,
    battery_full_life_time: u32,
}
#[link(name = "kernel32")]
extern "system" { fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32; }

pub fn power_source() -> String {
    let mut status = SystemPowerStatus { ac_line_status: 255, battery_flag: 255, battery_life_percent: 255, system_status_flag: 0, battery_life_time: 0, battery_full_life_time: 0 };
    let result = unsafe { GetSystemPowerStatus(&mut status) };
    if result == 0 { "Unknown".into() } else if status.ac_line_status == 1 { "AC power".into() } else if status.ac_line_status == 0 { "Battery".into() } else { "Unknown".into() }
}

fn gpu_adapters() -> Vec<String> {
    // Fixed program and fixed script. No input from the webview is interpolated.
    let output = crate::windows_command::powershell("Import-Module (Join-Path $PSHOME 'Modules/CimCmdlets/CimCmdlets.psd1') -ErrorAction Stop; Get-CimInstance Win32_VideoController -ErrorAction Stop | Select-Object -ExpandProperty Name");
    output.map(|value| value.lines().map(str::trim).filter(|name| !name.is_empty()).map(str::to_string).collect()).unwrap_or_default()
}

pub fn initial_information() -> DeviceInfo {
    let mut system = System::new();
    system.refresh_cpu_all();
    system.refresh_memory();
    DeviceInfo {
        device_name: System::host_name().unwrap_or_else(|| "Windows PC".into()),
        operating_system: System::long_os_version().unwrap_or_else(|| "Windows".into()),
        cpu_model: system.cpus().first().map(|c| c.brand().to_string()).unwrap_or_else(|| "Unknown CPU".into()),
        cpu_cores: system.cpus().len(),
        ram_total_bytes: system.total_memory(),
        gpu_adapters: vec![],
        power_source: power_source(),
    }
}

pub fn discover() -> DeviceInfo {
    DeviceInfo { gpu_adapters: gpu_adapters(), ..initial_information() }
}

pub fn capabilities(info: &DeviceInfo, power_supported: bool) -> Vec<Capability> {
    let at = Utc::now().to_rfc3339();
    let add = |key: &str, status: &str, reason: &str| Capability { capability_key: key.into(), status: status.into(), reason: reason.into(), detected_at: at.clone() };
    vec![
        add("cpu_usage", "supported", "Dibaca dari Windows melalui sysinfo"),
        add("ram_usage", "supported", "Dibaca dari Windows melalui sysinfo"),
        add("disk_capacity", "supported", "Volume yang tersedia dibaca dari Windows"),
        add("network_rate", "supported", "Laju adapter dihitung dari delta byte"),
        add("cpu_temperature", "unsupported", "Provider sensor suhu belum tersedia pada build ini"),
        add("gpu_usage", "unsupported", "Provider sensor GPU belum tersedia pada build ini"),
        add("gpu_temperature", "unsupported", "Provider sensor suhu GPU belum tersedia pada build ini"),
        add("gpu_adapters", if info.gpu_adapters.is_empty() { "unknown" } else { "supported" }, if info.gpu_adapters.is_empty() { "Adapter GPU tidak terdeteksi" } else { "Identitas adapter terdeteksi; sensor terpisah memerlukan provider" }),
        add("power_plans", if power_supported { "supported" } else { "unsupported" }, if power_supported { "Skema daya tersedia melalui API Windows" } else { "Skema daya Windows belum dapat dibaca" }),
        add("user_temp_cleaner", "supported", "Hanya direktori temp akun saat ini"),
    ]
}

pub struct Collector {
    system: System,
    disks: Disks,
    networks: Networks,
    last_network: Option<Instant>,
}
impl Collector {
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_all();
        system.refresh_memory();
        Self { system, disks: Disks::new_with_refreshed_list(), networks: Networks::new_with_refreshed_list(), last_network: None }
    }
    pub fn sample(&mut self, info: &DeviceInfo) -> HardwareSnapshot {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.disks.refresh(true);
        self.networks.refresh(true);
        let at = Utc::now().to_rfc3339();
        let source = "Windows / sysinfo";
        let gb = 1024_f64.powi(3);
        let ram_total = self.system.total_memory() as f64 / gb;
        let ram_used = self.system.used_memory() as f64 / gb;
        let disk = self.disks.list().iter().find(|d| d.mount_point().to_string_lossy().eq_ignore_ascii_case(&std::env::var("SystemDrive").map(|d| format!("{}\\", d)).unwrap_or_else(|_| "C:\\".into()))).or_else(|| self.disks.list().first());
        let disk_total = disk.map(|d| d.total_space() as f64 / gb);
        let disk_used = disk.map(|d| (d.total_space().saturating_sub(d.available_space())) as f64 / gb);
        let elapsed = self.last_network.replace(Instant::now()).map(|instant| instant.elapsed().as_secs_f64()).filter(|s| *s > 0.05);
        let received: u64 = self.networks.list().values().map(|n| n.received()).sum();
        let transmitted: u64 = self.networks.list().values().map(|n| n.transmitted()).sum();
        let measured = |value: Option<f64>, unit: &str| value.filter(|v| v.is_finite()).map(|v| Metric::measured(v,unit,source,&at)).unwrap_or_else(|| Metric::unavailable(unit,source,&at));
        HardwareSnapshot {
            recorded_at: at.clone(),
            cpu_usage: measured(Some(self.system.global_cpu_usage() as f64), "%"),
            cpu_temperature: Metric::unavailable("°C", "Sensor provider", &at),
            gpu_usage: Metric::unavailable("%", "Sensor provider", &at),
            gpu_temperature: Metric::unavailable("°C", "Sensor provider", &at),
            ram_usage: measured((ram_total > 0.0).then_some(ram_used / ram_total * 100.0), "%"),
            ram_used_gb: measured(Some(ram_used), "GB"),
            ram_total_gb: measured(Some(ram_total), "GB"),
            disk_used_gb: measured(disk_used, "GB"),
            disk_total_gb: measured(disk_total, "GB"),
            network_down_kbps: measured(elapsed.map(|s| received as f64 / 1024.0 / s), "KB/s"),
            network_up_kbps: measured(elapsed.map(|s| transmitted as f64 / 1024.0 / s), "KB/s"),
            cpu_name: info.cpu_model.clone(),
            gpu_name: info.gpu_adapters.first().cloned().unwrap_or_else(|| "GPU unavailable".into()),
            disk_name: disk.map(|d| d.name().to_string_lossy().to_string()).unwrap_or_else(|| "Storage unavailable".into()),
            power_source: power_source(),
        }
    }
}
