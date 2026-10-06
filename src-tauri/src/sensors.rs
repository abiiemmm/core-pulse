//! A narrow, versioned read-only sensor protocol. Never exposed as a command runner.
use crate::{models::{HardwareSnapshot, Metric}, windows_command::{Job, CREATE_NO_WINDOW}};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, io::{BufRead, BufReader, Read, Write}, os::windows::process::CommandExt, path::PathBuf, process::{Child, ChildStdin, Command, Stdio}, sync::{atomic::{AtomicBool, AtomicU32, Ordering}, mpsc::{self, Receiver}, Arc, Mutex}, thread, time::{Duration, Instant}};

pub const PROVIDER: &str = "LibreHardwareMonitor 0.9.6";
const MAX_PACKET: usize = 262144;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Packet { protocol_version: u8, #[serde(rename="type")] kind: String, sequence: u64, recorded_at: String, provider: String, elevated: bool, devices: Vec<Device> }
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Device { id: String, name: String, kind: String, status: String, readings: Vec<Reading> }
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reading { id: String, name: String, kind: String, unit: String, value: Option<f64> }

#[derive(Clone, Serialize, PartialEq)]
pub struct Adapter { pub id: String, pub name: String, pub kind: String, pub temperature_available: bool, pub load_available: bool }
#[derive(Clone, Serialize)]
pub struct Inventory { pub status: String, pub message: String, pub provider: String, pub elevated: bool, pub restart_count: u32, pub updated_at: String, pub devices: Vec<Adapter> }
struct Cache { inventory: Inventory, packet: Option<Packet>, received_at: Option<Instant> }
struct Shared { enabled: AtomicBool, shutdown: AtomicBool, interval: AtomicU32, cache: Mutex<Cache> }
pub struct Service(Arc<Shared>);

pub fn valid_adapter_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.starts_with('/') && !id.contains("..") && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.'))
}
fn valid_text(value: &str, limit: usize) -> bool { !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control) }
fn decode(line: &[u8], previous_sequence: u64) -> Result<Packet, String> {
    if line.len() > MAX_PACKET { return Err("Ukuran data provider sensor melampaui batas".into()); }
    let packet: Packet = serde_json::from_slice(line).map_err(|_| "Data provider sensor tidak valid")?;
    if packet.protocol_version != 1 || packet.kind != "snapshot" || packet.provider != PROVIDER || packet.sequence <= previous_sequence { return Err("Versi atau urutan data provider sensor tidak valid".into()); }
    let recorded = DateTime::parse_from_rfc3339(&packet.recorded_at).map_err(|_| "Waktu provider sensor tidak valid")?;
    if (Utc::now() - recorded.with_timezone(&Utc)).num_seconds().abs() > 30 { return Err("Waktu provider sensor terlalu jauh dari waktu aplikasi".into()); }
    if packet.devices.len() > 32 { return Err("Jumlah perangkat sensor melampaui batas".into()); }
    let mut ids = HashSet::new();
    for device in &packet.devices {
        if !valid_adapter_id(&device.id) || !valid_text(&device.name, 256) || !["cpu", "gpu"].contains(&device.kind.as_str()) || !["ready", "error"].contains(&device.status.as_str()) || !ids.insert(device.id.clone()) || device.readings.len() > 128 { return Err("Identitas perangkat sensor tidak valid".into()); }
        for reading in &device.readings {
            if !valid_adapter_id(&reading.id) || !reading.id.starts_with(&format!("{}/", device.id)) || !valid_text(&reading.name, 256) || !ids.insert(reading.id.clone()) { return Err("Identitas pembacaan sensor tidak valid".into()); }
            let max = match (reading.kind.as_str(), reading.unit.as_str()) { ("temperature", "°C") => 150.0, ("load", "%") => 100.0, _ => return Err("Jenis atau unit sensor tidak valid".into()) };
            if reading.value.is_some_and(|value| !value.is_finite() || value < 0.0 || value > max) { return Err("Nilai sensor di luar batas pembacaan".into()); }
            if device.status == "error" && reading.value.is_some() { return Err("Provider mengirim nilai dari perangkat yang gagal dibaca".into()); }
        }
    }
    Ok(packet)
}
fn read_frame(reader: &mut impl BufRead) -> Result<Vec<u8>, String> {
    let mut result = Vec::new();
    loop {
        let chunk = reader.fill_buf().map_err(|_| "Provider sensor terputus")?;
        if chunk.is_empty() { return Err("Provider sensor berhenti".into()); }
        let end = chunk.iter().position(|byte| *byte == b'\n');
        let count = end.map(|at| at + 1).unwrap_or(chunk.len());
        if result.len() + count > MAX_PACKET { return Err("Ukuran data provider sensor melampaui batas".into()); }
        result.extend_from_slice(&chunk[..count]);
        reader.consume(count);
        if end.is_some() { return Ok(result); }
    }
}
struct Host {
    child: Child, input: ChildStdin, job: Option<Job>, responses: Option<Receiver<Result<Vec<u8>, String>>>, readers: Vec<thread::JoinHandle<()>>, sequence: u64,
}
impl Host {
    fn start(program: &PathBuf) -> Result<Self, String> {
        Self::start_with_args(program, &["--stdio"])
    }
    fn start_with_args(program: &PathBuf, args: &[&str]) -> Result<Self, String> {
        if !program.is_absolute() || !program.is_file() { return Err("Host sensor belum tersedia dalam instalasi ini".into()); }
        let job = Job::new().map_err(|_| "Host sensor tidak dapat diisolasi")?;
        let mut child = Command::new(program).args(args).current_dir(program.parent().ok_or("Lokasi host sensor tidak valid")?)
            .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1").creation_flags(CREATE_NO_WINDOW).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|_| "Host sensor gagal dijalankan")?;
        if job.assign(&child).is_err() { let _ = child.kill(); let _ = child.wait(); return Err("Host sensor tidak dapat diisolasi".into()); }
        let input = child.stdin.take().ok_or("Input host sensor tidak tersedia")?;
        let stdout = child.stdout.take().ok_or("Output host sensor tidak tersedia")?;
        let stderr = child.stderr.take().ok_or("Log host sensor tidak tersedia")?;
        let (sender, responses) = mpsc::sync_channel(1);
        let output = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let frame = read_frame(&mut reader);
                let failed = frame.is_err();
                if sender.send(frame).is_err() || failed { break; }
            }
        });
        let errors = thread::spawn(move || { let _ = std::io::copy(&mut stderr.take(65536), &mut std::io::sink()); });
        Ok(Self { child, input, job: Some(job), responses: Some(responses), readers: vec![output, errors], sequence: 0 })
    }
    fn sample(&mut self) -> Result<Packet, String> {
        self.sample_with_timeout(Duration::from_secs(15))
    }
    fn sample_with_timeout(&mut self, timeout: Duration) -> Result<Packet, String> {
        let requested_at = Utc::now();
        self.input.write_all(b"sample\n").and_then(|_| self.input.flush()).map_err(|_| "Provider sensor terputus")?;
        let frame = self.responses.as_ref().ok_or("Provider sensor terputus")?.recv_timeout(timeout).map_err(|_| "Provider sensor tidak merespons dalam batas waktu")??;
        let packet = decode(&frame, self.sequence)?;
        let recorded = DateTime::parse_from_rfc3339(&packet.recorded_at).map_err(|_| "Waktu provider sensor tidak valid")?.with_timezone(&Utc);
        if recorded < requested_at - chrono::Duration::seconds(1) { return Err("Provider sensor mengirim sampel lama".into()); }
        self.sequence = packet.sequence;
        Ok(packet)
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.responses.take(); // Release any reader waiting to publish a frame.
        self.job.take(); // Kill the entire job before joining blocking readers.
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) { let _ = reader.join(); }
    }
}
fn retry_delay(failures: u32) -> Duration { Duration::from_secs([1, 2, 5, 10, 30, 60][failures.saturating_sub(1).min(5) as usize]) }
impl Service {
    pub fn start(program: PathBuf, callback: impl Fn(Inventory) + Send + 'static) -> Self {
        let inventory = Inventory { status: "waiting".into(), message: "Menunggu provider sensor".into(), provider: PROVIDER.into(), elevated: false, restart_count: 0, updated_at: Utc::now().to_rfc3339(), devices: vec![] };
        let shared = Arc::new(Shared { enabled: AtomicBool::new(false), shutdown: AtomicBool::new(false), interval: AtomicU32::new(1), cache: Mutex::new(Cache { inventory, packet: None, received_at: None }) });
        let worker = shared.clone();
        thread::spawn(move || {
            let mut host: Option<Host> = None;
            let mut next = Instant::now();
            let mut failures = 0_u32;
            let mut successes = 0_u32;
            let mut last_active = Instant::now();
            loop {
                thread::sleep(Duration::from_millis(100));
                if worker.shutdown.load(Ordering::Relaxed) { break; }
                if !worker.enabled.load(Ordering::Relaxed) {
                    if last_active.elapsed() > Duration::from_secs(10) && host.is_some() { host.take(); }
                    continue;
                }
                last_active = Instant::now();
                if Instant::now() < next { continue; }
                let result = (|| {
                    if host.is_none() { host = Some(Host::start(&program)?); }
                    host.as_mut().ok_or("Host sensor tidak tersedia")?.sample()
                })();
                match result {
                    Ok(packet) => {
                        successes = successes.saturating_add(1);
                        if successes >= 3 { failures = 0; }
                        let devices = packet.devices.iter().map(|device| Adapter { id: device.id.clone(), name: device.name.clone(), kind: device.kind.clone(), temperature_available: reading_metric(Some(device), "temperature", true, "now").value.is_some(), load_available: reading_metric(Some(device), "load", true, "now").value.is_some() }).collect();
                        let changed = {
                            let mut cache = worker.cache.lock().unwrap_or_else(|error| error.into_inner());
                            let changed = cache.inventory.status != "ready" || cache.inventory.devices != devices;
                            cache.inventory = Inventory { status: "ready".into(), message: "Provider sensor aktif".into(), provider: PROVIDER.into(), elevated: packet.elevated, restart_count: cache.inventory.restart_count, updated_at: Utc::now().to_rfc3339(), devices };
                            cache.packet = Some(packet); cache.received_at = Some(Instant::now());
                            changed.then(|| cache.inventory.clone())
                        };
                        if let Some(inventory) = changed { callback(inventory); }
                        next = Instant::now() + Duration::from_secs(worker.interval.load(Ordering::Relaxed).clamp(1, 5) as u64);
                    }
                    Err(message) => {
                        host.take(); successes = 0; failures = failures.saturating_add(1);
                        let status = {
                            let mut cache = worker.cache.lock().unwrap_or_else(|error| error.into_inner());
                            cache.received_at = None;
                            cache.inventory.status = "degraded".into(); cache.inventory.message = message; cache.inventory.restart_count = cache.inventory.restart_count.saturating_add(1); cache.inventory.updated_at = Utc::now().to_rfc3339();
                            cache.inventory.clone()
                        };
                        callback(status);
                        next = Instant::now() + retry_delay(failures);
                    }
                }
            }
        });
        Self(shared)
    }
    pub fn configure(&self, enabled: bool, interval: u8) { self.0.enabled.store(enabled, Ordering::Relaxed); self.0.interval.store(interval.clamp(1, 5) as u32, Ordering::Relaxed); }
    pub fn inventory(&self) -> Inventory { self.0.cache.lock().unwrap_or_else(|error| error.into_inner()).inventory.clone() }
    pub fn selected_gpu_source(&self, selected: Option<&str>) -> Option<String> {
        let cache = self.0.cache.lock().unwrap_or_else(|error| error.into_inner());
        selected.map(str::to_string).or_else(|| cache.packet.as_ref().and_then(|packet| choose_gpu(packet, None)).map(|device| device.id.clone())).map(|id| format!("{PROVIDER} · {id}"))
    }
    pub fn merge(&self, sample: &mut HardwareSnapshot, selected: Option<&str>, interval: u8) {
        let cache = self.0.cache.lock().unwrap_or_else(|error| error.into_inner());
        let usable = cache.inventory.status == "ready" && cache.received_at.is_some_and(|at| at.elapsed() <= Duration::from_secs((interval as u64 * 3).max(5)));
        let packet = cache.packet.as_ref();
        let cpu = packet.and_then(|packet| packet.devices.iter().find(|device| device.kind == "cpu"));
        let gpu = packet.and_then(|packet| choose_gpu(packet, selected));
        let at = packet.map(|packet| packet.recorded_at.clone()).unwrap_or_else(|| sample.recorded_at.clone());
        sample.cpu_temperature = reading_metric(cpu, "temperature", usable, &at);
        sample.gpu_temperature = reading_metric(gpu, "temperature", usable, &at);
        sample.gpu_usage = reading_metric(gpu, "load", usable, &at);
        sample.gpu_name = gpu.map(|gpu| gpu.name.clone()).unwrap_or_else(|| "GPU unavailable".into());
    }
}
impl Drop for Service { fn drop(&mut self) { self.0.shutdown.store(true, Ordering::Relaxed); } }
fn choose_gpu<'a>(packet: &'a Packet, selected: Option<&str>) -> Option<&'a Device> {
    let devices = packet.devices.iter().filter(|device| device.kind == "gpu");
    if let Some(id) = selected { return devices.into_iter().find(|device| device.id == id); }
    // Prefer NVIDIA on hybrid laptops; the user can explicitly choose any GPU.
    devices.min_by_key(|device| (if device.id.starts_with("/gpu-nvidia/") { 0 } else if device.id.starts_with("/gpu-amd/") { 1 } else { 2 }, &device.id))
}
fn reading_metric(device: Option<&Device>, kind: &str, usable: bool, at: &str) -> Metric {
    let unit = if kind == "temperature" { "°C" } else { "%" };
    let source = device.map(|device| format!("{PROVIDER} · {}", device.id)).unwrap_or_else(|| PROVIDER.into());
    let measured = device.filter(|device| usable && device.status == "ready").and_then(|device| {
        let readings = device.readings.iter().filter(|reading| reading.kind == kind && reading.value.is_some());
        if kind == "load" {
            // Core load is a GPU utilization reading. Memory-controller/video/
            // bus load are never substituted for whole-GPU core utilization.
            readings.filter(|reading| matches!(reading.name.as_str(), "GPU Core" | "GPU D3D 3D" | "D3D 3D")).min_by_key(|reading| if reading.name == "GPU Core" { 0 } else { 1 }).and_then(|reading| reading.value)
        } else {
            let mut values: Vec<&Reading> = readings.collect();
            values.sort_by_key(|reading| match reading.name.as_str() { "CPU Package" | "CPU (Tctl/Tdie)" | "CPU (Tdie)" | "GPU Core" => 0, _ => 1 });
            // Fallback is the hottest reported sensor; avoid arbitrary ordering.
            let rank = values.first().map(|reading| matches!(reading.name.as_str(), "CPU Package" | "CPU (Tctl/Tdie)" | "CPU (Tdie)" | "GPU Core"));
            if rank == Some(true) { values[0].value } else { values.iter().filter_map(|reading| reading.value).max_by(f64::total_cmp) }
        }
    });
    measured.map(|value| Metric::measured(value, unit, &source, at)).unwrap_or_else(|| Metric { value: None, unit: unit.into(), source, status: if usable { "unsupported" } else { "unknown" }.into(), recorded_at: at.into() })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(value: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"protocol_version":1,"type":"snapshot","sequence":1,"recorded_at":Utc::now().to_rfc3339(),"provider":PROVIDER,"elevated":false,"devices":[{"id":"/nvidiagpu/0","name":"Fixture GPU","kind":"gpu","status":"ready","readings":[{"id":"/nvidiagpu/0/load/0","name":"GPU Core","kind":"load","unit":"%","value":value}]}]})).unwrap()
    }
    #[test]
    fn valid_zero_and_missing_sensor_are_distinct() {
        for (value, expected) in [(serde_json::json!(0), Some(0.0)), (serde_json::Value::Null, None)] {
            let parsed = decode(&packet(value), 0).unwrap();
            assert_eq!(reading_metric(parsed.devices.first(), "load", true, "now").value, expected);
            assert_eq!(reading_metric(parsed.devices.first(), "load", false, "now").value, None);
        }
    }
    #[test]
    fn wrong_version_sequence_timestamp_duplicate_and_out_of_range_are_rejected() {
        let base: serde_json::Value = serde_json::from_slice(&packet(serde_json::json!(0))).unwrap();
        for mutation in ["version", "time", "duplicate", "unit", "range", "error", "identity"] {
            let mut fixture = base.clone();
            match mutation {
                "version" => fixture["protocol_version"] = 2.into(),
                "time" => fixture["recorded_at"] = "2000-01-01T00:00:00Z".into(),
                "duplicate" => { let device = fixture["devices"][0].clone(); fixture["devices"].as_array_mut().unwrap().push(device); },
                "unit" => fixture["devices"][0]["readings"][0]["unit"] = "GHz".into(),
                "range" => fixture["devices"][0]["readings"][0]["value"] = 101.into(),
                "error" => fixture["devices"][0]["status"] = "error".into(),
                _ => fixture["devices"][0]["readings"][0]["id"] = "/othergpu/0/load/0".into(),
            }
            assert!(decode(&serde_json::to_vec(&fixture).unwrap(), 0).is_err(), "{mutation}");
        }
        assert!(decode(&serde_json::to_vec(&base).unwrap(), 1).is_err());
    }
    #[test]
    fn malformed_and_oversize_streams_have_bounded_frames() {
        assert!(read_frame(&mut std::io::Cursor::new(vec![b'a'; MAX_PACKET + 1])).is_err());
        assert!(read_frame(&mut std::io::Cursor::new(b"unfinished")).is_err());
        assert_eq!(read_frame(&mut std::io::Cursor::new(b"one\ntwo\n")).unwrap(), b"one\n");
    }
    #[test]
    fn memory_load_is_not_reported_as_gpu_core_usage_and_missing_selection_does_not_fallback() {
        let mut parsed = decode(&packet(serde_json::json!(42)), 0).unwrap();
        parsed.devices[0].readings[0].name = "GPU Memory Controller".into();
        assert_eq!(reading_metric(parsed.devices.first(), "load", true, "now").value, None);
        assert!(choose_gpu(&parsed, Some("/nvidiagpu/1")).is_none());
        assert_eq!(choose_gpu(&parsed, None).unwrap().id, "/nvidiagpu/0");
    }
    #[test]
    fn retries_are_bounded_even_after_long_failure_runs() {
        assert_eq!(retry_delay(1), Duration::from_secs(1));
        assert_eq!(retry_delay(4), Duration::from_secs(10));
        assert_eq!(retry_delay(u32::MAX), Duration::from_secs(60));
        assert!(!valid_adapter_id("/gpu/../other"));
    }
    #[test]
    fn hybrid_gpu_default_uses_nvidia_and_explicit_selection_uses_exact_identity() {
        let mut parsed = decode(&packet(serde_json::json!(42)), 0).unwrap();
        parsed.devices[0].id = "/gpu-nvidia/0".into();
        let mut integrated = parsed.devices[0].clone();
        integrated.id = "/gpu-amd/0".into();
        parsed.devices.insert(0, integrated);
        assert_eq!(choose_gpu(&parsed, None).unwrap().id, "/gpu-nvidia/0");
        assert_eq!(choose_gpu(&parsed, Some("/gpu-amd/0")).unwrap().id, "/gpu-amd/0");
        assert!(choose_gpu(&parsed, Some("/gpu-nvidia/9")).is_none());
    }
    fn fixture_host(script: &str) -> Host {
        let powershell = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        Host::start_with_args(&powershell, &["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script]).unwrap()
    }
    #[test]
    fn provider_crash_malformed_output_and_hang_are_bounded_and_killed() {
        for script in ["exit 9", "[Console]::ReadLine() | Out-Null; [Console]::WriteLine('bad json')", "[Console]::ReadLine() | Out-Null; Start-Sleep -Seconds 60"] {
            let mut host = fixture_host(script);
            assert!(host.sample_with_timeout(Duration::from_secs(2)).is_err());
            let began = Instant::now();
            host.job.take();
            let _ = host.child.wait().unwrap();
            drop(host); // Both pipe readers must join after termination.
            assert!(began.elapsed() < Duration::from_secs(3));
        }
    }
    #[test]
    fn missing_provider_recovers_with_bounded_retries_without_blocking_cache_reads() {
        let service = Service::start(PathBuf::from("C:/corepulse-missing-sensor-fixture.exe"), |_| {});
        service.configure(true, 1);
        let began = Instant::now();
        while service.inventory().restart_count < 2 && began.elapsed() < Duration::from_secs(3) { thread::sleep(Duration::from_millis(20)); }
        let inventory = service.inventory();
        assert_eq!(inventory.status, "degraded");
        assert_eq!(inventory.restart_count, 2);
        assert!(inventory.devices.is_empty());
        service.configure(false, 1);
        thread::sleep(Duration::from_millis(300));
        assert_eq!(service.inventory().restart_count, 2);
    }
}
