mod windows;

use chrono::{Duration as ChronoDuration, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
use uuid::Uuid;
use windows::{Executable, Process};

const SELECTION_TTL: Duration = Duration::from_secs(300);
const MAX_GAMES: usize = 100;
const MAX_PROCESSES: usize = 8192;

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
#[derive(Clone, Deserialize)]
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
    processes: HashMap<String, Vec<Process>>,
    previous: Option<(Instant, Snapshot)>,
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
        if let Some((at, previous)) = &self.previous {
            if at.elapsed() < Duration::from_secs(2) {
                return Ok(previous.clone());
            }
        }
        let ids: HashSet<_> = games.iter().map(|game| game.id.clone()).collect();
        self.processes.retain(|id, processes| {
            processes.retain(Process::alive);
            ids.contains(id)
        });
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
            for (pid, process) in listed.into_iter().take(MAX_PROCESSES) {
                let name = process.name().to_string_lossy().to_lowercase();
                if names.contains(&name) {
                    // A name is only a prefilter. The held process handle, creation
                    // time, canonical path, and file ID supply the actual match.
                    candidates.push((name, Process::open(pid.as_u32())));
                }
            }
        }
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
            let mut uncertain = !complete;
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
                        // Move the checked handle without reopening a potentially
                        // reused PID. Other registered files cannot share its ID.
                        if let Ok(process) = std::mem::replace(candidate, Err(String::new())) {
                            running.push(process);
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
            let message=match status {"unavailable"=>Some("Executable tidak tersedia atau identitasnya berubah. Daftarkan ulang file.".into()),"unverified"=>Some("Sebagian proses tidak dapat diverifikasi. Pencocokan nama saja tidak digunakan.".into()),_=>None};
            statuses.push(GameStatus {
                game,
                status: status.into(),
                process_count: running.len(),
                message,
            });
        }
        let snapshot = Snapshot {
            recorded_at: Utc::now().to_rfc3339(),
            status: if complete { "ready" } else { "degraded" }.into(),
            games: statuses,
        };
        self.previous = Some((Instant::now(), snapshot.clone()));
        Ok(snapshot)
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
            let first = root.join("one/ping.exe");
            let second = root.join("two/ping.exe");
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
