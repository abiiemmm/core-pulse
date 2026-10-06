use crate::models::{ProcessEntry, ProcessSnapshot};
use chrono::Utc;
use std::{collections::HashSet, time::{Duration, Instant}};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

const MAX_ENTRIES: usize = 2048;
const INTERVAL: Duration = Duration::from_secs(2);

pub struct Collector {
    system: System,
    seen: HashSet<(u32, u64)>,
    previous: Option<(Instant, ProcessSnapshot)>,
}
impl Default for Collector {
    fn default() -> Self { Self { system: System::new(), seen: HashSet::new(), previous: None } }
}
impl Collector {
    pub fn sample(&mut self) -> ProcessSnapshot {
        if let Some((at, previous)) = &self.previous {
            if at.elapsed() < INTERVAL { return previous.clone(); }
            if at.elapsed() > Duration::from_secs(10) {
                self.system = System::new();
                self.seen.clear();
            }
        }
        self.system.refresh_cpu_usage();
        self.system.refresh_processes_specifics(ProcessesToUpdate::All, true,
            ProcessRefreshKind::nothing().with_cpu().with_memory().with_exe(UpdateKind::OnlyIfNotSet));
        let cores = self.system.cpus().len();
        let mut rows: Vec<ProcessEntry> = self.system.processes().iter().filter(|(pid, _)| pid.as_u32() != 0).map(|(pid, process)| {
            let identity = (pid.as_u32(), process.start_time());
            let executable = process.exe().map(|path| path.to_string_lossy().into_owned());
            let readable = executable.is_some();
            ProcessEntry {
                pid: identity.0, started_at: identity.1, name: process.name().to_string_lossy().into_owned(),
                cpu_percent: normalized_cpu(process.cpu_usage(), cores, readable && identity.1 > 0 && self.seen.contains(&identity)),
                // Windows returns default counters for inaccessible processes.
                // Only expose counters when we have a readable process identity.
                memory_bytes: (readable && process.memory() > 0).then_some(process.memory()), executable,
            }
        }).collect();
        self.seen = rows.iter().map(|row| (row.pid, row.started_at)).collect();
        let total_count = rows.len();
        rows.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes).then_with(|| a.pid.cmp(&b.pid)));
        rows.truncate(MAX_ENTRIES);
        let snapshot = ProcessSnapshot { recorded_at: Utc::now().to_rfc3339(), total_count, truncated: total_count > rows.len(), processes: rows };
        self.previous = Some((Instant::now(), snapshot.clone()));
        snapshot
    }
}
fn normalized_cpu(value: f32, cores: usize, valid: bool) -> Option<f64> {
    (valid && cores > 0 && value.is_finite() && value >= 0.0).then(|| (value as f64 / cores as f64).clamp(0.0, 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_values_distinguish_warmup_missing_and_measured_zero() {
        assert_eq!(normalized_cpu(400.0, 8, true), Some(50.0));
        assert_eq!(normalized_cpu(0.0, 8, true), Some(0.0));
        assert_eq!(normalized_cpu(0.0, 8, false), None);
        assert_eq!(normalized_cpu(f32::NAN, 8, true), None);
        assert_eq!(normalized_cpu(5.0, 0, true), None);
    }
    #[test]
    fn native_process_snapshot_finds_self_caches_and_measures_after_warmup() {
        let mut collector = Collector::default();
        let first = collector.sample();
        let own = first.processes.iter().find(|process| process.pid == std::process::id()).unwrap();
        assert!(own.executable.is_some());
        assert!(own.memory_bytes.unwrap() > 0);
        assert_eq!(own.cpu_percent, None);
        assert_eq!(collector.sample().recorded_at, first.recorded_at);
        std::thread::sleep(INTERVAL + Duration::from_millis(50));
        let next = collector.sample();
        assert_ne!(first.recorded_at, next.recorded_at);
        let cpu = next.processes.iter().find(|process| process.pid == std::process::id()).unwrap().cpu_percent.unwrap();
        assert!((0.0..=100.0).contains(&cpu));
        assert!(next.processes.len() <= MAX_ENTRIES);
    }
}
