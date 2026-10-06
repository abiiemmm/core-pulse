mod windows;

use crate::{db, models::{CleanerScan, CleaningResult, CleanupProgress}};
use chrono::{Duration, Utc};
use rusqlite::{params, Connection};
use std::{collections::HashMap, fs, path::{Path, PathBuf}, sync::{atomic::{AtomicBool, Ordering}, Mutex}, time::{Instant, SystemTime}};
use uuid::Uuid;

const MAX_ENTRIES: usize = 20_000;
const MIN_AGE_SECONDS: u64 = 24 * 60 * 60;
const PLAN_LIFETIME_SECONDS: u64 = 5 * 60;
#[cfg(windows)] const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

#[derive(Clone)]
pub struct PlannedFile { pub path: PathBuf, pub canonical: PathBuf, pub len: u64, pub modified: SystemTime, identity: windows::Identity }
pub struct Plan { pub root: PathBuf, root_identity: windows::Identity, pub expires: Instant, pub files: Vec<PlannedFile>, pub estimated: u64, pub skipped: usize }

fn is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)] { use std::os::windows::fs::MetadataExt; metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 }
    #[cfg(not(windows))] { metadata.file_type().is_symlink() }
}
fn allowed(root: &Path, candidate: &Path) -> bool { candidate.starts_with(root) && candidate != root }
fn root() -> Result<PathBuf, String> { windows::user_temp().map_err(|_| "Folder temp pengguna tidak dapat diakses dengan aman".into()) }

pub fn scan(database: &Mutex<Connection>, plans: &Mutex<HashMap<String, Plan>>, categories: &[String]) -> Result<CleanerScan, String> {
    if categories.len() != 1 || categories[0] != "user_temp" { return Err("Kategori pembersihan tidak didukung".into()); }
    let root = root()?;
    scan_at(database, plans, root)
}

// The production command never accepts a root from the UI. Tests use private fixtures.
fn scan_at(database: &Mutex<Connection>, plans: &Mutex<HashMap<String, Plan>>, root: PathBuf) -> Result<CleanerScan, String> {
    let root_guards = windows::pin_root(&root).map_err(|_| "Folder temp tidak aman")?;
    let root_identity = root_guards.last().ok_or("Folder temp tidak aman")?.identity().map_err(|_| "Identitas folder temp tidak tersedia")?;
    let cutoff = SystemTime::now() - std::time::Duration::from_secs(MIN_AGE_SECONDS);
    let mut stack = vec![root.clone()];
    let mut files = Vec::new();
    let mut skipped = 0_usize;
    let mut seen = 0_usize;
    let mut truncated = false;
    'scan: while let Some(dir) = stack.pop() {
        // Pin the directory chain while enumerating: reparse points cannot redirect
        // traversal after the entry was checked by its parent.
        let _parents = if dir == root { Vec::new() } else {
            match windows::pin_parents(&root, &dir.join("scan-entry")) { Ok(guards) => guards, Err(_) => { skipped += 1; continue; } }
        };
        let entries = match fs::read_dir(&dir) { Ok(entries) => entries, Err(_) => { skipped += 1; continue; } };
        for item in entries {
            if seen >= MAX_ENTRIES { truncated = true; break 'scan; }
            seen += 1;
            let item = match item { Ok(item) => item, Err(_) => { skipped += 1; continue; } };
            let path = item.path();
            let meta = match fs::symlink_metadata(&path) { Ok(meta) => meta, Err(_) => { skipped += 1; continue; } };
            if is_reparse(&meta) || meta.file_type().is_symlink() { skipped += 1; continue; }
            let canonical = match fs::canonicalize(&path) { Ok(path) if allowed(&root, &path) => path, _ => { skipped += 1; continue; } };
            if meta.is_dir() { stack.push(path); continue; }
            if !meta.is_file() { skipped += 1; continue; }
            let modified = match meta.modified() { Ok(time) if time < cutoff => time, _ => { skipped += 1; continue; } };
            let handle = match windows::open_candidate(&path, false) { Ok(handle) => handle, Err(_) => { skipped += 1; continue; } };
            let current = match handle.metadata() { Ok(current) => current, Err(_) => { skipped += 1; continue; } };
            let identity = match windows::identity(&handle) { Ok(identity) => identity, Err(_) => { skipped += 1; continue; } };
            if !current.is_file() || is_reparse(&current) || current.len() != meta.len() || current.modified().ok() != Some(modified)
                || windows::final_path(&handle).ok().as_ref() != Some(&canonical) || windows::single_link(&handle).ok() != Some(true) {
                skipped += 1; continue;
            }
            files.push(PlannedFile { path, canonical, len: meta.len(), modified, identity });
        }
    }
    let estimated = files.iter().fold(0_u64, |sum, file| sum.saturating_add(file.len));
    let id = Uuid::new_v4().to_string();
    let created = Utc::now();
    let expires_at = (created + Duration::seconds(PLAN_LIFETIME_SECONDS as i64)).to_rfc3339();
    let warning = if truncated { vec!["Batas 20.000 entri tercapai; pindai ulang setelah membersihkan sebagian file.".into()] } else { Vec::new() };
    database.lock().map_err(|_| "Database tidak tersedia")?.execute("INSERT INTO cleaning_scans(id,category_snapshot,estimated_bytes,eligible_count,status,created_at,expires_at) VALUES(?1,'user_temp',?2,?3,'ready',?4,?5)", params![id,estimated as i64,files.len() as i64,created.to_rfc3339(),expires_at]).map_err(|e| e.to_string())?;
    let eligible_count = files.len();
    let mut plans = plans.lock().map_err(|_| "Scan state tidak tersedia")?;
    plans.retain(|_, plan| plan.expires > Instant::now());
    if plans.len() >= 8 {
        if let Some(oldest) = plans.iter().min_by_key(|(_, plan)| plan.expires).map(|(id, _)| id.clone()) { plans.remove(&oldest); }
    }
    plans.insert(id.clone(), Plan { root, root_identity, expires: Instant::now() + std::time::Duration::from_secs(PLAN_LIFETIME_SECONDS), files, estimated, skipped });
    Ok(CleanerScan { plan_id: id, expires_at, category: "user_temp".into(), estimated_bytes: estimated, eligible_count, skipped_count: skipped, warnings: warning })
}

pub fn execute(database: &Mutex<Connection>, plans: &Mutex<HashMap<String, Plan>>, cancel: &AtomicBool, plan_id: &str, categories: &[String], progress: impl FnMut(CleanupProgress)) -> Result<CleaningResult, String> {
    if Uuid::parse_str(plan_id).is_err() || categories.len() != 1 || categories[0] != "user_temp" { return Err("Rencana pembersihan tidak valid".into()); }
    execute_at(database, plans, cancel, plan_id, &root()?, progress)
}

fn execute_at(database: &Mutex<Connection>, plans: &Mutex<HashMap<String, Plan>>, cancel: &AtomicBool, plan_id: &str, expected_root: &Path, mut progress: impl FnMut(CleanupProgress)) -> Result<CleaningResult, String> {
    let plan = plans.lock().map_err(|_| "Scan state tidak tersedia")?.remove(plan_id).ok_or("Rencana scan tidak ditemukan; pindai ulang")?;
    if Instant::now() > plan.expires { return Err("Rencana scan sudah kedaluwarsa; pindai ulang".into()); }
    if expected_root != plan.root { return Err("Folder temp berubah; pindai ulang".into()); }
    let root_guards = windows::pin_root(&plan.root).map_err(|_| "Folder temp berubah; pindai ulang")?;
    if root_guards.last().and_then(|guard| guard.identity().ok()) != Some(plan.root_identity) { return Err("Folder temp berubah; pindai ulang".into()); }
    // Ensure history is writable before the first irreversible operation.
    let changed = database.lock().map_err(|_| "Database tidak tersedia")?.execute("UPDATE cleaning_scans SET status='running' WHERE id=?1 AND status='ready'", [plan_id]).map_err(|e| e.to_string())?;
    if changed != 1 { return Err("Rencana scan tidak ditemukan; pindai ulang".into()); }
    let mut result = CleaningResult { id:Uuid::new_v4().to_string(),category:"user_temp".into(),estimated_bytes:plan.estimated,recovered_bytes:0,deleted_count:0,skipped_count:plan.skipped,error_count:0,finished_at:String::new(),status:"completed".into() };
    let report = |processed, status: &str, result: &CleaningResult| CleanupProgress { plan_id: plan_id.into(), processed_count: processed, total_count: plan.files.len(), deleted_count: result.deleted_count, skipped_count: result.skipped_count, error_count: result.error_count, recovered_bytes: result.recovered_bytes, status: status.into(), timestamp: Utc::now().to_rfc3339() };
    progress(report(0, "running", &result));
    let mut last_progress = Instant::now();
    let mut processed = 0;
    for (index, file) in plan.files.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) { result.status = "cancelled".into(); result.skipped_count += plan.files.len() - index; break; }
        match delete_planned(&plan.root, file) {
            DeleteOutcome::Deleted => { result.deleted_count += 1; result.recovered_bytes = result.recovered_bytes.saturating_add(file.len); }
            DeleteOutcome::Skipped => { result.skipped_count += 1; }
            DeleteOutcome::Failed => { result.error_count += 1; }
        }
        processed = index + 1;
        if last_progress.elapsed() >= std::time::Duration::from_millis(100) {
            progress(report(index + 1, "running", &result)); last_progress = Instant::now();
        }
    }
    result.finished_at = Utc::now().to_rfc3339();
    if result.error_count > 0 && result.status == "completed" { result.status = "partial".into(); }
    progress(report(processed, &result.status, &result));
    let conn = database.lock().map_err(|_| "Database tidak tersedia")?;
    db::save_cleaning_result(&conn,plan_id,&result).map_err(|_| "Pembersihan selesai, tetapi riwayat gagal disimpan. Tinjau hasil terakhir di halaman pembersihan.".to_string())?;
    db::log(&conn,"cleaner",&result.status,&format!("{} files deleted; {} skipped; {} bytes removed",result.deleted_count,result.skipped_count,result.recovered_bytes));
    Ok(result)
}

enum DeleteOutcome { Deleted, Skipped, Failed }

fn delete_planned(root: &Path, planned: &PlannedFile) -> DeleteOutcome {
    if !allowed(root, &planned.path) || !allowed(root, &planned.canonical) { return DeleteOutcome::Skipped; }
    let _parents = match windows::pin_parents(root, &planned.path) { Ok(guards) => guards, Err(_) => return DeleteOutcome::Skipped };
    let handle = match windows::open_candidate(&planned.path, true) { Ok(handle) => handle, Err(_) => return DeleteOutcome::Skipped };
    let meta = match handle.metadata() { Ok(meta) => meta, Err(_) => return DeleteOutcome::Skipped };
    if !meta.is_file() || is_reparse(&meta) || meta.len() != planned.len || meta.modified().ok() != Some(planned.modified)
        || meta.modified().map(|t| t >= SystemTime::now() - std::time::Duration::from_secs(MIN_AGE_SECONDS)).unwrap_or(true)
        || windows::identity(&handle).ok() != Some(planned.identity) || windows::single_link(&handle).ok() != Some(true)
        || windows::final_path(&handle).ok().as_ref() != Some(&planned.canonical) { return DeleteOutcome::Skipped; }
    // The SAME exclusive handle is validated and marked for deletion. The file and
    // pinned parents stay open until disposition succeeds and the file is closed.
    if windows::mark_for_deletion(&handle).is_err() { return DeleteOutcome::Failed; }
    drop(handle);
    DeleteOutcome::Deleted
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::File, os::windows::fs::OpenOptionsExt};

    struct Fixture { directory: PathBuf, root: PathBuf, db: Mutex<Connection>, plans: Mutex<HashMap<String, Plan>> }
    impl Fixture {
        fn new() -> Self {
            let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("cleaner-tests");
            fs::create_dir_all(&base).unwrap();
            let directory = base.join(Uuid::new_v4().to_string());
            let root = directory.join("temp");
            fs::create_dir_all(&root).unwrap();
            let root = fs::canonicalize(root).unwrap();
            let db = db::open(&directory.join("test.db")).unwrap();
            Self { directory, root, db: Mutex::new(db), plans: Mutex::new(HashMap::new()) }
        }
        fn old(&self, name: &str) -> PathBuf {
            let path = self.root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"old temporary content").unwrap();
            File::options().write(true).open(&path).unwrap().set_modified(SystemTime::now() - std::time::Duration::from_secs(MIN_AGE_SECONDS + 600)).unwrap();
            path
        }
        fn scan(&self) -> CleanerScan { scan_at(&self.db, &self.plans, self.root.clone()).unwrap() }
        fn clean(&self, id: &str) -> CleaningResult {
            execute_at(&self.db, &self.plans, &AtomicBool::new(false), id, &self.root, |_| {}).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // All test data stays in the uniquely created workspace fixture.
            let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("cleaner-tests");
            assert!(self.directory.starts_with(&base) && self.directory != base);
            if let Ok(conn) = self.db.get_mut() {
                let connection = std::mem::replace(conn, Connection::open_in_memory().unwrap());
                let _ = connection.close();
            }
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn deletes_only_old_files_and_persists_an_accurate_result() {
        let f = Fixture::new();
        let old = f.old("nested/old.tmp");
        let recent = f.root.join("recent.tmp");
        fs::write(&recent, b"recent").unwrap();
        let scan = f.scan();
        assert_eq!(scan.eligible_count, 1);
        let result = f.clean(&scan.plan_id);
        assert_eq!(result.deleted_count, 1);
        assert_eq!(result.recovered_bytes, scan.estimated_bytes);
        assert_eq!(result.error_count, 0);
        assert!(!old.exists());
        assert!(recent.exists());
        assert_eq!(db::cleaning_history(&f.db.lock().unwrap()).unwrap()[0].id, result.id);
        assert!(execute_at(&f.db, &f.plans, &AtomicBool::new(false), &scan.plan_id, &f.root, |_| {}).is_err());
    }

    #[test]
    fn changed_and_locked_files_are_skipped() {
        let f = Fixture::new();
        let changed = f.old("changed.tmp");
        let locked = f.old("locked.tmp");
        let scan = f.scan();
        fs::write(&changed, b"new content").unwrap();
        let _lock = File::options().read(true).share_mode(0).open(&locked).unwrap();
        let result = f.clean(&scan.plan_id);
        assert_eq!(result.deleted_count, 0);
        assert_eq!(result.skipped_count, 2);
        assert!(changed.exists() && locked.exists());
    }

    #[test]
    fn same_size_and_timestamp_replacement_is_skipped_by_identity() {
        let f = Fixture::new();
        let original = f.old("replace.tmp");
        let scan = f.scan();
        let modified = fs::metadata(&original).unwrap().modified().unwrap();
        // Retain the original elsewhere so its ID cannot be recycled.
        fs::rename(&original, f.directory.join("original.tmp")).unwrap();
        fs::write(&original, b"old temporary content").unwrap();
        File::options().write(true).open(&original).unwrap().set_modified(modified).unwrap();
        assert_eq!(f.clean(&scan.plan_id).deleted_count, 0);
        assert!(original.exists());
    }

    #[test]
    fn hard_links_are_never_cleanup_candidates() {
        let f = Fixture::new();
        let original = f.old("linked.tmp");
        let outside = f.directory.join("outside.tmp");
        fs::hard_link(&original, &outside).unwrap();
        assert_eq!(f.scan().eligible_count, 0);
        assert!(outside.exists());
        // A hard link added after a scan must also be excluded.
        fs::remove_file(&outside).unwrap();
        let scan = f.scan();
        fs::hard_link(&original, &outside).unwrap();
        assert_eq!(f.clean(&scan.plan_id).deleted_count, 0);
        assert!(original.exists() && outside.exists());
    }

    #[test]
    fn cancellation_preserves_files_and_reports_terminal_progress() {
        let f = Fixture::new();
        let path = f.old("cancel.tmp");
        let scan = f.scan();
        let mut events = Vec::new();
        let result = execute_at(&f.db, &f.plans, &AtomicBool::new(true), &scan.plan_id, &f.root, |event| events.push(event)).unwrap();
        assert_eq!(result.status, "cancelled");
        assert_eq!(result.deleted_count, 0);
        assert_eq!(result.skipped_count, 1);
        assert_eq!(events.first().unwrap().processed_count, 0);
        assert_eq!(events.last().unwrap().status, "cancelled");
        assert_eq!(events.last().unwrap().total_count, 1);
        assert_eq!(events.last().unwrap().processed_count, 0);
        assert!(path.exists());
        let status: String = f.db.lock().unwrap().query_row("SELECT status FROM cleaning_scans WHERE id=?1", [&scan.plan_id], |row| row.get(0)).unwrap();
        assert_eq!(status, "cancelled");
    }

    #[test]
    fn expired_plan_and_replaced_root_are_rejected() {
        let f = Fixture::new();
        let path = f.old("old.tmp");
        let scan = f.scan();
        f.plans.lock().unwrap().get_mut(&scan.plan_id).unwrap().expires = Instant::now() - std::time::Duration::from_secs(1);
        assert!(execute_at(&f.db, &f.plans, &AtomicBool::new(false), &scan.plan_id, &f.root, |_| {}).is_err());
        assert!(path.exists());
        let scan = f.scan();
        fs::rename(&f.root, f.directory.join("original-root")).unwrap();
        fs::create_dir(&f.root).unwrap();
        assert!(execute_at(&f.db, &f.plans, &AtomicBool::new(false), &scan.plan_id, &f.root, |_| {}).is_err());
    }

    #[test]
    fn directory_pins_prevent_parent_swaps_until_released() {
        let f = Fixture::new();
        let path = f.old("nested/old.tmp");
        let root = windows::DirectoryGuard::open(&f.root).unwrap();
        let guards = windows::pin_parents(&f.root, &path).unwrap();
        assert!(fs::rename(path.parent().unwrap(), f.directory.join("moved")).is_err());
        assert!(fs::rename(&f.root, f.directory.join("moved-root")).is_err());
        drop(guards);
        drop(root);
        assert!(fs::rename(path.parent().unwrap(), f.directory.join("moved")).is_ok());
    }

    #[test]
    fn deletion_failure_is_counted_once_as_an_error() {
        let f = Fixture::new();
        let path = f.old("read-only.tmp");
        let scan = f.scan();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions.clone()).unwrap();
        let result = f.clean(&scan.plan_id);
        permissions.set_readonly(false);
        fs::set_permissions(&path, permissions).unwrap();
        assert_eq!(result.deleted_count, 0);
        assert_eq!(result.error_count, 1);
        assert_eq!(result.skipped_count, 0);
        assert_eq!(result.status, "partial");
        assert!(path.exists());
    }

    #[test]
    fn windows_temp_allowlist_is_a_real_directory_with_identity() {
        let root = root().unwrap();
        let guard = windows::DirectoryGuard::open(&root).unwrap();
        assert!(guard.identity().is_ok());
        assert!(!windows::pin_root(&root).unwrap().is_empty());
        assert_eq!(root.file_name().unwrap(), "Temp");
        assert!(execute(&Mutex::new(Connection::open_in_memory().unwrap()), &Mutex::new(HashMap::new()), &AtomicBool::new(false), "invalid", &["user_temp".into()], |_| {}).is_err());
    }

    fn junction(link: &Path, target: &Path) {
        let output = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "New-Item -ItemType Junction -Path $env:CORE_PULSE_TEST_LINK -Target $env:CORE_PULSE_TEST_TARGET -ErrorAction Stop | Out-Null"])
            .env("CORE_PULSE_TEST_LINK", link).env("CORE_PULSE_TEST_TARGET", target).output().unwrap();
        assert!(output.status.success(), "junction fixture creation failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    #[test]
    fn junctions_before_scan_and_after_preview_never_escape_the_root() {
        let f = Fixture::new();
        let nested = f.old("nested/old.tmp");
        let outside = f.directory.join("outside");
        fs::create_dir(&outside).unwrap();
        let protected = outside.join("old.tmp");
        fs::write(&protected, b"protected").unwrap();
        File::options().write(true).open(&protected).unwrap().set_modified(SystemTime::now() - std::time::Duration::from_secs(MIN_AGE_SECONDS + 600)).unwrap();
        let existing_link = f.root.join("redirect");
        junction(&existing_link, &outside);
        let scan = f.scan();
        assert_eq!(scan.eligible_count, 1);
        fs::remove_dir(&existing_link).unwrap();
        let parent = nested.parent().unwrap();
        fs::rename(parent, f.directory.join("original-nested")).unwrap();
        junction(parent, &outside);
        let result = f.clean(&scan.plan_id);
        assert_eq!(result.deleted_count, 0);
        assert_eq!(result.skipped_count, scan.skipped_count + 1);
        assert_eq!(fs::read(&protected).unwrap(), b"protected");
        fs::remove_dir(parent).unwrap();
    }

    #[test]
    fn root_ancestor_pins_prevent_moving_the_entire_allowlist() {
        let f = Fixture::new();
        let guards = windows::pin_root(&f.root).unwrap();
        assert!(fs::rename(&f.directory, f.directory.with_extension("moved")).is_err());
        drop(guards);
    }
    #[test]
    fn only_children_of_canonical_root_are_allowed() {
        let root = Path::new("C:\\Users\\sample\\AppData\\Local\\Temp");
        assert!(allowed(root, &root.join("old.tmp")));
        assert!(!allowed(root, root));
        assert!(!allowed(root, Path::new("C:\\Users\\sample\\Downloads\\old.tmp")));
    }
}
