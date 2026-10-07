mod windows;

use chrono::{Duration as ChronoDuration, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
use uuid::Uuid;
use windows::{Executable, Process};

const SELECTION_TTL: Duration = Duration::from_secs(300);
const MAX_GAMES: usize = 100;
const MAX_PROCESSES: usize = 8192;
const MAX_NEW_INSPECTIONS: usize = 256;
const MAX_TRACKED_PROCESSES: usize = 512;

#[derive(Clone, Copy)]
struct PollBudget {
    inspections: usize,
    tracked: usize,
}
impl Default for PollBudget {
    fn default() -> Self {
        Self {
            inspections: MAX_NEW_INSPECTIONS,
            tracked: MAX_TRACKED_PROCESSES,
        }
    }
}

#[derive(Clone, Serialize)]
pub struct Selection {
    pub selection_id: String,
    pub canonical_executable_path: String,
    pub suggested_name: String,
    pub expires_at: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct RegisteredGame {
    pub id: String,
    pub display_name: String,
    pub canonical_executable_path: String,
    pub profile_id: String,
    pub auto_boost: bool,
    pub restore_on_exit: bool,
    pub ac_only: bool,
    #[serde(skip_serializing)]
    pub executable_identity: String,
}
impl RegisteredGame {
    fn executable(&self) -> Executable {
        Executable {
            path: self.canonical_executable_path.clone(),
            identity: self.executable_identity.clone(),
        }
    }
}
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct Settings {
    pub display_name: String,
    pub profile_id: String,
    pub restore_on_exit: bool,
    pub ac_only: bool,
}
impl Settings {
    fn validate(&self) -> Result<(), String> {
        if self.display_name.trim().is_empty()
            || self.display_name.chars().count() > 100
            || self.display_name.chars().any(char::is_control)
        {
            return Err("Nama game harus berisi 1–100 karakter tanpa karakter kontrol".into());
        }
        if !["balanced", "gaming", "saving"].contains(&self.profile_id.as_str()) {
            return Err("Profil tidak ditemukan".into());
        }
        Ok(())
    }
}
#[derive(Clone, Serialize)]
pub struct GameStatus {
    pub game: RegisteredGame,
    pub status: String,
    pub process_count: usize,
    pub message: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct Snapshot {
    pub recorded_at: String,
    pub status: String,
    pub games: Vec<GameStatus>,
}
struct PendingSelection {
    file: Executable,
    selected: Instant,
}

#[derive(Default)]
pub struct Registry {
    selection: HashMap<String, PendingSelection>,
    processes: HashMap<String, Vec<Arc<Process>>>,
    previous: Option<(Instant, Snapshot)>,
    candidate_cursor: u32,
    #[cfg(test)]
    last_inspections: usize,
}
impl Registry {
    // Only the native file-picker calls this method; IPC accepts its opaque token.
    pub fn selected(&mut self, path: &Path) -> Result<Selection, String> {
        self.selection.clear();
        let file = windows::inspect(path)?;
        let selection_id = Uuid::new_v4().to_string();
        let selected = Selection {
            selection_id: selection_id.clone(),
            canonical_executable_path: file.path.clone(),
            suggested_name: path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            expires_at: (Utc::now() + ChronoDuration::minutes(5)).to_rfc3339(),
        };
        self.selection.insert(
            selection_id,
            PendingSelection {
                file,
                selected: Instant::now(),
            },
        );
        Ok(selected)
    }
    pub fn register(
        &mut self,
        database: &Mutex<Connection>,
        selection_id: &str,
        settings: Settings,
    ) -> Result<RegisteredGame, String> {
        settings.validate()?;
        let pending = self
            .selection
            .get(selection_id)
            .ok_or("Pilih ulang executable sebelum menyimpan game")?;
        if pending.selected.elapsed() > SELECTION_TTL {
            self.selection.remove(selection_id);
            return Err("Pilihan executable kedaluwarsa. Pilih ulang file.".into());
        }
        let current = windows::inspect(Path::new(&pending.file.path))?;
        if current != pending.file {
            self.selection.remove(selection_id);
            return Err("Executable berubah sejak dipilih. Pilih ulang file.".into());
        }
        let mut conn = database.lock().map_err(|_| "Database tidak tersedia")?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let count: usize = tx
            .query_row(
                "SELECT COUNT(*) FROM registered_games WHERE archived=0",
                [],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if count >= MAX_GAMES {
            return Err("Batas 100 game terdaftar telah tercapai".into());
        }
        let duplicate: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM registered_games WHERE archived=0 AND executable_identity=?1)", [&current.identity], |row| row.get(0)).map_err(|error| error.to_string())?;
        if duplicate {
            return Err("Executable ini sudah terdaftar".into());
        }
        let id = Uuid::new_v4().to_string();
        let at = Utc::now().to_rfc3339();
        tx.execute("INSERT INTO registered_games(id,display_name,canonical_executable_path,executable_identity,profile_id,auto_boost,restore_on_exit,ac_only,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,0,?6,?7,?8,?8)", params![id,settings.display_name.trim(),current.path,current.identity,settings.profile_id,settings.restore_on_exit,settings.ac_only,at]).map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        self.selection.remove(selection_id);
        self.previous = None;
        crate::db::log(
            &conn,
            "game",
            "registered",
            "Game registered with verified executable identity; automatic tuning disabled",
        );
        get_games(&conn)?
            .into_iter()
            .find(|game| game.id == id)
            .ok_or("Game tidak ditemukan".into())
    }
    pub fn update(
        &mut self,
        conn: &Connection,
        id: &str,
        settings: Settings,
    ) -> Result<RegisteredGame, String> {
        settings.validate()?;
        Uuid::parse_str(id).map_err(|_| "Identitas game tidak valid")?;
        let current = get_games(conn)?
            .into_iter()
            .find(|game| game.id == id)
            .ok_or("Game tidak ditemukan")?;
        if current.auto_boost
            && (current.profile_id != settings.profile_id
                || current.ac_only != settings.ac_only
                || current.restore_on_exit != settings.restore_on_exit)
        {
            return Err(
                "Nonaktifkan Auto Boost sebelum mengubah profil atau aturan pemulihan.".into(),
            );
        }
        let changed = conn.execute("UPDATE registered_games SET display_name=?2,profile_id=?3,restore_on_exit=?4,ac_only=?5,updated_at=?6 WHERE id=?1 AND archived=0", params![id,settings.display_name.trim(),settings.profile_id,settings.restore_on_exit,settings.ac_only,Utc::now().to_rfc3339()]).map_err(|error| error.to_string())?;
        if changed != 1 {
            return Err("Game tidak ditemukan".into());
        }
        self.previous = None;
        get_games(conn)?
            .into_iter()
            .find(|game| game.id == id)
            .ok_or("Game tidak ditemukan".into())
    }
    pub fn remove(&mut self, conn: &Connection, id: &str) -> Result<(), String> {
        Uuid::parse_str(id).map_err(|_| "Identitas game tidak valid")?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|error| error.to_string())?;
        let changed = tx.execute("UPDATE registered_games SET archived=1,auto_boost=0,updated_at=?2 WHERE id=?1 AND archived=0", params![id,Utc::now().to_rfc3339()]).map_err(|error| error.to_string())?;
        if changed != 1 {
            return Err("Game tidak ditemukan".into());
        }
        // Keep referenced games for session history; do not accumulate unused
        // registrations forever when a game was never observed in a session.
        tx.execute("DELETE FROM registered_games WHERE id=?1 AND archived=1 AND NOT EXISTS(SELECT 1 FROM gaming_sessions WHERE game_id=?1)",[id]).map_err(|error|error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        self.processes.remove(id);
        self.previous = None;
        crate::db::log(
            conn,
            "game",
            "removed",
            "Game removed from registration; historical sessions preserved",
        );
        Ok(())
    }
    pub fn snapshot(&mut self, games: Vec<RegisteredGame>) -> Result<Snapshot, String> {
        self.snapshot_with_budget(games, PollBudget::default())
    }
    fn snapshot_with_budget(
        &mut self,
        games: Vec<RegisteredGame>,
        budget: PollBudget,
    ) -> Result<Snapshot, String> {
        if let Some((at, previous)) = &self.previous {
            if at.elapsed() < Duration::from_secs(2) {
                return Ok(previous.clone());
            }
        }
        let ids: HashSet<_> = games.iter().map(|game| game.id.clone()).collect();
        self.processes.retain(|id, processes| {
            processes.retain(|process| process.alive());
            ids.contains(id)
        });
        let mut tracked = self.processes.values().map(Vec::len).sum::<usize>();
        // A still-live held handle proves this PID has not been reused. Skip
        // reopening it; unknown wait results must be inspected conservatively.
        let known: HashSet<_> = self
            .processes
            .values()
            .flatten()
            .filter(|process| process.verified_alive())
            .map(|process| process.pid)
            .collect();
        let names: HashSet<_> = games
            .iter()
            .map(|game| {
                PathBuf::from(&game.canonical_executable_path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase()
            })
            .collect();
        let mut candidates = vec![];
        let mut complete = true;
        let mut limited_names = HashSet::new();
        if !games.is_empty() {
            let mut system = System::new();
            system.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::nothing(),
            );
            complete = !system.processes().is_empty() && system.processes().len() <= MAX_PROCESSES;
            let mut listed: Vec<_> = system.processes().iter().collect();
            listed.sort_by_key(|(pid, _)| pid.as_u32());
            listed.truncate(MAX_PROCESSES);
            let split = listed.partition_point(|(pid, _)| pid.as_u32() <= self.candidate_cursor);
            // Rotate within the bounded list so repeated unrelated same-name
            // processes do not permanently starve a later verified game.
            for (pid, process) in listed[split..].iter().chain(&listed[..split]) {
                let name = process.name().to_string_lossy().to_lowercase();
                if names.contains(&name) && !known.contains(&pid.as_u32()) {
                    if candidates.len() >= budget.inspections || tracked >= budget.tracked {
                        limited_names.insert(name);
                        continue;
                    }
                    self.candidate_cursor = pid.as_u32();
                    // A name is only a prefilter. The held process handle, creation
                    // time, canonical path, and file ID supply the actual match.
                    candidates.push((name, Process::open(pid.as_u32())));
                }
            }
        }
        #[cfg(test)]
        {
            self.last_inspections = candidates.len();
        }
        let mut incomplete = !complete || !limited_names.is_empty();
        let mut statuses = vec![];
        for game in games {
            let expected = game.executable();
            let valid = windows::inspect(Path::new(&expected.path))
                .is_ok_and(|current| current == expected && !expected.identity.is_empty());
            let basename = PathBuf::from(&expected.path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            let mut uncertain = !complete || limited_names.contains(&basename);
            let running = self.processes.entry(game.id.clone()).or_default();
            for (name, candidate) in &mut candidates {
                if name != &basename {
                    continue;
                }
                match candidate {
                    Ok(process)
                        if valid
                            && process.matches(&expected)
                            && !running.iter().any(|known| {
                                known.pid == process.pid && known.created == process.created
                            }) =>
                    {
                        if tracked >= budget.tracked {
                            uncertain = true;
                            continue;
                        }
                        // Move the checked handle without reopening a potentially
                        // reused PID. Other registered files cannot share its ID.
                        if let Ok(process) = std::mem::replace(candidate, Err(String::new())) {
                            running.push(Arc::new(process));
                            tracked += 1;
                        }
                    }
                    Err(error) if !error.is_empty() => uncertain = true,
                    _ => {}
                }
            }
            let status = if !valid {
                "unavailable"
            } else if !running.is_empty() {
                "running"
            } else if uncertain {
                "unverified"
            } else {
                "ready"
            };
            incomplete |= uncertain;
            let message = if status == "unavailable" {
                Some(
                    "Executable tidak tersedia atau identitasnya berubah. Daftarkan ulang file."
                        .into(),
                )
            } else if uncertain {
                Some("Pemeriksaan proses belum lengkap. Jumlah yang ditampilkan hanya proses terverifikasi.".into())
            } else {
                None
            };
            statuses.push(GameStatus {
                game,
                status: status.into(),
                process_count: running.len(),
                message,
            });
        }
        let snapshot = Snapshot {
            recorded_at: Utc::now().to_rfc3339(),
            status: if incomplete { "degraded" } else { "ready" }.into(),
            games: statuses,
        };
        self.previous = Some((Instant::now(), snapshot.clone()));
        Ok(snapshot)
    }
}

pub struct Observation {
    snapshot: Snapshot,
    proofs: HashMap<String, Vec<Arc<Process>>>,
}
impl Observation {
    pub fn has_live_game(&self, game: &RegisteredGame) -> bool {
        self.proofs.get(&game.id).is_some_and(|proofs| {
            proofs
                .iter()
                .any(|process| process.verified_alive() && process.matches(&game.executable()))
        })
    }
    pub fn view(&self, games: Vec<RegisteredGame>) -> Snapshot {
        let mut rows = vec![];
        for game in games {
            let expected = game.executable();
            let previous = self
                .snapshot
                .games
                .iter()
                .find(|row| row.game.id == game.id && row.game.executable() == expected);
            let count = self
                .proofs
                .get(&game.id)
                .map(|proofs| {
                    proofs
                        .iter()
                        .filter(|process| process.verified_alive() && process.matches(&expected))
                        .count()
                })
                .unwrap_or(0);
            let status = match previous {
                Some(row) if row.status == "running" && count == 0 => "unverified",
                Some(row) => row.status.as_str(),
                None => "unverified",
            };
            rows.push(GameStatus {
                game,
                status: status.into(),
                process_count: count,
                message: previous.and_then(|row| row.message.clone()),
            });
        }
        Snapshot {
            recorded_at: self.snapshot.recorded_at.clone(),
            status: self.snapshot.status.clone(),
            games: rows,
        }
    }
}
impl Registry {
    pub fn observe(&mut self, games: Vec<RegisteredGame>) -> Result<Observation, String> {
        self.previous = None;
        let snapshot = self.snapshot(games)?;
        Ok(Observation {
            snapshot,
            proofs: self.processes.clone(),
        })
    }
    pub fn set_auto(
        &mut self,
        conn: &Connection,
        id: &str,
        enabled: bool,
        expected: &Settings,
        plans: &[crate::models::PowerPlan],
    ) -> Result<RegisteredGame, String> {
        Uuid::parse_str(id).map_err(|_| "Identitas game tidak valid")?;
        let game = get_games(conn)?
            .into_iter()
            .find(|game| game.id == id)
            .ok_or("Game tidak ditemukan")?;
        if enabled {
            expected.validate()?;
            if game.settings() != *expected {
                return Err(
                    "Pengaturan game berubah. Muat ulang sebelum mengaktifkan Auto Boost.".into(),
                );
            }
            if game.executable_identity.is_empty() {
                return Err("Executable tidak dapat diverifikasi untuk Auto Boost".into());
            }
            let profile = crate::db::profiles(conn)?
                .into_iter()
                .find(|profile| profile.id == game.profile_id)
                .ok_or("Profil tidak ditemukan")?;
            if !plans
                .iter()
                .any(|plan| Some(plan.guid.as_str()) == profile.scheme_guid.as_deref())
            {
                return Err(
                    "Petakan profil ke skema daya yang tersedia sebelum mengaktifkan Auto Boost."
                        .into(),
                );
            }
        }
        conn.execute(
            "UPDATE registered_games SET auto_boost=?2,updated_at=?3 WHERE id=?1 AND archived=0",
            params![id, enabled, Utc::now().to_rfc3339()],
        )
        .map_err(|error| error.to_string())?;
        self.previous = None;
        Ok(RegisteredGame {
            auto_boost: enabled,
            ..game
        })
    }
}
impl RegisteredGame {
    pub fn settings(&self) -> Settings {
        Settings {
            display_name: self.display_name.clone(),
            profile_id: self.profile_id.clone(),
            restore_on_exit: self.restore_on_exit,
            ac_only: self.ac_only,
        }
    }
    pub fn validate_file(&self) -> Result<(), String> {
        if windows::inspect(Path::new(&self.canonical_executable_path))? != self.executable()
            || self.executable_identity.is_empty()
        {
            return Err("Executable tidak dapat diverifikasi untuk Auto Boost".into());
        }
        Ok(())
    }
}

pub fn get_games(conn: &Connection) -> Result<Vec<RegisteredGame>, String> {
    let mut query=conn.prepare("SELECT id,display_name,canonical_executable_path,profile_id,auto_boost,restore_on_exit,ac_only,executable_identity FROM registered_games WHERE archived=0 ORDER BY created_at,id LIMIT 100").map_err(|error| error.to_string())?;
    let rows = query
        .query_map([], |row| {
            Ok(RegisteredGame {
                id: row.get(0)?,
                display_name: row.get(1)?,
                canonical_executable_path: row.get(2)?,
                profile_id: row.get(3)?,
                auto_boost: row.get(4)?,
                restore_on_exit: row.get(5)?,
                ac_only: row.get(6)?,
                executable_identity: row.get(7)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

#[derive(Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub game_name: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub status: String,
    pub tuning_session_id: Option<String>,
    pub profile_name: Option<String>,
}
pub fn sessions(conn: &Connection) -> Result<Vec<Session>, String> {
    let mut query=conn.prepare("SELECT s.id,g.display_name,s.started_at,s.ended_at,s.status,s.tuning_session_id,p.name FROM gaming_sessions s JOIN registered_games g ON g.id=s.game_id LEFT JOIN tuning_sessions t ON t.id=s.tuning_session_id LEFT JOIN performance_profiles p ON p.id=t.profile_id ORDER BY s.started_at DESC,s.id DESC LIMIT 50").map_err(|error|error.to_string())?;
    let rows = query
        .query_map([], |row| {
            Ok(Session {
                id: row.get(0)?,
                game_name: row.get(1)?,
                started_at: row.get(2)?,
                ended_at: row.get(3)?,
                status: row.get(4)?,
                tuning_session_id: row.get(5)?,
                profile_name: row.get(6)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}
pub fn delete_session(conn: &mut Connection, id: &str) -> Result<(), String> {
    Uuid::parse_str(id).map_err(|_| "Identitas sesi game tidak valid")?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let allowed: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM gaming_sessions WHERE id=?1 AND status!='active')",
            [id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !allowed {
        return Err("Sesi game aktif tidak dapat dihapus".into());
    }
    tx.execute(
        "DELETE FROM gaming_session_metrics WHERE session_id=?1",
        [id],
    )
    .map_err(|error| error.to_string())?;
    // Keep monitoring readings and all power recovery records. Only remove the
    // selected completed summary and its optional derived metrics.
    tx.execute(
        "UPDATE hardware_samples SET gaming_session_id=NULL WHERE gaming_session_id=?1",
        [id],
    )
    .map_err(|error| error.to_string())?;
    tx.execute("DELETE FROM gaming_sessions WHERE id=?1", [id])
        .map_err(|error| error.to_string())?;
    tx.execute("DELETE FROM registered_games WHERE archived=1 AND NOT EXISTS(SELECT 1 FROM gaming_sessions WHERE game_id=registered_games.id)",[]).map_err(|error|error.to_string())?;
    tx.commit().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    struct Fixture {
        root: PathBuf,
        first: PathBuf,
        second: PathBuf,
        database: Mutex<Connection>,
        registry: Registry,
    }
    impl Fixture {
        fn new() -> Self {
            let project = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
            let root = project
                .join("artifacts/games")
                .join(Uuid::new_v4().to_string());
            let name = format!("game-{}.exe", Uuid::new_v4());
            let first = root.join("one").join(&name);
            let second = root.join("two").join(&name);
            std::fs::create_dir_all(first.parent().unwrap()).unwrap();
            std::fs::create_dir_all(second.parent().unwrap()).unwrap();
            let root = std::fs::canonicalize(root).unwrap();
            assert!(root.starts_with(std::fs::canonicalize(project).unwrap()));
            let source =
                PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/ping.exe");
            std::fs::copy(&source, &first).unwrap();
            std::fs::copy(&source, &second).unwrap();
            Self {
                root,
                first,
                second,
                database: Mutex::new(crate::db::open(Path::new(":memory:")).unwrap()),
                registry: Registry::default(),
            }
        }
        fn settings() -> Settings {
            Settings {
                display_name: "Test game".into(),
                profile_id: "gaming".into(),
                restore_on_exit: true,
                ac_only: true,
            }
        }
        fn register(&mut self) -> RegisteredGame {
            let selected = self.registry.selected(&self.first).unwrap();
            self.registry
                .register(&self.database, &selected.selection_id, Self::settings())
                .unwrap()
        }
        fn snapshot(&mut self) -> Snapshot {
            self.registry.previous = None;
            let games = get_games(&self.database.lock().unwrap()).unwrap();
            self.registry.snapshot(games).unwrap()
        }
        fn bounded_snapshot(&mut self, budget: PollBudget) -> Snapshot {
            self.registry.previous = None;
            let games = get_games(&self.database.lock().unwrap()).unwrap();
            self.registry.snapshot_with_budget(games, budget).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    struct Child(std::process::Child);
    impl Child {
        fn start(path: &Path) -> Self {
            Self(
                Command::new(path)
                    .args(["-n", "30", "127.0.0.1"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            )
        }
        fn stop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    impl Drop for Child {
        fn drop(&mut self) {
            self.stop();
        }
    }
    #[test]
    fn bounded_handles_keep_uncertain_overlap_until_a_verified_final_exit() {
        let mut fixture = Fixture::new();
        let game = fixture.register();
        let mut first = Child::start(&fixture.first);
        let mut second = Child::start(&fixture.first);
        let budget = PollBudget {
            inspections: 1,
            tracked: 1,
        };
        let snapshot = fixture.bounded_snapshot(budget);
        assert_eq!(snapshot.status, "degraded");
        assert_eq!(snapshot.games[0].status, "running");
        assert_eq!(snapshot.games[0].process_count, 1);
        assert!(snapshot.games[0].message.is_some());
        assert_eq!(fixture.registry.last_inspections, 1);
        let held = fixture.registry.processes[&game.id][0].clone();
        let next = fixture.bounded_snapshot(budget);
        assert_eq!(next.games[0].process_count, 1);
        assert_eq!(fixture.registry.last_inspections, 0);
        assert!(Arc::ptr_eq(&held, &fixture.registry.processes[&game.id][0]));
        if held.pid == first.0.id() {
            first.stop();
        } else {
            second.stop();
        }
        let after_exit = fixture.bounded_snapshot(budget);
        assert_eq!(after_exit.games[0].status, "running");
        assert_eq!(after_exit.games[0].process_count, 1);
        assert_ne!(fixture.registry.processes[&game.id][0].pid, held.pid);
        assert_eq!(
            fixture
                .registry
                .processes
                .values()
                .map(Vec::len)
                .sum::<usize>(),
            1
        );
        first.stop();
        second.stop();
        let ended = fixture.bounded_snapshot(budget);
        assert_eq!(ended.games[0].status, "ready");
        assert_eq!(ended.games[0].process_count, 0);
        assert!(ended.games[0].message.is_none());
        assert_eq!(
            fixture
                .registry
                .processes
                .values()
                .map(Vec::len)
                .sum::<usize>(),
            0
        );
    }
    #[test]
    fn bounded_inspection_rotates_past_unrelated_same_name_processes() {
        let mut fixture = Fixture::new();
        fixture.register();
        let mut unrelated = Child::start(&fixture.second);
        let mut matching = Child::start(&fixture.first);
        fixture.registry.candidate_cursor = unrelated.0.id().saturating_sub(1);
        let budget = PollBudget {
            inspections: 1,
            tracked: 2,
        };
        let limited = fixture.bounded_snapshot(budget);
        assert_eq!(limited.games[0].status, "unverified");
        assert_eq!(limited.games[0].process_count, 0);
        assert_eq!(fixture.registry.last_inspections, 1);
        let rotated = fixture.bounded_snapshot(budget);
        assert_eq!(rotated.games[0].status, "running");
        assert_eq!(rotated.games[0].process_count, 1);
        assert_eq!(fixture.registry.last_inspections, 1);
        let complete = fixture.bounded_snapshot(budget);
        assert_eq!(complete.status, "ready");
        assert_eq!(complete.games[0].process_count, 1);
        assert!(complete.games[0].message.is_none());
        matching.stop();
        let ended = fixture.bounded_snapshot(budget);
        assert_eq!(ended.games[0].status, "ready");
        assert_eq!(ended.games[0].process_count, 0);
        assert!(unrelated.0.try_wait().unwrap().is_none());
    }
    #[test]
    fn known_live_handles_are_not_reopened_even_with_spare_budget() {
        let mut fixture = Fixture::new();
        let game = fixture.register();
        let _first = Child::start(&fixture.first);
        let _second = Child::start(&fixture.first);
        let budget = PollBudget {
            inspections: 2,
            tracked: 3,
        };
        assert_eq!(fixture.bounded_snapshot(budget).games[0].process_count, 2);
        assert_eq!(fixture.registry.last_inspections, 2);
        let held = fixture.registry.processes[&game.id].clone();
        assert_eq!(fixture.bounded_snapshot(budget).games[0].process_count, 2);
        assert_eq!(fixture.registry.last_inspections, 0);
        for process in held {
            assert!(fixture.registry.processes[&game.id]
                .iter()
                .any(|next| Arc::ptr_eq(&process, next)));
        }
    }
    #[test]
    fn opt_in_requires_current_settings_and_mapping_and_armed_policy_edits_require_disarming() {
        let mut fixture = Fixture::new();
        let game = fixture.register();
        let target = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";
        let plans = vec![crate::models::PowerPlan {
            guid: target.into(),
            name: "Test plan".into(),
            active: false,
        }];
        let mut conn = fixture.database.lock().unwrap();
        assert!(fixture
            .registry
            .set_auto(&conn, &game.id, true, &game.settings(), &plans)
            .is_err());
        crate::db::save_profile_mapping(&mut conn, "gaming", target, false).unwrap();
        let mut changed = game.settings();
        changed.profile_id = "saving".into();
        assert!(fixture
            .registry
            .set_auto(&conn, &game.id, true, &changed, &plans)
            .is_err());
        assert!(!get_games(&conn).unwrap()[0].auto_boost);
        assert!(
            fixture
                .registry
                .set_auto(&conn, &game.id, true, &game.settings(), &plans)
                .unwrap()
                .auto_boost
        );
        changed = game.settings();
        changed.ac_only = false;
        assert!(fixture.registry.update(&conn, &game.id, changed).is_err());
        let mut renamed = game.settings();
        renamed.display_name = "Renamed".into();
        let renamed = fixture.registry.update(&conn, &game.id, renamed).unwrap();
        std::fs::remove_file(&fixture.first).unwrap();
        assert!(renamed.validate_file().is_err());
        assert!(
            !fixture
                .registry
                .set_auto(&conn, &game.id, false, &renamed.settings(), &[])
                .unwrap()
                .auto_boost
        );
    }
    #[test]
    fn held_observation_rechecks_process_liveness_without_reopening_executables() {
        let mut fixture = Fixture::new();
        let game = fixture.register();
        let mut child = Child::start(&fixture.first);
        let observation = fixture.registry.observe(vec![game.clone()]).unwrap();
        assert_eq!(
            observation.view(vec![game.clone()]).games[0].process_count,
            1
        );
        child.stop();
        let after = observation.view(vec![game]);
        assert_eq!(after.games[0].process_count, 0);
        assert_eq!(after.games[0].status, "unverified");
    }
    #[test]
    fn deleting_completed_history_preserves_samples_active_sessions_and_power_recovery() {
        let mut fixture = Fixture::new();
        let game = fixture.register();
        let completed = Uuid::new_v4().to_string();
        let active = Uuid::new_v4().to_string();
        let tuning = Uuid::new_v4().to_string();
        let mut conn = fixture.database.lock().unwrap();
        conn.execute("INSERT INTO tuning_sessions(id,profile_id,previous_scheme_guid,applied_scheme_guid,trigger,status,started_at) VALUES(?1,'gaming','previous','applied','auto_game','conflict','2026-10-06T00:00:00Z')",[&tuning]).unwrap();
        for (id, status) in [(&completed, "completed"), (&active, "active")] {
            conn.execute("INSERT INTO gaming_sessions(id,game_id,started_at,status,tuning_session_id) VALUES(?1,?2,'2026-10-06T00:00:00Z',?3,?4)",params![id,game.id,status,tuning]).unwrap();
        }
        conn.execute_batch("INSERT INTO devices(id,created_at,updated_at) VALUES('history-device','now','now');INSERT INTO hardware_sensors(id,device_id,provider,sensor_key,sensor_name) VALUES('history-sensor','history-device','test','cpu','CPU');").unwrap();
        conn.execute("INSERT INTO hardware_samples(sensor_id,gaming_session_id,recorded_at,value) VALUES('history-sensor',?1,'2026-10-06T00:00:00Z',0)",[&completed]).unwrap();
        conn.execute(
            "INSERT INTO gaming_session_metrics(id,session_id) VALUES(?1,?2)",
            params![Uuid::new_v4().to_string(), completed],
        )
        .unwrap();
        assert!(delete_session(&mut conn, &active).is_err());
        delete_session(&mut conn, &completed).unwrap();
        assert_eq!(sessions(&conn).unwrap().len(), 1);
        let sample: (Option<String>, f64) = conn
            .query_row(
                "SELECT gaming_session_id,value FROM hardware_samples",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(sample, (None, 0.0));
        assert_eq!(crate::db::unfinished_sessions(&conn).unwrap()[0].id, tuning);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM gaming_session_metrics", [], |row| row
                .get::<_, usize>(0))
                .unwrap(),
            0
        );
        assert!(conn
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query([])
            .unwrap()
            .next()
            .unwrap()
            .is_none());
        assert!(fixture.first.is_file());
    }
    #[test]
    fn registration_requires_native_unexpired_unchanged_selection_and_valid_settings() {
        let mut fixture = Fixture::new();
        assert!(fixture
            .registry
            .register(&fixture.database, "arbitrary path", Fixture::settings())
            .is_err());
        let selected = fixture.registry.selected(&fixture.first).unwrap();
        fixture
            .registry
            .selection
            .get_mut(&selected.selection_id)
            .unwrap()
            .selected = Instant::now() - SELECTION_TTL - Duration::from_secs(1);
        assert!(fixture
            .registry
            .register(
                &fixture.database,
                &selected.selection_id,
                Fixture::settings()
            )
            .unwrap_err()
            .contains("kedaluwarsa"));
        let selected = fixture.registry.selected(&fixture.first).unwrap();
        let old = fixture.first.with_extension("old");
        std::fs::rename(&fixture.first, old).unwrap();
        std::fs::copy(&fixture.second, &fixture.first).unwrap();
        assert!(fixture
            .registry
            .register(
                &fixture.database,
                &selected.selection_id,
                Fixture::settings()
            )
            .unwrap_err()
            .contains("berubah"));
        let selected = fixture.registry.selected(&fixture.first).unwrap();
        for name in ["", "\n", &"x".repeat(101)] {
            let mut settings = Fixture::settings();
            settings.display_name = name.into();
            assert!(fixture
                .registry
                .register(&fixture.database, &selected.selection_id, settings)
                .is_err());
        }
        {
            use std::io::Write;
            let mut patch = std::fs::OpenOptions::new()
                .append(true)
                .open(&fixture.first)
                .unwrap();
            patch.write_all(b"patch").unwrap();
        }
        assert!(fixture
            .registry
            .register(
                &fixture.database,
                &selected.selection_id,
                Fixture::settings()
            )
            .unwrap_err()
            .contains("berubah"));
        let selected = fixture.registry.selected(&fixture.first).unwrap();
        let game = fixture
            .registry
            .register(
                &fixture.database,
                &selected.selection_id,
                Fixture::settings(),
            )
            .unwrap();
        assert!(!game.auto_boost);
        assert!(fixture
            .registry
            .register(
                &fixture.database,
                &selected.selection_id,
                Fixture::settings()
            )
            .is_err());
        let next = fixture.registry.selected(&fixture.first).unwrap();
        assert!(fixture
            .registry
            .register(&fixture.database, &next.selection_id, Fixture::settings())
            .unwrap_err()
            .contains("sudah terdaftar"));
        assert_eq!(
            get_games(&fixture.database.lock().unwrap()).unwrap().len(),
            1
        );
    }
    #[test]
    fn edits_validate_profile_and_removal_preserves_session_foreign_keys_and_executable() {
        let mut fixture = Fixture::new();
        let game = fixture.register();
        let conn = fixture.database.lock().unwrap();
        conn.execute("INSERT INTO gaming_sessions(id,game_id,started_at,status) VALUES('past',?1,'2026-10-06T00:00:00Z','completed')",[&game.id]).unwrap();
        let mut settings = Fixture::settings();
        settings.display_name = "Renamed".into();
        settings.profile_id = "missing".into();
        assert!(fixture
            .registry
            .update(&conn, &game.id, settings.clone())
            .is_err());
        settings.profile_id = "balanced".into();
        let updated = fixture.registry.update(&conn, &game.id, settings).unwrap();
        assert_eq!(updated.display_name, "Renamed");
        assert_eq!(updated.profile_id, "balanced");
        fixture.registry.remove(&conn, &game.id).unwrap();
        assert!(get_games(&conn).unwrap().is_empty());
        assert_eq!(
            conn.query_row(
                "SELECT game_id FROM gaming_sessions WHERE id='past'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            game.id
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, usize>(0)
            })
            .unwrap(),
            0
        );
        assert!(fixture.first.is_file());
        assert!(fixture.registry.remove(&conn, &game.id).is_err());
    }
    #[test]
    fn real_watcher_rejects_same_name_elsewhere_tracks_overlap_and_detects_final_exit() {
        let mut fixture = Fixture::new();
        let _game = fixture.register();
        let _unrelated = Child::start(&fixture.second);
        std::thread::sleep(Duration::from_millis(100));
        let initial = fixture.snapshot();
        assert_eq!(initial.games[0].process_count, 0);
        assert_eq!(initial.games[0].status, "ready");
        let mut first = Child::start(&fixture.first);
        let mut second = Child::start(&fixture.first);
        std::thread::sleep(Duration::from_millis(100));
        let live = fixture.snapshot();
        assert_eq!(live.games[0].process_count, 2);
        assert_eq!(live.games[0].status, "running");
        first.stop();
        assert_eq!(fixture.snapshot().games[0].process_count, 1);
        second.stop();
        assert_eq!(fixture.snapshot().games[0].process_count, 0);
        assert_eq!(fixture.snapshot().games[0].status, "ready");
    }
    #[test]
    fn unavailable_executable_never_matches_and_snapshots_are_cached() {
        let mut fixture = Fixture::new();
        let _game = fixture.register();
        let snapshot = fixture.snapshot();
        let cached = fixture.registry.snapshot(vec![]).unwrap();
        assert_eq!(cached.recorded_at, snapshot.recorded_at);
        std::fs::remove_file(&fixture.first).unwrap();
        assert_eq!(fixture.snapshot().games[0].status, "unavailable");
    }
}
