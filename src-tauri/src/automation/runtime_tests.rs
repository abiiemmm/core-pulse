use super::*;
use crate::{automation::Mode, db, games::Settings, models::PowerPlan};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
};
use uuid::Uuid;

const ORIGINAL: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
const TARGET: &str = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";
struct FakeState {
    current: String,
    source: String,
    calls: Vec<String>,
}
struct FakePower {
    state: Arc<Mutex<FakeState>>,
    database: Arc<Mutex<Connection>>,
}
impl PowerBackend for FakePower {
    fn plans(&mut self) -> Result<Vec<PowerPlan>, String> {
        Ok([ORIGINAL, TARGET]
            .iter()
            .map(|guid| PowerPlan {
                guid: (*guid).into(),
                name: (*guid).into(),
                active: *guid == self.state.lock().unwrap().current,
            })
            .collect())
    }
    fn active(&mut self) -> Result<String, String> {
        Ok(self.state.lock().unwrap().current.clone())
    }
    fn power_source(&mut self) -> String {
        self.state.lock().unwrap().source.clone()
    }
    fn set_active(&mut self, guid: &str) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(1);
        let conn = loop {
            if let Ok(conn) = self.database.try_lock() {
                break conn;
            }
            assert!(
                Instant::now() < deadline,
                "Native controller must release SQLite before OS mutation"
            );
            thread::sleep(Duration::from_millis(1));
        };
        let session = db::unfinished_sessions(&conn).unwrap().remove(0);
        assert_eq!(
            session.status,
            if guid == ORIGINAL {
                "restoring"
            } else {
                "pending"
            }
        );
        let linked: usize = conn
            .query_row(
                "SELECT COUNT(*) FROM gaming_sessions WHERE tuning_session_id=?1",
                [&session.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(linked > 0);
        drop(conn);
        let mut state = self.state.lock().unwrap();
        state.calls.push(guid.into());
        state.current = guid.into();
        Ok(())
    }
}
struct Fixture {
    root: PathBuf,
    file: PathBuf,
    game: String,
    database: Arc<Mutex<Connection>>,
    registry: Arc<Mutex<games::Registry>>,
    power: Arc<power::Controller>,
    state: Arc<Mutex<FakeState>>,
    service: Option<Service>,
    children: Vec<Child>,
}
impl Fixture {
    fn new(ac_only: bool) -> Self {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let root = project
            .join("artifacts/games")
            .join(Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        assert!(root.starts_with(std::fs::canonicalize(project).unwrap()));
        let file = root.join(format!("runtime-{}.exe", Uuid::new_v4()));
        std::fs::copy(
            PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/ping.exe"),
            &file,
        )
        .unwrap();
        let database = Arc::new(Mutex::new(db::open(Path::new(":memory:")).unwrap()));
        db::save_profile_mapping(&mut database.lock().unwrap(), "gaming", TARGET, false).unwrap();
        let registry = Arc::new(Mutex::new(games::Registry::default()));
        let settings = Settings {
            display_name: "Native runtime fixture".into(),
            profile_id: "gaming".into(),
            ac_only,
            restore_on_exit: true,
        };
        let selection = registry.lock().unwrap().selected(&file).unwrap();
        let game = registry
            .lock()
            .unwrap()
            .register(&database, &selection.selection_id, settings.clone())
            .unwrap();
        game.validate_file().unwrap();
        registry
            .lock()
            .unwrap()
            .set_auto(
                &database.lock().unwrap(),
                &game.id,
                true,
                &settings,
                &[PowerPlan {
                    guid: TARGET.into(),
                    name: "Simulated plan".into(),
                    active: false,
                }],
            )
            .unwrap();
        Self {
            root,
            file,
            game: game.id,
            database,
            registry,
            power: Arc::new(power::Controller::default()),
            state: Arc::new(Mutex::new(FakeState {
                current: ORIGINAL.into(),
                source: "AC power".into(),
                calls: vec![],
            })),
            service: None,
            children: vec![],
        }
    }
    fn backend(&self) -> FakePower {
        FakePower {
            state: self.state.clone(),
            database: self.database.clone(),
        }
    }
    fn start(&mut self) {
        let backend = self.backend();
        self.service = Some(Service::start_with(
            self.database.clone(),
            self.registry.clone(),
            self.power.clone(),
            |_, _| {},
            move || backend,
            Duration::from_millis(40),
            Duration::from_millis(20),
        ));
    }
    fn launch(&mut self) {
        self.children.push(
            Command::new(&self.file)
                .args(["-n", "60", "127.0.0.1"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
    }
    fn stop_child(&mut self, index: usize) {
        let _ = self.children[index].kill();
        let _ = self.children[index].wait();
    }
    fn wait(&self, predicate: impl Fn(&Service) -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while !predicate(self.service.as_ref().unwrap()) {
            assert!(
                Instant::now() < end,
                "Native runtime did not reach expected state"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(service) = &self.service {
            let _ = service.shutdown_with(&mut self.backend());
        }
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn native_discovery_tracks_overlap_and_restores_after_final_real_process_exit() {
    let mut f = Fixture::new(false);
    f.start();
    f.launch();
    f.launch();
    f.wait(|service| {
        service.status().tuning.mode == Mode::Active
            && service.game_snapshot().unwrap().games[0].process_count == 2
    });
    f.stop_child(0);
    f.wait(|service| service.game_snapshot().unwrap().games[0].process_count == 1);
    assert_eq!(f.state.lock().unwrap().calls, [TARGET]);
    f.stop_child(1);
    f.wait(|service| service.status().tuning.mode == Mode::Idle);
    assert_eq!(f.state.lock().unwrap().calls, [TARGET, ORIGINAL]);
    let sessions = games::sessions(&f.database.lock().unwrap()).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].status, "completed");
}

#[test]
fn ac_protection_and_manual_restore_remain_responsive_while_discovery_is_blocked() {
    for ac_loss in [false, true] {
        let mut f = Fixture::new(ac_loss);
        f.start();
        f.launch();
        f.wait(|service| service.status().tuning.mode == Mode::Active);
        let registry = f.registry.lock().unwrap();
        let session = f
            .service
            .as_ref()
            .unwrap()
            .status()
            .tuning
            .tuning_session_id
            .unwrap();
        let began = Instant::now();
        if ac_loss {
            f.state.lock().unwrap().source = "Battery".into();
            f.wait(|service| service.status().tuning.tuning_session_id.is_none());
        } else {
            f.service
                .as_ref()
                .unwrap()
                .configure(|controller| {
                    f.power
                        .restore_with(&f.database, &session, false, &mut f.backend())?;
                    controller.manually_restored(&session);
                    Ok(())
                })
                .unwrap();
        }
        assert!(began.elapsed() < Duration::from_secs(3));
        assert_eq!(f.state.lock().unwrap().calls, [TARGET, ORIGINAL]);
        assert!(db::unfinished_sessions(&f.database.lock().unwrap())
            .unwrap()
            .is_empty());
        assert!(f.children[0].try_wait().unwrap().is_none());
        drop(registry);
    }
}

#[test]
fn shutdown_restores_owned_profile_without_waiting_for_discovery_or_terminating_the_game() {
    let mut f = Fixture::new(false);
    f.start();
    f.launch();
    f.wait(|service| service.status().tuning.mode == Mode::Active);
    let registry = f.registry.lock().unwrap();
    let began = Instant::now();
    f.service
        .as_ref()
        .unwrap()
        .shutdown_with(&mut f.backend())
        .unwrap();
    assert!(began.elapsed() < Duration::from_secs(3));
    assert_eq!(f.state.lock().unwrap().calls, [TARGET, ORIGINAL]);
    assert!(f.children[0].try_wait().unwrap().is_none());
    assert_eq!(
        games::sessions(&f.database.lock().unwrap()).unwrap()[0].status,
        "app_closed"
    );
    drop(registry);
}

#[test]
fn stale_and_configuration_invalidated_observations_cannot_activate_a_live_game() {
    for stale in [false, true] {
        let mut f = Fixture::new(false);
        f.launch();
        let mut registry = f.registry.lock().unwrap();
        let games = {
            let conn = f.database.lock().unwrap();
            games::get_games(&conn).unwrap()
        };
        let observation = registry.observe(games).unwrap();
        assert_eq!(
            observation
                .view(games::get_games(&f.database.lock().unwrap()).unwrap())
                .games[0]
                .process_count,
            1
        );
        let (resume, wait) = mpsc::channel();
        let backend = f.backend();
        let service = Service::start_with(
            f.database.clone(),
            f.registry.clone(),
            f.power.clone(),
            |_, _| {},
            move || {
                wait.recv().unwrap();
                backend
            },
            Duration::from_millis(40),
            Duration::from_millis(20),
        );
        *service.cache.lock().unwrap() = Some(Arc::new(Discovery {
            started: if stale {
                Instant::now() - MAX_OBSERVATION_AGE - Duration::from_secs(1)
            } else {
                Instant::now()
            },
            revision: 0,
            observation,
        }));
        if !stale {
            service.invalidate();
        }
        resume.send(()).unwrap();
        f.service = Some(service);
        f.wait(|service| service.status().discovery_status == "stale");
        assert!(f.state.lock().unwrap().calls.is_empty());
        assert_eq!(
            f.service.as_ref().unwrap().game_snapshot().unwrap().games[0].status,
            "unverified"
        );
        drop(registry);
        f.wait(|service| service.status().tuning.mode == Mode::Active);
        assert_eq!(f.state.lock().unwrap().calls, [TARGET]);
        assert_eq!(
            games::get_games(&f.database.lock().unwrap()).unwrap()[0].id,
            f.game
        );
    }
}

#[test]
fn final_activation_gate_rechecks_held_liveness_and_expiration_after_pending_intent() {
    let mut f = Fixture::new(false);
    f.launch();
    let games = {
        let conn = f.database.lock().unwrap();
        games::get_games(&conn).unwrap()
    };
    let observation = f.registry.lock().unwrap().observe(games).unwrap();
    let session = Uuid::new_v4().to_string();
    f.database.lock().unwrap().execute("INSERT INTO gaming_sessions(id,game_id,started_at,status) VALUES(?1,?2,'2026-10-06T00:00:00Z','active')",rusqlite::params![session,f.game]).unwrap();
    let service = Service {
        database: f.database.clone(),
        power: f.power.clone(),
        controller: Arc::new(Mutex::new(Controller::default())),
        status: Arc::new(Mutex::new(RuntimeStatus {
            tuning: Status::default(),
            discovery_status: "waiting".into(),
            discovery_message: None,
        })),
        cache: Arc::new(Mutex::new(None)),
        discovery_error: Arc::new(Mutex::new(None)),
        revision: Arc::new(AtomicU64::new(0)),
        stopped: Arc::new(AtomicBool::new(false)),
        publish: Arc::new(|_, _| {}),
    };
    let mut discovery = Discovery {
        started: Instant::now(),
        revision: 0,
        observation,
    };
    let mut backend = f.backend();
    assert!(GuardedBackend {
        backend: &mut backend,
        discovery: &discovery,
        service: &service
    }
    .verify_game_activation(std::slice::from_ref(&session))
    .is_ok());
    f.stop_child(0);
    assert!(GuardedBackend {
        backend: &mut backend,
        discovery: &discovery,
        service: &service
    }
    .verify_game_activation(std::slice::from_ref(&session))
    .is_err());
    discovery.started = Instant::now() - MAX_OBSERVATION_AGE - Duration::from_secs(1);
    assert!(GuardedBackend {
        backend: &mut backend,
        discovery: &discovery,
        service: &service
    }
    .verify_game_observation()
    .is_err());
    assert!(f.state.lock().unwrap().calls.is_empty());
}
