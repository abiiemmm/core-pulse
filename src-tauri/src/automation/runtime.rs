use super::{Controller, Status};
use crate::{
    games::{self, Observation, RegisteredGame, Snapshot},
    models::TuningSession,
    power::{self, PowerBackend},
};
use rusqlite::Connection;
use serde::Serialize;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const MAX_OBSERVATION_AGE: Duration = Duration::from_secs(5);

#[derive(Clone, Serialize)]
pub struct RuntimeStatus {
    #[serde(flatten)]
    pub tuning: Status,
    pub discovery_status: String,
    pub discovery_message: Option<String>,
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
struct Discovery {
    started: Instant,
    revision: u64,
    observation: Observation,
}
struct GuardedBackend<'a, B> {
    backend: &'a mut B,
    discovery: &'a Discovery,
    service: &'a Service,
}
impl<B: PowerBackend> PowerBackend for GuardedBackend<'_, B> {
    fn plans(&mut self) -> Result<Vec<crate::models::PowerPlan>, String> {
        self.backend.plans()
    }
    fn active(&mut self) -> Result<String, String> {
        self.backend.active()
    }
    fn set_active(&mut self, guid: &str) -> Result<(), String> {
        self.backend.set_active(guid)
    }
    fn power_source(&mut self) -> String {
        self.backend.power_source()
    }
    fn verify_game_observation(&mut self) -> Result<(), String> {
        if self.discovery.started.elapsed() > MAX_OBSERVATION_AGE
            || self.discovery.revision != self.service.revision.load(Ordering::Acquire)
        {
            return Err("Pengamatan game kedaluwarsa. Menunggu pemeriksaan proses baru.".into());
        }
        self.backend.verify_game_observation()
    }
    fn verify_game_activation(&mut self, ids: &[String]) -> Result<(), String> {
        self.verify_game_observation()?;
        let (games, profiles, participants) = {
            let conn = self
                .service
                .database
                .lock()
                .map_err(|_| "Database tidak tersedia")?;
            let mut participants = vec![];
            for id in ids {
                let game: String = conn
                    .query_row(
                        "SELECT game_id FROM gaming_sessions WHERE id=?1 AND status='active'",
                        [id],
                        |row| row.get(0),
                    )
                    .map_err(|_| "Sesi game tidak lagi memenuhi syarat Auto Boost")?;
                participants.push(game);
            }
            (
                games::get_games(&conn)?,
                crate::db::profiles(&conn)?,
                participants,
            )
        };
        // Database reads can wait; repeat the deadline after releasing SQLite,
        // then read current power source and the held process handles.
        self.verify_game_observation()?;
        let source = self.backend.power_source();
        let live = games.iter().any(|game| {
            game.auto_boost
                && participants.contains(&game.id)
                && profiles
                    .iter()
                    .find(|profile| profile.id == game.profile_id)
                    .is_some_and(|profile| {
                        profile
                            .scheme_guid
                            .as_deref()
                            .is_some_and(|guid| !guid.is_empty())
                            && (!(profile.ac_only || game.ac_only) || source == "AC power")
                    })
                && self.discovery.observation.has_live_game(game)
        });
        if !live {
            return Err("Game tidak lagi terverifikasi aktif untuk Auto Boost".into());
        }
        self.backend.verify_game_activation(ids)
    }
}
type Publisher = dyn Fn(RuntimeStatus, Option<TuningSession>) + Send + Sync;

#[derive(Clone)]
pub struct Service {
    database: Arc<Mutex<Connection>>,
    power: Arc<power::Controller>,
    controller: Arc<Mutex<Controller>>,
    status: Arc<Mutex<RuntimeStatus>>,
    cache: Arc<Mutex<Option<Arc<Discovery>>>>,
    discovery_error: Arc<Mutex<Option<String>>>,
    revision: Arc<AtomicU64>,
    stopped: Arc<AtomicBool>,
    publish: Arc<Publisher>,
}
impl Service {
    pub fn start(
        database: Arc<Mutex<Connection>>,
        registry: Arc<Mutex<games::Registry>>,
        power: Arc<power::Controller>,
        publish: impl Fn(RuntimeStatus, Option<TuningSession>) + Send + Sync + 'static,
    ) -> Self {
        Self::start_with(
            database,
            registry,
            power,
            publish,
            || power::WindowsPower,
            Duration::from_secs(2),
            Duration::from_secs(1),
        )
    }

    fn start_with<B: PowerBackend + Send + 'static>(
        database: Arc<Mutex<Connection>>,
        registry: Arc<Mutex<games::Registry>>,
        power: Arc<power::Controller>,
        publish: impl Fn(RuntimeStatus, Option<TuningSession>) + Send + Sync + 'static,
        backend: impl FnOnce() -> B + Send + 'static,
        discovery_interval: Duration,
        control_interval: Duration,
    ) -> Self {
        let service = Self {
            database,
            power,
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
            publish: Arc::new(publish),
        };
        let discovery = service.clone();
        thread::spawn(move || {
            while !discovery.stopped.load(Ordering::Acquire) {
                let started = Instant::now();
                let revision = discovery.revision.load(Ordering::Acquire);
                let result = (|| {
                    let mut registry = registry.lock().map_err(|_| "Daftar game tidak tersedia")?;
                    let games = discovery.registrations()?;
                    // Neither SQLite nor the power/controller operation lock is
                    // held across executable inspection or process enumeration.
                    registry.observe(games)
                })();
                if discovery.stopped.load(Ordering::Acquire) {
                    break;
                }
                match result {
                    Ok(observation) => {
                        *discovery.cache.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(Arc::new(Discovery {
                                started,
                                revision,
                                observation,
                            }));
                        *discovery
                            .discovery_error
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = None;
                    }
                    Err(error) => {
                        *discovery
                            .discovery_error
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(error)
                    }
                }
                thread::sleep(discovery_interval);
            }
        });
        let safety = service.clone();
        thread::spawn(move || {
            let mut backend = backend();
            let mut processed = None;
            while !safety.stopped.load(Ordering::Acquire) {
                {
                    let mut controller =
                        safety.controller.lock().unwrap_or_else(|e| e.into_inner());
                    if safety.stopped.load(Ordering::Acquire) {
                        break;
                    }
                    // AC protection and outside-change detection run every second,
                    // even if discovery is hung, stale, paused or inaccessible.
                    let _ = controller.protect(&safety.database, &safety.power, &mut backend);
                    let cached = safety
                        .cache
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone();
                    if let Some(cached) = cached.filter(|cached| {
                        cached.started.elapsed() <= MAX_OBSERVATION_AGE
                            && cached.revision == safety.revision.load(Ordering::Acquire)
                            && processed != Some(cached.started)
                    }) {
                        match safety.registrations() {
                            Ok(games) => {
                                // Held handles are checked again here without
                                // reopening files or trusting a cached PID/name.
                                let snapshot = cached.observation.view(games);
                                let mut guarded = GuardedBackend {
                                    backend: &mut backend,
                                    discovery: &cached,
                                    service: &safety,
                                };
                                let _ = controller.tick(
                                    &snapshot,
                                    &safety.database,
                                    &safety.power,
                                    &mut guarded,
                                );
                                processed = Some(cached.started);
                            }
                            Err(error) => {
                                *safety
                                    .discovery_error
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner()) = Some(error)
                            }
                        }
                    }
                    safety.refresh_status(controller.status());
                }
                thread::sleep(control_interval);
            }
        });
        service
    }

    fn registrations(&self) -> Result<Vec<RegisteredGame>, String> {
        let conn = self
            .database
            .lock()
            .map_err(|_| "Database tidak tersedia")?;
        games::get_games(&conn)
    }
    pub fn status(&self) -> RuntimeStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn invalidate(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    // Configuration callers acquire the registry before this lock, and must
    // finish executable validation beforehand. Manual restore needs no registry.
    pub fn configure<T>(
        &self,
        operation: impl FnOnce(&mut Controller) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut controller = self
            .controller
            .lock()
            .map_err(|_| "Pengendali Auto Boost tidak tersedia")?;
        if self.stopped.load(Ordering::Acquire) {
            return Err("Aplikasi sedang ditutup".into());
        }
        let result = operation(&mut controller)?;
        self.invalidate();
        self.refresh_status(controller.status());
        Ok(result)
    }

    pub fn game_snapshot(&self) -> Result<Snapshot, String> {
        let games = self.registrations()?;
        let cached = self
            .cache
            .lock()
            .map_err(|_| "Status game tidak tersedia")?
            .clone();
        if let Some(cached) = cached.filter(|cached| {
            cached.started.elapsed() <= MAX_OBSERVATION_AGE
                && cached.revision == self.revision.load(Ordering::Acquire)
        }) {
            return Ok(cached.observation.view(games));
        }
        Ok(Snapshot {
            recorded_at: chrono::Utc::now().to_rfc3339(),
            status: "degraded".into(),
            games: games
                .into_iter()
                .map(|game| games::GameStatus {
                    game,
                    status: "unverified".into(),
                    process_count: 0,
                    message: None,
                })
                .collect(),
        })
    }

    fn refresh_status(&self, tuning: Status) {
        let error = self
            .discovery_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let discovery_status = if error.is_some() {
            "error"
        } else {
            match cache {
                None => "waiting",
                Some(cached)
                    if cached.started.elapsed() > MAX_OBSERVATION_AGE
                        || cached.revision != self.revision.load(Ordering::Acquire) =>
                {
                    "stale"
                }
                Some(_) => "ready",
            }
        };
        let next = RuntimeStatus {
            tuning,
            discovery_status: discovery_status.into(),
            discovery_message: error,
        };
        let previous = {
            let mut stored = self.status.lock().unwrap_or_else(|e| e.into_inner());
            let previous = stored.clone();
            *stored = next.clone();
            previous
        };
        let changed = previous.tuning.mode != next.tuning.mode
            || previous.tuning.tuning_session_id != next.tuning.tuning_session_id
            || previous.tuning.relevant_games != next.tuning.relevant_games
            || previous.tuning.reason != next.tuning.reason
            || previous.tuning.error != next.tuning.error
            || previous.discovery_status != next.discovery_status
            || previous.discovery_message != next.discovery_message;
        if changed {
            let session_id = next
                .tuning
                .tuning_session_id
                .as_ref()
                .or(previous.tuning.tuning_session_id.as_ref());
            let session = session_id.and_then(|id| {
                self.database
                    .lock()
                    .ok()
                    .and_then(|conn| power::load_session(&conn, id).ok())
            });
            (self.publish)(next, session);
        }
    }

    pub fn shutdown(&self) -> Result<(), String> {
        self.shutdown_with(&mut power::WindowsPower)
    }
    fn shutdown_with(&self, backend: &mut impl PowerBackend) -> Result<(), String> {
        self.stopped.store(true, Ordering::Release);
        let mut controller = self
            .controller
            .lock()
            .map_err(|_| "Pengendali Auto Boost tidak tersedia")?;
        let result = controller.shutdown(&self.database, &self.power, backend);
        self.refresh_status(controller.status());
        result
    }
}
