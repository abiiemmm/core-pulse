mod runtime;
pub use runtime::{RuntimeStatus, Service};

// One global game-owned tuning session. The runtime worker and opt-in UI will
// call this controller under one operation lock; it never trusts process names.
use crate::{
    db,
    games::Snapshot,
    models::TuningSession,
    power::{self, PowerBackend},
};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Idle,
    Active,
    WaitingForAc,
    Unmapped,
    RecoveryRequired,
    Suspended,
    Retained,
    Error,
}

#[derive(Clone, Serialize)]
pub struct Status {
    pub mode: Mode,
    pub tuning_session_id: Option<String>,
    pub profile_id: Option<String>,
    pub relevant_games: usize,
    pub reason: Option<String>,
    pub error: Option<String>,
    pub updated_at: String,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            mode: Mode::Idle,
            tuning_session_id: None,
            profile_id: None,
            relevant_games: 0,
            reason: None,
            error: None,
            updated_at: Utc::now().to_rfc3339(),
        }
    }
}
struct Active {
    session: TuningSession,
    ac_only: bool,
    restore_on_exit: bool,
    linked: HashSet<String>,
}
#[derive(Default)]
pub struct Controller {
    initialized: bool,
    // One history session per verified game, across overlapping processes.
    observed: HashMap<String, String>,
    cycle: HashSet<String>,
    pending_suppression: bool,
    active: Option<Active>,
    // Manual restore, outside changes or failed mutations suppress this cycle.
    // A new launch can try again only after all relevant games have exited.
    blocked: Option<String>,
    status: Status,
}
impl Controller {
    pub fn status(&self) -> Status {
        self.status.clone()
    }

    pub fn tick(
        &mut self,
        snapshot: &Snapshot,
        database: &Mutex<Connection>,
        power: &power::Controller,
        backend: &mut impl PowerBackend,
    ) -> Result<Status, String> {
        match self.advance(snapshot, database, power, backend) {
            Ok(()) => {
                if self.blocked.is_none() {
                    self.status.error = None;
                }
                Ok(self.status())
            }
            Err(error) => {
                self.status.error = Some(error.clone());
                self.describe(Mode::Error, self.blocked.clone());
                Err(error)
            }
        }
    }

    fn advance(
        &mut self,
        snapshot: &Snapshot,
        database: &Mutex<Connection>,
        power: &power::Controller,
        backend: &mut impl PowerBackend,
    ) -> Result<(), String> {
        backend.verify_game_observation()?;
        self.sync_sessions(snapshot, database)?;
        backend.verify_game_observation()?;
        let observed_opt_in: Vec<_> = snapshot
            .games
            .iter()
            .filter(|row| row.game.auto_boost && self.observed.contains_key(&row.game.id))
            .collect();
        self.cycle.retain(|game| {
            self.observed.contains_key(game)
                || snapshot
                    .games
                    .iter()
                    .any(|row| &row.game.id == game && row.status != "ready")
        });
        if self.pending_suppression {
            // A restore may precede the first watcher observation. Apply its
            // cancellation to the next verified snapshot, including unknown
            // candidates until a complete observation proves they are absent.
            self.cycle.extend(
                snapshot
                    .games
                    .iter()
                    .filter(|row| row.game.auto_boost && row.status != "ready")
                    .map(|row| row.game.id.clone()),
            );
            self.pending_suppression = false;
        }
        if self.cycle.is_empty() && self.active.is_none() {
            self.blocked = None;
        }
        let profiles = {
            let conn = database.lock().map_err(|_| "Database tidak tersedia")?;
            db::profiles(&conn)?
        };
        let source = if self.active.is_some() || !observed_opt_in.is_empty() {
            backend.power_source()
        } else {
            String::new()
        };
        let mut waiting_ac = false;
        let relevant = observed_opt_in
            .iter()
            .copied()
            .filter(|row| {
                let Some(profile) = profiles
                    .iter()
                    .find(|profile| profile.id == row.game.profile_id)
                else {
                    return false;
                };
                if profile.scheme_guid.as_deref().is_none_or(str::is_empty) {
                    return false;
                }
                if (row.game.ac_only || profile.ac_only) && source != "AC power" {
                    waiting_ac = true;
                    return false;
                }
                true
            })
            .collect::<Vec<_>>();
        self.status.relevant_games = relevant.len();

        if self.protect_inner(database, power, backend, Some(&source))? {
            return Ok(());
        }
        if self.blocked.is_some() {
            self.describe(
                if self.active.is_some() {
                    Mode::RecoveryRequired
                } else {
                    Mode::Suspended
                },
                self.blocked.clone(),
            );
            return Ok(());
        }

        if let Some(active) = &self.active {
            if relevant.is_empty() {
                if !active.restore_on_exit {
                    self.describe(Mode::Retained, Some("manual_restore_required".into()));
                    return Ok(());
                }
                // On failure retain ownership but stop automatic mutation retries;
                // the durable session stays visible for explicit recovery.
                self.blocked = Some("recovery_required".into());
                power.restore_with(database, &active.session.id, false, backend)?;
                self.active = None;
                self.blocked = if self.cycle.is_empty() {
                    None
                } else {
                    Some("cycle_finished".into())
                };
                self.describe(
                    if self.blocked.is_some() {
                        Mode::Suspended
                    } else {
                        Mode::Idle
                    },
                    self.blocked.clone(),
                );
                return Ok(());
            }
            let unlinked = relevant
                .iter()
                .map(|row| self.observed[&row.game.id].clone())
                .filter(|id| !active.linked.contains(id))
                .collect::<Vec<_>>();
            if !unlinked.is_empty() {
                self.link_participants(database, &active.session.id, &unlinked)?;
                self.active.as_mut().unwrap().linked.extend(unlinked);
            }
            self.cycle
                .extend(relevant.iter().map(|row| row.game.id.clone()));
            self.describe(Mode::Active, None);
            return Ok(());
        }

        let unfinished = {
            let conn = database.lock().map_err(|_| "Database tidak tersedia")?;
            db::unfinished_sessions(&conn)?
        };
        if !unfinished.is_empty() && !observed_opt_in.is_empty() {
            self.cycle
                .extend(observed_opt_in.iter().map(|row| row.game.id.clone()));
            self.describe(
                Mode::RecoveryRequired,
                Some("unfinished_tuning_session".into()),
            );
            return Ok(());
        }
        if relevant.is_empty() {
            self.describe(
                if waiting_ac {
                    Mode::WaitingForAc
                } else if !observed_opt_in.is_empty() {
                    Mode::Unmapped
                } else {
                    Mode::Idle
                },
                if waiting_ac {
                    Some("ac_required".into())
                } else if !observed_opt_in.is_empty() {
                    Some("no_eligible_profile".into())
                } else {
                    None
                },
            );
            return Ok(());
        }
        let plans = backend.plans()?;
        // Snapshot order is registration time then ID. The first eligible
        // verified game pins this profile and both policy flags for the cycle.
        for row in &relevant {
            if row.status != "running" || row.process_count == 0 {
                continue;
            }
            let Some(profile) = profiles
                .iter()
                .find(|profile| profile.id == row.game.profile_id)
            else {
                continue;
            };
            let Some(guid) = profile.scheme_guid.as_deref() else {
                continue;
            };
            if !plans.iter().any(|plan| plan.guid == guid) {
                continue;
            }
            let ac_only = profile.ac_only || row.game.ac_only;
            if ac_only && source != "AC power" {
                waiting_ac = true;
                continue;
            }
            let ids = relevant
                .iter()
                .filter(|participant| {
                    participant.status == "running" && participant.process_count > 0
                })
                .map(|participant| self.observed[&participant.game.id].clone())
                .collect::<Vec<_>>();
            // Record both pending power intent and game linkage before any OS
            // mutation. A failure is visible and suppressed for this game cycle.
            self.blocked = Some("activation_failed".into());
            self.cycle
                .extend(relevant.iter().map(|row| row.game.id.clone()));
            let session =
                power.activate_game_with(database, &profile.id, ac_only, &ids, backend)?;
            self.active = Some(Active {
                session,
                ac_only,
                restore_on_exit: row.game.restore_on_exit,
                linked: ids.into_iter().collect(),
            });
            self.blocked = None;
            self.describe(Mode::Active, None);
            return Ok(());
        }
        self.describe(
            if waiting_ac {
                Mode::WaitingForAc
            } else {
                Mode::Unmapped
            },
            Some(
                if waiting_ac {
                    "ac_required"
                } else {
                    "no_eligible_profile"
                }
                .into(),
            ),
        );
        Ok(())
    }

    pub fn protect(
        &mut self,
        database: &Mutex<Connection>,
        power: &power::Controller,
        backend: &mut impl PowerBackend,
    ) -> Result<Status, String> {
        match self.protect_inner(database, power, backend, None) {
            Ok(_) => Ok(self.status()),
            Err(error) => {
                self.status.error = Some(error.clone());
                self.describe(Mode::Error, self.blocked.clone());
                Err(error)
            }
        }
    }
    fn protect_inner(
        &mut self,
        database: &Mutex<Connection>,
        power: &power::Controller,
        backend: &mut impl PowerBackend,
        source: Option<&str>,
    ) -> Result<bool, String> {
        if let Some(active) = &self.active {
            let stored = {
                let conn = database.lock().map_err(|_| "Database tidak tersedia")?;
                power::load_session(&conn, &active.session.id)?
            };
            if stored.status == "restored" {
                self.active = None;
                self.blocked = Some("manual_restore".into());
            } else if stored.status != "active" {
                self.blocked = Some("recovery_required".into());
                self.describe(Mode::RecoveryRequired, self.blocked.clone());
                return Ok(true);
            }
        }
        if let Some(active) = &self.active {
            if self.blocked.is_some() {
                self.describe(Mode::RecoveryRequired, self.blocked.clone());
                return Ok(true);
            }
            let current = backend.active()?;
            if current != active.session.applied_guid {
                // The existing restoration protocol records conflicts and never
                // forces over an outside change. Returning to the original GUID
                // is also treated as cancellation, with no reapplication.
                self.blocked = Some("external_change".into());
                let restored = power.restore_with(database, &active.session.id, false, backend)?;
                if restored.status == "restored" {
                    self.active = None;
                }
                self.describe(Mode::Suspended, self.blocked.clone());
                return Ok(true);
            }
            if active.ac_only
                && source
                    .map(str::to_owned)
                    .unwrap_or_else(|| backend.power_source())
                    != "AC power"
            {
                // AC protection wins over restore-on-exit=false. Do not silently
                // rearm while this same game cycle remains running.
                self.blocked = Some("ac_lost".into());
                power.restore_with(database, &active.session.id, false, backend)?;
                self.active = None;
                self.describe(Mode::WaitingForAc, self.blocked.clone());
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn describe(&mut self, mode: Mode, reason: Option<String>) {
        self.status.mode = mode;
        self.status.reason = reason;
        self.status.updated_at = Utc::now().to_rfc3339();
        self.status.tuning_session_id =
            self.active.as_ref().map(|active| active.session.id.clone());
        self.status.profile_id = self
            .active
            .as_ref()
            .map(|active| active.session.profile_id.clone());
    }

    fn sync_sessions(
        &mut self,
        snapshot: &Snapshot,
        database: &Mutex<Connection>,
    ) -> Result<(), String> {
        let starts = snapshot
            .games
            .iter()
            .filter(|row| {
                row.status == "running"
                    && row.process_count > 0
                    && !self.observed.contains_key(&row.game.id)
            })
            .map(|row| (row.game.id.clone(), Uuid::new_v4().to_string()))
            .collect::<Vec<_>>();
        let ends = self
            .observed
            .iter()
            .filter_map(|(game, session)| {
                match snapshot.games.iter().find(|row| &row.game.id == game) {
                    // Zero matched handles alone is insufficient when enumeration or
                    // the executable cannot be verified. Keep the prior session.
                    Some(row) if row.status == "ready" && row.process_count == 0 => {
                        Some((game.clone(), session.clone(), "completed"))
                    }
                    None => Some((game.clone(), session.clone(), "removed")),
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
        if self.initialized && starts.is_empty() && ends.is_empty() {
            return Ok(());
        }
        let mut conn = database.lock().map_err(|_| "Database tidak tersedia")?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let at = Utc::now().to_rfc3339();
        if !self.initialized {
            // Prior runtime sessions never imply current process ownership or
            // grant permission to recover a prior tuning transaction silently.
            tx.execute(
                "UPDATE gaming_sessions SET status='interrupted',ended_at=?1 WHERE status='active'",
                [&at],
            )
            .map_err(|error| error.to_string())?;
        }
        for (game, session) in &starts {
            let changed = tx.execute("INSERT INTO gaming_sessions(id,game_id,started_at,status) SELECT ?1,id,?3,'active' FROM registered_games WHERE id=?2 AND archived=0 AND executable_identity!=''", params![session,game,at]).map_err(|error| error.to_string())?;
            if changed != 1 {
                return Err("Game tidak lagi tersedia untuk pencatatan sesi".into());
            }
        }
        for (_, session, status) in &ends {
            let changed = tx.execute("UPDATE gaming_sessions SET status=?2,ended_at=?3 WHERE id=?1 AND status='active'", params![session,status,at]).map_err(|error| error.to_string())?;
            if changed != 1 {
                return Err("Sesi game berubah saat diperbarui".into());
            }
        }
        tx.commit().map_err(|error| error.to_string())?;
        self.initialized = true;
        self.observed.extend(starts);
        for (game, _, _) in ends {
            self.observed.remove(&game);
        }
        if let Some(active) = &mut self.active {
            active
                .linked
                .retain(|id| self.observed.values().any(|current| current == id));
        }
        Ok(())
    }

    fn link_participants(
        &self,
        database: &Mutex<Connection>,
        tuning: &str,
        ids: &[String],
    ) -> Result<(), String> {
        let mut conn = database.lock().map_err(|_| "Database tidak tersedia")?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        for id in ids {
            tx.execute("UPDATE gaming_sessions SET tuning_session_id=?2 WHERE id=?1 AND status='active' AND tuning_session_id IS NULL", params![id,tuning]).map_err(|error| error.to_string())?;
        }
        tx.commit().map_err(|error| error.to_string())
    }

    // Called after a successful explicit restore, under the same outer operation
    // lock as tick. Keep the current game cycle from immediately applying again.
    pub fn manually_restored(&mut self, session_id: &str) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.session.id == session_id)
            || self.active.is_none()
        {
            self.active = None;
            self.blocked = Some("manual_restore".into());
            self.pending_suppression = true;
            self.status.error = None;
            self.describe(Mode::Suspended, self.blocked.clone());
        }
    }

    pub fn shutdown(
        &mut self,
        database: &Mutex<Connection>,
        power: &power::Controller,
        backend: &mut impl PowerBackend,
    ) -> Result<(), String> {
        if let Some(active) = &self.active {
            if active.restore_on_exit {
                power.restore_with(database, &active.session.id, false, backend)?;
                self.active = None;
            }
        }
        let conn = database.lock().map_err(|_| "Database tidak tersedia")?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|error| error.to_string())?;
        let at = Utc::now().to_rfc3339();
        for session in self.observed.values() {
            tx.execute("UPDATE gaming_sessions SET status='app_closed',ended_at=?2 WHERE id=?1 AND status='active'", params![session,at]).map_err(|error| error.to_string())?;
        }
        tx.commit().map_err(|error| error.to_string())?;
        self.observed.clear();
        self.describe(
            if self.active.is_some() {
                Mode::Retained
            } else {
                Mode::Idle
            },
            None,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        games::{GameStatus, RegisteredGame},
        models::PowerPlan,
    };
    use std::{collections::VecDeque, path::Path, sync::Arc};

    const ORIGINAL: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
    const TARGET: &str = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";
    const OUTSIDE: &str = "a1841308-3541-4fab-bc81-f71556f20b4a";

    struct FakePower {
        database: Arc<Mutex<Connection>>,
        current: String,
        source: String,
        sources: VecDeque<String>,
        calls: Vec<String>,
        denied: bool,
        panic_after_apply: bool,
    }
    impl PowerBackend for FakePower {
        fn plans(&mut self) -> Result<Vec<PowerPlan>, String> {
            Ok([ORIGINAL, TARGET, OUTSIDE]
                .iter()
                .map(|guid| PowerPlan {
                    guid: (*guid).into(),
                    name: (*guid).into(),
                    active: *guid == self.current,
                })
                .collect())
        }
        fn active(&mut self) -> Result<String, String> {
            Ok(self.current.clone())
        }
        fn power_source(&mut self) -> String {
            self.sources
                .pop_front()
                .unwrap_or_else(|| self.source.clone())
        }
        fn set_active(&mut self, guid: &str) -> Result<(), String> {
            let conn = self
                .database
                .try_lock()
                .expect("Windows work must not hold SQLite");
            let unfinished = db::unfinished_sessions(&conn).unwrap();
            assert_eq!(unfinished.len(), 1);
            assert_eq!(
                unfinished[0].status,
                if guid == ORIGINAL {
                    "restoring"
                } else {
                    "pending"
                }
            );
            let trigger: String = conn
                .query_row(
                    "SELECT trigger FROM tuning_sessions WHERE id=?1",
                    [&unfinished[0].id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(trigger, "auto_game");
            let linked: usize = conn
                .query_row(
                    "SELECT COUNT(*) FROM gaming_sessions WHERE tuning_session_id=?1",
                    [&unfinished[0].id],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(linked > 0, "game linkage must be durable before mutation");
            drop(conn);
            self.calls.push(guid.into());
            if self.denied {
                self.denied = false;
                return Err("simulated permission denied".into());
            }
            self.current = guid.into();
            if self.panic_after_apply {
                self.panic_after_apply = false;
                panic!("simulated process interruption");
            }
            Ok(())
        }
    }
    struct Fixture {
        database: Arc<Mutex<Connection>>,
        engine: Controller,
        power: power::Controller,
        backend: FakePower,
        snapshot: Snapshot,
    }
    impl Fixture {
        fn new() -> Self {
            let mut conn = db::open(Path::new(":memory:")).unwrap();
            for (id, guid) in [
                ("gaming", TARGET),
                ("balanced", ORIGINAL),
                ("saving", OUTSIDE),
            ] {
                db::save_profile_mapping(&mut conn, id, guid, false).unwrap();
            }
            let database = Arc::new(Mutex::new(conn));
            let backend = FakePower {
                database: database.clone(),
                current: ORIGINAL.into(),
                source: "AC power".into(),
                sources: VecDeque::new(),
                calls: vec![],
                denied: false,
                panic_after_apply: false,
            };
            Self {
                database,
                engine: Controller::default(),
                power: power::Controller::default(),
                backend,
                snapshot: Snapshot {
                    recorded_at: Utc::now().to_rfc3339(),
                    status: "ready".into(),
                    games: vec![],
                },
            }
        }
        fn game(
            &mut self,
            profile: &str,
            opted: bool,
            ac_only: bool,
            restore: bool,
            count: usize,
        ) -> String {
            let id = Uuid::new_v4().to_string();
            let file = format!("C:\\qa\\{id}.exe");
            let at = Utc::now().to_rfc3339();
            self.database.lock().unwrap().execute("INSERT INTO registered_games(id,display_name,canonical_executable_path,executable_identity,profile_id,auto_boost,restore_on_exit,ac_only,created_at,updated_at) VALUES(?1,'QA',?2,?1,?3,?4,?5,?6,?7,?7)",params![id,file,profile,opted,restore,ac_only,at]).unwrap();
            self.snapshot.games.push(GameStatus {
                game: RegisteredGame {
                    id: id.clone(),
                    display_name: "QA".into(),
                    canonical_executable_path: file,
                    executable_identity: id.clone(),
                    profile_id: profile.into(),
                    auto_boost: opted,
                    restore_on_exit: restore,
                    ac_only,
                },
                status: if count > 0 { "running" } else { "ready" }.into(),
                process_count: count,
                message: None,
            });
            id
        }
        fn row(&mut self, id: &str) -> &mut GameStatus {
            self.snapshot
                .games
                .iter_mut()
                .find(|row| row.game.id == id)
                .unwrap()
        }
        fn count(&mut self, id: &str, count: usize) {
            let row = self.row(id);
            row.process_count = count;
            row.status = if count > 0 { "running" } else { "ready" }.into();
        }
        fn tick(&mut self) -> Result<Status, String> {
            self.engine.tick(
                &self.snapshot,
                &self.database,
                &self.power,
                &mut self.backend,
            )
        }
        fn unfinished(&self) -> Vec<TuningSession> {
            db::unfinished_sessions(&self.database.lock().unwrap()).unwrap()
        }
        fn history_count(&self, status: &str) -> usize {
            self.database
                .lock()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM gaming_sessions WHERE status=?1",
                    [status],
                    |row| row.get(0),
                )
                .unwrap()
        }
    }

    #[test]
    fn default_off_and_unverified_names_never_apply_but_verified_sessions_are_recorded() {
        let mut f = Fixture::new();
        let off = f.game("gaming", false, false, true, 1);
        let unknown = f.game("gaming", true, false, true, 0);
        f.row(&unknown).status = "unverified".into();
        assert_eq!(f.tick().unwrap().mode, Mode::Idle);
        assert!(f.backend.calls.is_empty());
        assert_eq!(f.history_count("active"), 1);
        f.count(&off, 0);
        f.tick().unwrap();
        assert_eq!(f.history_count("completed"), 1);
        assert!(f.unfinished().is_empty());
    }

    #[test]
    fn overlap_pins_first_profile_and_never_restores_while_another_process_or_game_remains() {
        let mut f = Fixture::new();
        let first = f.game("gaming", true, false, true, 2);
        let initial = f.tick().unwrap();
        let second = f.game("saving", true, false, false, 1);
        for _ in 0..3 {
            assert_eq!(
                f.tick().unwrap().tuning_session_id,
                initial.tuning_session_id
            );
        }
        assert_eq!(f.backend.calls, [TARGET]);
        f.count(&first, 1);
        f.tick().unwrap();
        f.count(&first, 0);
        let ongoing = f.tick().unwrap();
        assert_eq!(ongoing.profile_id.as_deref(), Some("gaming"));
        assert_eq!(f.backend.calls, [TARGET]);
        assert_eq!(f.history_count("active"), 1);
        let linked: usize = f
            .database
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM gaming_sessions WHERE tuning_session_id=?1",
                [&initial.tuning_session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(linked, 2);
        f.count(&second, 0);
        assert_eq!(f.tick().unwrap().mode, Mode::Idle);
        for _ in 0..3 {
            f.tick().unwrap();
        }
        assert_eq!(f.backend.calls, [TARGET, ORIGINAL]);
        assert!(f.unfinished().is_empty());
        assert_eq!(f.history_count("completed"), 2);
    }

    #[test]
    fn simultaneous_launches_use_registration_order_and_skip_unmapped_or_ac_ineligible_games() {
        let mut f = Fixture::new();
        db::save_profile_mapping(&mut f.database.lock().unwrap(), "balanced", "", false).unwrap();
        f.game("balanced", true, false, true, 1);
        f.game("gaming", true, true, true, 1);
        f.game("saving", true, false, true, 1);
        f.backend.source = "Battery".into();
        assert_eq!(f.tick().unwrap().profile_id.as_deref(), Some("saving"));
        assert_eq!(f.backend.calls, [OUTSIDE]);
    }

    #[test]
    fn uncertain_discovery_never_signals_final_exit_or_creates_another_session() {
        let mut f = Fixture::new();
        let id = f.game("gaming", true, false, true, 1);
        f.tick().unwrap();
        f.row(&id).process_count = 0;
        for status in ["unverified", "unavailable"] {
            f.row(&id).status = status.into();
            assert_eq!(f.tick().unwrap().mode, Mode::Active);
            assert_eq!(f.history_count("active"), 1);
            assert_eq!(f.backend.calls, [TARGET]);
        }
        f.count(&id, 0);
        f.tick().unwrap();
        assert_eq!(f.backend.calls, [TARGET, ORIGINAL]);
    }

    #[test]
    fn ac_is_required_at_start_and_loss_restores_even_when_restore_on_exit_is_off() {
        for ac_in_profile in [false, true] {
            let mut f = Fixture::new();
            db::save_profile_mapping(
                &mut f.database.lock().unwrap(),
                "gaming",
                TARGET,
                ac_in_profile,
            )
            .unwrap();
            let id = f.game("gaming", true, !ac_in_profile, false, 1);
            for source in ["Battery", "Unknown"] {
                f.backend.source = source.into();
                assert_eq!(f.tick().unwrap().mode, Mode::WaitingForAc);
                assert!(f.backend.calls.is_empty());
            }
            f.backend.source = "AC power".into();
            f.tick().unwrap();
            assert_eq!(
                f.power.owned_ac_session(),
                None,
                "manual watchdog must not compete with game policy"
            );
            f.backend.source = "Battery".into();
            assert_eq!(f.tick().unwrap().mode, Mode::WaitingForAc);
            assert_eq!(f.backend.calls, [TARGET, ORIGINAL]);
            f.backend.source = "AC power".into();
            assert_eq!(f.tick().unwrap().mode, Mode::Suspended);
            assert_eq!(f.backend.calls, [TARGET, ORIGINAL]);
            f.count(&id, 0);
            f.tick().unwrap();
            f.count(&id, 1);
            f.tick().unwrap();
            assert_eq!(f.backend.calls, [TARGET, ORIGINAL, TARGET]);
        }
    }

    #[test]
    fn ac_ineligible_overlap_does_not_keep_a_non_ac_boost_after_the_last_eligible_game_exits() {
        let mut f = Fixture::new();
        let first = f.game("gaming", true, false, true, 1);
        f.tick().unwrap();
        f.game("saving", true, true, true, 1);
        f.tick().unwrap();
        f.backend.source = "Battery".into();
        f.count(&first, 0);
        assert_eq!(f.tick().unwrap().mode, Mode::Suspended);
        assert_eq!(f.backend.calls, [TARGET, ORIGINAL]);
    }

    #[test]
    fn disabling_and_reenabling_a_still_running_game_cannot_bypass_cycle_suppression() {
        for explicit_restore in [false, true] {
            let mut f = Fixture::new();
            let id = f.game("gaming", true, false, true, 1);
            let session = f.tick().unwrap().tuning_session_id.unwrap();
            if explicit_restore {
                f.power
                    .restore_with(&f.database, &session, false, &mut f.backend)
                    .unwrap();
                f.engine.manually_restored(&session);
            }
            f.row(&id).game.auto_boost = false;
            f.database
                .lock()
                .unwrap()
                .execute(
                    "UPDATE registered_games SET auto_boost=0 WHERE id=?1",
                    [&id],
                )
                .unwrap();
            assert_eq!(f.tick().unwrap().mode, Mode::Suspended);
            f.row(&id).game.auto_boost = true;
            f.database
                .lock()
                .unwrap()
                .execute(
                    "UPDATE registered_games SET auto_boost=1 WHERE id=?1",
                    [&id],
                )
                .unwrap();
            for _ in 0..3 {
                assert_eq!(f.tick().unwrap().mode, Mode::Suspended);
            }
            assert_eq!(f.backend.calls, [TARGET, ORIGINAL]);
            f.count(&id, 0);
            assert_eq!(f.tick().unwrap().mode, Mode::Idle);
            f.count(&id, 1);
            assert_eq!(f.tick().unwrap().mode, Mode::Active);
            assert_eq!(f.backend.calls, [TARGET, ORIGINAL, TARGET]);
        }
    }

    #[test]
    fn manual_recovery_before_the_first_watcher_tick_cancels_an_already_running_game_cycle() {
        let mut f = Fixture::new();
        let id = f.game("gaming", true, false, true, 1);
        let session = f.tick().unwrap().tuning_session_id.unwrap();
        f.engine = Controller::default();
        f.power = power::Controller::default();
        f.power
            .restore_with(&f.database, &session, false, &mut f.backend)
            .unwrap();
        f.engine.manually_restored(&session);
        assert_eq!(f.tick().unwrap().mode, Mode::Suspended);
        assert_eq!(f.backend.calls, [TARGET, ORIGINAL]);
        f.count(&id, 0);
        assert_eq!(f.tick().unwrap().mode, Mode::Idle);
        f.count(&id, 1);
        assert_eq!(f.tick().unwrap().mode, Mode::Active);
        assert_eq!(f.backend.calls, [TARGET, ORIGINAL, TARGET]);
    }

    #[test]
    fn manual_restore_with_initially_unverifiable_candidates_waits_for_a_verified_absence() {
        let mut f = Fixture::new();
        let id = f.game("gaming", true, false, true, 0);
        f.row(&id).status = "unverified".into();
        f.engine.manually_restored(&Uuid::new_v4().to_string());
        assert_eq!(f.tick().unwrap().mode, Mode::Suspended);
        f.count(&id, 1);
        assert_eq!(f.tick().unwrap().mode, Mode::Suspended);
        assert!(f.backend.calls.is_empty());
        f.count(&id, 0);
        assert_eq!(f.tick().unwrap().mode, Mode::Idle);
        f.count(&id, 1);
        assert_eq!(f.tick().unwrap().mode, Mode::Active);
        assert_eq!(f.backend.calls, [TARGET]);
    }

    #[test]
    fn explicit_restore_suppresses_reactivation_until_every_relevant_game_exits() {
        let mut f = Fixture::new();
        let id = f.game("gaming", true, false, true, 1);
        let session = f.tick().unwrap().tuning_session_id.unwrap();
        f.power
            .restore_with(&f.database, &session, false, &mut f.backend)
            .unwrap();
        f.engine.manually_restored(&session);
        for _ in 0..3 {
            assert_eq!(f.tick().unwrap().mode, Mode::Suspended);
        }
        assert_eq!(f.backend.calls, [TARGET, ORIGINAL]);
        f.count(&id, 0);
        assert_eq!(f.tick().unwrap().mode, Mode::Idle);
        f.count(&id, 1);
        assert_eq!(f.tick().unwrap().mode, Mode::Active);
        assert_eq!(f.backend.calls, [TARGET, ORIGINAL, TARGET]);
    }

    #[test]
    fn outside_scheme_is_preserved_and_conflict_requires_explicit_recovery() {
        let mut f = Fixture::new();
        f.game("gaming", true, false, true, 1);
        f.tick().unwrap();
        f.backend.current = OUTSIDE.into();
        assert!(f.tick().is_err());
        assert_eq!(f.unfinished()[0].status, "conflict");
        for _ in 0..3 {
            assert_eq!(f.tick().unwrap().mode, Mode::RecoveryRequired);
        }
        assert_eq!(f.backend.calls, [TARGET]);
        assert_eq!(f.backend.current, OUTSIDE);
        assert!(f.engine.status().error.is_some());
        assert!(f
            .engine
            .shutdown(&f.database, &f.power, &mut f.backend)
            .is_err());
        assert_eq!(f.backend.calls, [TARGET]);
    }

    #[test]
    fn session_and_pending_write_failures_cannot_change_windows_or_report_activation() {
        for table in ["gaming_sessions", "tuning_sessions"] {
            let mut f = Fixture::new();
            f.game("gaming", true, false, true, 1);
            f.database.lock().unwrap().execute_batch(&format!("CREATE TRIGGER reject_insert BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'simulated storage failure'); END;")).unwrap();
            assert!(f.tick().is_err());
            assert_eq!(f.engine.status().mode, Mode::Error);
            assert!(f.backend.calls.is_empty());
            assert!(f.unfinished().is_empty());
            assert_eq!(f.backend.current, ORIGINAL);
        }
    }

    #[test]
    fn denied_apply_is_suppressed_and_restore_write_failure_keeps_durable_recovery() {
        let mut f = Fixture::new();
        let id = f.game("gaming", true, false, true, 1);
        f.backend.denied = true;
        assert!(f.tick().is_err());
        for _ in 0..3 {
            f.tick().unwrap();
        }
        assert_eq!(f.backend.calls, [TARGET]);
        assert_eq!(f.backend.current, ORIGINAL);
        assert!(f.engine.status().error.is_some());
        f.count(&id, 0);
        f.tick().unwrap();
        f.count(&id, 1);
        f.tick().unwrap();
        f.database.lock().unwrap().execute_batch("CREATE TRIGGER reject_restore BEFORE UPDATE OF status ON tuning_sessions WHEN NEW.status='restoring' BEGIN SELECT RAISE(ABORT,'simulated storage failure'); END;").unwrap();
        f.count(&id, 0);
        assert!(f.tick().is_err());
        assert_eq!(f.unfinished()[0].status, "active");
        assert_eq!(f.backend.current, TARGET);
        for _ in 0..3 {
            assert_eq!(f.tick().unwrap().mode, Mode::RecoveryRequired);
        }
        assert_eq!(f.backend.calls, [TARGET, TARGET]);
        f.database
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_restore")
            .unwrap();
        let session = f.unfinished()[0].id.clone();
        f.power
            .restore_with(&f.database, &session, false, &mut f.backend)
            .unwrap();
        f.engine.manually_restored(&session);
        f.tick().unwrap();
        assert_eq!(f.backend.calls, [TARGET, TARGET, ORIGINAL]);
    }

    #[test]
    fn crash_after_mutation_leaves_pending_linkage_and_restart_never_clobbers_an_outside_change() {
        let mut f = Fixture::new();
        f.game("gaming", true, false, true, 1);
        f.backend.panic_after_apply = true;
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.tick())).is_err());
        assert_eq!(f.backend.current, TARGET);
        assert_eq!(f.unfinished()[0].status, "pending");
        f.engine = Controller::default();
        f.power = power::Controller::default();
        f.backend.current = OUTSIDE.into();
        assert_eq!(f.tick().unwrap().mode, Mode::RecoveryRequired);
        assert_eq!(f.history_count("interrupted"), 1);
        assert_eq!(f.history_count("active"), 1);
        assert_eq!(f.backend.calls, [TARGET]);
        let session = f.unfinished()[0].id.clone();
        assert!(f
            .power
            .restore_with(&f.database, &session, false, &mut f.backend)
            .is_err());
        assert_eq!(f.backend.current, OUTSIDE);
    }

    #[test]
    fn recovery_restore_after_restart_is_not_immediately_undone_by_a_running_game() {
        for source in ["AC power", "Battery"] {
            let mut f = Fixture::new();
            f.game("gaming", true, true, true, 1);
            f.tick().unwrap();
            f.engine = Controller::default();
            f.power = power::Controller::default();
            f.backend.source = source.into();
            assert_eq!(f.tick().unwrap().mode, Mode::RecoveryRequired);
            let session = f.unfinished()[0].id.clone();
            f.power
                .restore_with(&f.database, &session, false, &mut f.backend)
                .unwrap();
            f.engine.manually_restored(&session);
            f.backend.source = "AC power".into();
            assert_eq!(f.tick().unwrap().mode, Mode::Suspended);
            assert_eq!(f.backend.calls, [TARGET, ORIGINAL]);
        }
    }

    #[test]
    fn disarmed_opt_in_or_failed_linkage_rolls_back_the_entire_pending_transaction() {
        for disarmed in [false, true] {
            let mut f = Fixture::new();
            f.game("gaming", true, false, true, 1);
            let second = f.game("saving", true, false, true, 1);
            if disarmed {
                f.database
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE registered_games SET auto_boost=0 WHERE id=?1",
                        [&second],
                    )
                    .unwrap();
            } else {
                f.database.lock().unwrap().execute_batch("CREATE TRIGGER reject_link BEFORE UPDATE OF tuning_session_id ON gaming_sessions BEGIN SELECT RAISE(ABORT,'simulated linkage failure'); END;").unwrap();
            }
            assert!(f.tick().is_err());
            assert!(f.backend.calls.is_empty());
            assert!(f.unfinished().is_empty());
            let conn = f.database.lock().unwrap();
            let pending: usize = conn
                .query_row("SELECT COUNT(*) FROM tuning_sessions", [], |row| row.get(0))
                .unwrap();
            let linked: usize = conn
                .query_row(
                    "SELECT COUNT(*) FROM gaming_sessions WHERE tuning_session_id IS NOT NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!((pending, linked), (0, 0));
        }
    }

    #[test]
    fn ac_disconnect_between_committed_intent_and_apply_never_changes_windows() {
        let mut f = Fixture::new();
        f.game("gaming", true, true, true, 1);
        f.backend.sources =
            VecDeque::from(["AC power".into(), "AC power".into(), "Battery".into()]);
        assert!(f.tick().is_err());
        assert!(f.backend.calls.is_empty());
        assert!(f.unfinished().is_empty());
        let status: String = f
            .database
            .lock()
            .unwrap()
            .query_row("SELECT status FROM tuning_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(status, "failed");
        for _ in 0..3 {
            f.tick().unwrap();
        }
        assert!(f.backend.calls.is_empty());
    }

    #[test]
    fn repeated_polls_avoid_redundant_linkage_and_continuous_overlap_keeps_tracking_bounded() {
        let mut f = Fixture::new();
        f.game("gaming", true, false, true, 1);
        f.tick().unwrap();
        let second = f.game("saving", true, false, true, 1);
        f.tick().unwrap();
        f.database.lock().unwrap().execute_batch("CREATE TRIGGER reject_redundant_link BEFORE UPDATE OF tuning_session_id ON gaming_sessions WHEN OLD.tuning_session_id=NEW.tuning_session_id BEGIN SELECT RAISE(ABORT,'redundant linkage write'); END;").unwrap();
        for _ in 0..3 {
            f.tick().unwrap();
        }
        for _ in 0..128 {
            f.count(&second, 0);
            f.tick().unwrap();
            f.count(&second, 1);
            f.tick().unwrap();
            assert!(f.engine.active.as_ref().unwrap().linked.len() <= 2);
        }
        assert_eq!(f.backend.calls, [TARGET]);
        assert_eq!(f.history_count("active"), 2);
        assert_eq!(f.history_count("completed"), 128);
    }

    #[test]
    fn restore_on_exit_off_retains_a_visible_session_and_shutdown_respects_the_pinned_policy() {
        for restore in [false, true] {
            let mut f = Fixture::new();
            let id = f.game("gaming", true, false, restore, 1);
            f.tick().unwrap();
            if !restore {
                f.count(&id, 0);
                assert_eq!(f.tick().unwrap().mode, Mode::Retained);
                assert_eq!(f.backend.calls, [TARGET]);
                assert_eq!(f.unfinished()[0].status, "active");
                // Begin another game while the retained global plan is still owned.
                f.count(&id, 1);
                f.tick().unwrap();
            }
            f.engine
                .shutdown(&f.database, &f.power, &mut f.backend)
                .unwrap();
            assert_eq!(f.history_count("app_closed"), 1);
            assert_eq!(f.unfinished().len(), if restore { 0 } else { 1 });
            assert_eq!(f.backend.current, if restore { ORIGINAL } else { TARGET });
        }
    }
}
