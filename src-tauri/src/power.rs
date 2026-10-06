mod windows;

use crate::{db, models::{PerformanceProfile, PowerPlan, TuningSession}};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Mutex;
use uuid::Uuid;
pub(crate) use windows::WindowsPower;

pub(crate) trait PowerBackend {
    fn plans(&mut self) -> Result<Vec<PowerPlan>, String>;
    fn active(&mut self) -> Result<String, String>;
    fn set_active(&mut self, guid: &str) -> Result<(), String>;
    fn power_source(&mut self) -> String;
}

pub fn plans() -> Result<Vec<PowerPlan>, String> { WindowsPower.plans() }

struct OwnedSession { id: String, ac_only: bool, automatic: bool }
#[derive(Default)]
pub struct Controller { operation: Mutex<()>, owned: Mutex<Option<OwnedSession>> }
impl Controller {
    pub fn owned_ac_session(&self) -> Option<String> {
        self.owned.lock().unwrap_or_else(|error| error.into_inner()).as_ref().filter(|session| session.ac_only && !session.automatic).map(|session| session.id.clone())
    }
    fn release_owned(&self, id: &str) {
        let mut owned = self.owned.lock().unwrap_or_else(|error| error.into_inner());
        if owned.as_ref().is_some_and(|session| session.id == id) { *owned = None; }
    }
    pub fn activate(&self, database: &Mutex<Connection>, profile_id: &str) -> Result<TuningSession, String> {
        self.activate_with(database, profile_id, &mut WindowsPower)
    }
    pub fn restore(&self, database: &Mutex<Connection>, session_id: &str, force: bool) -> Result<TuningSession, String> {
        self.restore_with(database, session_id, force, &mut WindowsPower)
    }
    pub fn map_profile(&self, database: &Mutex<Connection>, profile_id: &str, guid: &str, ac_only: bool) -> Result<PerformanceProfile, String> {
        let _operation = self.operation.lock().unwrap_or_else(|error| error.into_inner());
        let target = if guid.is_empty() { String::new() } else { normalized_guid(guid)? };
        if !target.is_empty() && !WindowsPower.plans()?.iter().any(|plan| plan.guid == target) { return Err("Scheme tidak tersedia di Windows".into()); }
        with_db(database, |conn| {
            if db::unfinished_sessions(conn)?.iter().any(|session| session.profile_id == profile_id) { return Err("Pulihkan sesi profil ini sebelum mengubah pemetaan".into()); }
            db::save_profile_mapping(conn, profile_id, &target, ac_only)?;
            profile(conn, profile_id)
        })
    }

    fn activate_with(&self, database: &Mutex<Connection>, profile_id: &str, backend: &mut impl PowerBackend) -> Result<TuningSession, String> {
        self.activate_request(database, profile_id, false, None, backend)
    }

    // Prepared for the opt-in game watcher; no automatic caller is wired yet.
    #[allow(dead_code)]
    pub(crate) fn activate_game_with(&self, database: &Mutex<Connection>, profile_id: &str, ac_only: bool, gaming_sessions: &[String], backend: &mut impl PowerBackend) -> Result<TuningSession, String> {
        if gaming_sessions.is_empty() || gaming_sessions.len() > 100 { return Err("Sesi game terverifikasi diperlukan".into()); }
        self.activate_request(database, profile_id, ac_only, Some(gaming_sessions), backend)
    }

    fn activate_request(&self, database: &Mutex<Connection>, profile_id: &str, game_ac_only: bool, gaming_sessions: Option<&[String]>, backend: &mut impl PowerBackend) -> Result<TuningSession, String> {
        // Only system-changing power operations serialize here. SQLite is held for
        // short reads/writes and is NEVER held while asking Windows for anything.
        // A poisoned unit mutex contains no state; durable sessions remain the guard.
        let _operation = self.operation.lock().unwrap_or_else(|error| error.into_inner());
        let selected = with_db(database, |conn| { ensure_no_session(conn)?; profile(conn, profile_id) })?;
        let ac_only = selected.ac_only || game_ac_only;
        if ac_only && backend.power_source() != "AC power" { return Err("Profil ini hanya berlaku saat daya AC".into()); }
        let target = normalized_guid(selected.scheme_guid.as_deref().ok_or("Profil belum dipetakan ke Windows power scheme")?)?;
        if !backend.plans()?.iter().any(|plan| plan.guid == target) { return Err("Scheme yang dipetakan tidak tersedia lagi".into()); }
        let previous = normalized_guid(&backend.active()?)?;
        let session = TuningSession { id: Uuid::new_v4().to_string(), profile_id: profile_id.into(), previous_guid: previous, applied_guid: target, status: "pending".into(), started_at: Utc::now().to_rfc3339(), error: None };
        with_db(database, |conn| {
            // A SQLite write transaction protects the pending check across separate
            // app instances/connections, not just this controller's in-memory gate.
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|error| error.to_string())?;
            ensure_no_session(&tx)?;
            let current = profile(&tx, profile_id)?;
            if current.scheme_guid.as_deref().map(normalized_guid).transpose()?.as_deref() != Some(&session.applied_guid) || current.ac_only != selected.ac_only { return Err("Pemetaan profil berubah; muat ulang sebelum menerapkan".into()); }
            tx.execute("INSERT INTO tuning_sessions(id,profile_id,previous_scheme_guid,applied_scheme_guid,trigger,status,started_at) VALUES(?1,?2,?3,?4,?6,'pending',?5)", params![session.id,session.profile_id,session.previous_guid,session.applied_guid,session.started_at,if gaming_sessions.is_some() { "auto_game" } else { "manual" } ]).map_err(|error| error.to_string())?;
            if let Some(ids) = gaming_sessions {
                for id in ids {
                    Uuid::parse_str(id).map_err(|_| "Identitas sesi game tidak valid")?;
                    let changed = tx.execute("UPDATE gaming_sessions SET tuning_session_id=?2 WHERE id=?1 AND status='active' AND tuning_session_id IS NULL AND EXISTS(SELECT 1 FROM registered_games g WHERE g.id=gaming_sessions.game_id AND g.archived=0 AND g.auto_boost=1 AND g.executable_identity!='')", params![id,session.id]).map_err(|error|error.to_string())?;
                    if changed != 1 { return Err("Sesi game tidak lagi memenuhi syarat Auto Boost".into()); }
                }
            }
            db::log(&tx, "tuning", "pending", "Power scheme change requested");
            tx.commit().map_err(|error| error.to_string())
        })?;
        let result: Result<(), String> = (|| {
            // Recheck both AC and external scheme changes after committing pending.
            if ac_only && backend.power_source() != "AC power" { return Err("Profil ini hanya berlaku saat daya AC".into()); }
            if normalized_guid(&backend.active()?)? != session.previous_guid { return Err("Power plan telah diubah di luar Core Pulse. Tinjau sebelum memulihkan.".into()); }
            if session.applied_guid != session.previous_guid { apply_verified(backend, &session.applied_guid)?; }
            update_session(database, &session.id, "active", None, false)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                *self.owned.lock().unwrap_or_else(|error| error.into_inner()) = Some(OwnedSession { id: session.id.clone(), ac_only, automatic: gaming_sessions.is_some() });
                Ok(TuningSession { status: "active".into(), ..session })
            },
            Err(error) => {
                self.recover_failed_activation(database, backend, &session, &error)?;
                Err(error)
            }
        }
    }

    fn recover_failed_activation(&self, database: &Mutex<Connection>, backend: &mut impl PowerBackend, session: &TuningSession, error: &str) -> Result<(), String> {
        let mut safe = backend.active().ok().as_deref() == Some(&session.previous_guid);
        if !safe && backend.active().ok().as_deref() == Some(&session.applied_guid) {
            let previous_available = backend.plans().map(|plans| plans.iter().any(|plan| plan.guid == session.previous_guid)).unwrap_or(false);
            // Never roll back over an observed outside change or an unknown state.
            if previous_available && backend.active().ok().as_deref() == Some(&session.applied_guid) {
                let _ = apply_verified(backend, &session.previous_guid);
                safe = backend.active().ok().as_deref() == Some(&session.previous_guid);
            }
        }
        update_session(database, &session.id, if safe { "failed" } else { "conflict" }, Some(error), safe)
    }

    pub(crate) fn restore_with(&self, database: &Mutex<Connection>, session_id: &str, force: bool, backend: &mut impl PowerBackend) -> Result<TuningSession, String> {
        normalized_guid(session_id).map_err(|_| "Sesi tuning tidak valid")?;
        let _operation = self.operation.lock().unwrap_or_else(|error| error.into_inner());
        let session = with_db(database, |conn| load_session(conn, session_id))?;
        if session.status == "restored" { self.release_owned(session_id); return Ok(session); }
        if !["pending", "active", "restoring", "conflict"].contains(&session.status.as_str()) { return Err("Sesi tidak dapat dipulihkan".into()); }
        normalized_guid(&session.previous_guid)?;
        normalized_guid(&session.applied_guid)?;
        if !backend.plans()?.iter().any(|plan| plan.guid == session.previous_guid) { return Err("Power scheme sebelumnya tidak tersedia".into()); }
        let current = normalized_guid(&backend.active()?)?;
        if current != session.previous_guid {
            if current != session.applied_guid && !force {
                let error = "Power plan telah diubah di luar Core Pulse. Tinjau sebelum memulihkan.";
                update_session(database, session_id, "conflict", Some(error), false)?;
                return Err(error.into());
            }
            // Persist restoration intent before mutation. A crash leaves a visible,
            // retryable session; a completed restore is idempotent after restart.
            update_session(database, session_id, "restoring", None, false)?;
            let observed = normalized_guid(&backend.active()?)?;
            if observed != current && observed != session.previous_guid {
                let error = "Power plan berubah saat pemulihan; tinjau sesi kembali";
                update_session(database, session_id, "conflict", Some(error), false)?;
                return Err(error.into());
            }
            if observed != session.previous_guid {
                if let Err(error) = apply_verified(backend, &session.previous_guid) {
                    if backend.active().ok().as_deref() != Some(&session.previous_guid) {
                        update_session(database, session_id, "conflict", Some(&error), false)?;
                        return Err(error);
                    }
                }
            }
        }
        update_session(database, session_id, "restored", None, true)?;
        self.release_owned(session_id);
        Ok(TuningSession { status: "restored".into(), error: None, ..session })
    }
}

fn normalized_guid(value: &str) -> Result<String, String> {
    Uuid::parse_str(value).map(|id| id.to_string()).map_err(|_| "GUID power plan tidak valid".into())
}
fn with_db<T>(database: &Mutex<Connection>, operation: impl FnOnce(&mut Connection) -> Result<T, String>) -> Result<T, String> {
    let mut conn = database.lock().map_err(|_| "Database tidak tersedia")?;
    operation(&mut conn)
}
fn profile(conn: &Connection, profile_id: &str) -> Result<PerformanceProfile, String> {
    db::profiles(conn)?.into_iter().find(|profile| profile.id == profile_id).ok_or("Profil tidak ditemukan".into())
}
fn ensure_no_session(conn: &Connection) -> Result<(), String> {
    if !db::unfinished_sessions(conn)?.is_empty() { Err("Selesaikan sesi tuning sebelumnya sebelum mengaktifkan profil lain".into()) } else { Ok(()) }
}
pub(crate) fn load_session(conn: &Connection, id: &str) -> Result<TuningSession, String> {
    conn.query_row("SELECT id,profile_id,previous_scheme_guid,applied_scheme_guid,status,started_at,error FROM tuning_sessions WHERE id=?1", [id], |row| Ok(TuningSession { id:row.get(0)?,profile_id:row.get(1)?,previous_guid:row.get(2)?,applied_guid:row.get(3)?,status:row.get(4)?,started_at:row.get(5)?,error:row.get(6)? })).optional().map_err(|error| error.to_string())?.ok_or("Sesi tidak ditemukan".into())
}
fn apply_verified(backend: &mut impl PowerBackend, target: &str) -> Result<(), String> {
    normalized_guid(target)?;
    backend.set_active(target)?;
    if normalized_guid(&backend.active()?)? != target { return Err("Windows tidak mengaktifkan scheme yang diminta".into()); }
    Ok(())
}
fn update_session(database: &Mutex<Connection>, id: &str, status: &str, error: Option<&str>, finished: bool) -> Result<(), String> {
    with_db(database, |conn| {
        let at = Utc::now().to_rfc3339();
        let changed = conn.execute("UPDATE tuning_sessions SET status=?2,error=?3,ended_at=CASE WHEN ?4 THEN ?5 ELSE NULL END,restored_at=CASE WHEN ?2='restored' THEN ?5 ELSE NULL END WHERE id=?1 AND status IN ('pending','active','restoring','conflict')", params![id,status,error,finished,at]).map_err(|error| error.to_string())?;
        if changed != 1 { return Err("Sesi tidak ditemukan".into()); }
        db::log(conn, "tuning", status, "Power operation state updated");
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, path::Path, sync::Arc};
    const ORIGINAL: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
    const TARGET: &str = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";
    const OUTSIDE: &str = "a1841308-3541-4fab-bc81-f71556f20b4a";

    enum Change { Success, Denied, ChangedThenError, Ignored, OutsideChange, LostReadback, Panic }
    struct FakePower {
        current: String,
        available: Vec<String>,
        changes: VecDeque<Change>,
        sources: VecDeque<String>,
        calls: Vec<String>,
        read_errors: usize,
        before_set: Option<Box<dyn FnMut(&str) + Send>>,
    }
    impl Default for FakePower {
        fn default() -> Self { Self { current: ORIGINAL.into(), available: vec![ORIGINAL.into(), TARGET.into(), OUTSIDE.into()], changes: VecDeque::new(), sources: VecDeque::new(), calls: vec![], read_errors: 0, before_set: None } }
    }
    impl PowerBackend for FakePower {
        fn plans(&mut self) -> Result<Vec<PowerPlan>, String> { Ok(self.available.iter().map(|guid| PowerPlan { guid:guid.clone(), name:guid.clone(), active:*guid == self.current }).collect()) }
        fn active(&mut self) -> Result<String, String> {
            if self.read_errors > 0 { self.read_errors -= 1; return Err("simulated readback failure".into()); }
            Ok(self.current.clone())
        }
        fn set_active(&mut self, guid: &str) -> Result<(), String> {
            if let Some(callback) = &mut self.before_set { callback(guid); }
            self.calls.push(guid.into());
            match self.changes.pop_front().unwrap_or(Change::Success) {
                Change::Success => { self.current = guid.into(); Ok(()) }
                Change::Denied => Err("simulated permission denied".into()),
                Change::ChangedThenError => { self.current = guid.into(); Err("simulated partial apply".into()) }
                Change::Ignored => Ok(()),
                Change::OutsideChange => { self.current = OUTSIDE.into(); Ok(()) }
                Change::LostReadback => { self.current = guid.into(); self.read_errors = 1; Ok(()) }
                Change::Panic => { self.current = guid.into(); panic!("simulated interruption after mutation"); }
            }
        }
        fn power_source(&mut self) -> String { self.sources.pop_front().unwrap_or_else(|| "AC power".into()) }
    }
    fn database(ac_only: bool) -> Arc<Mutex<Connection>> {
        let mut conn = db::open(Path::new(":memory:")).unwrap();
        db::save_profile_mapping(&mut conn, "gaming", TARGET, ac_only).unwrap();
        Arc::new(Mutex::new(conn))
    }
    fn unfinished(database: &Mutex<Connection>) -> TuningSession { db::unfinished_sessions(&database.lock().unwrap()).unwrap().remove(0) }

    #[test]
    fn watchdog_owns_only_current_runtime_ac_sessions() {
        for ac_only in [false, true] {
            let database = database(ac_only);
            let mut backend = FakePower::default();
            let controller = Controller::default();
            let session = controller.activate_with(&database, "gaming", &mut backend).unwrap();
            assert_eq!(controller.owned_ac_session(), ac_only.then(|| session.id.clone()));
            // Durable recovery after restart never becomes an automatic mutation.
            assert_eq!(Controller::default().owned_ac_session(), None);
            controller.restore_with(&database, &session.id, false, &mut backend).unwrap();
            assert_eq!(controller.owned_ac_session(), None);
        }
    }

    #[test]
    fn restored_storage_failure_retries_without_a_second_os_mutation() {
        let database = database(true);
        let mut backend = FakePower::default();
        let controller = Controller::default();
        let session = controller.activate_with(&database, "gaming", &mut backend).unwrap();
        database.lock().unwrap().execute_batch("CREATE TRIGGER reject_restored BEFORE UPDATE OF status ON tuning_sessions WHEN NEW.status='restored' BEGIN SELECT RAISE(ABORT,'simulated storage failure'); END;").unwrap();
        assert!(controller.restore_with(&database, &session.id, false, &mut backend).is_err());
        assert_eq!(backend.current, ORIGINAL);
        assert_eq!(unfinished(&database).status, "restoring");
        assert_eq!(controller.owned_ac_session(), Some(session.id.clone()));
        database.lock().unwrap().execute_batch("DROP TRIGGER reject_restored").unwrap();
        controller.restore_with(&database, &session.id, false, &mut backend).unwrap();
        assert_eq!(backend.calls, [TARGET, ORIGINAL]);
        assert_eq!(controller.owned_ac_session(), None);
    }

    #[test]
    fn pending_is_durable_and_database_unlocked_before_os_mutation() {
        let database = database(false);
        let inspect = database.clone();
        let mut backend = FakePower { before_set: Some(Box::new(move |guid| {
            let conn = inspect.try_lock().expect("OS operations must not hold SQLite");
            let pending = db::unfinished_sessions(&conn).unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].status, "pending");
            assert_eq!(pending[0].previous_guid, ORIGINAL);
            assert_eq!(pending[0].applied_guid, guid);
        })), ..Default::default() };
        let session = Controller::default().activate_with(&database, "gaming", &mut backend).unwrap();
        assert_eq!(session.status, "active");
        assert_eq!(backend.current, TARGET);
        assert_eq!(unfinished(&database).status, "active");
    }

    #[test]
    fn failed_pending_write_never_changes_windows() {
        let database = database(false);
        database.lock().unwrap().execute_batch("CREATE TRIGGER reject_pending BEFORE INSERT ON tuning_sessions BEGIN SELECT RAISE(ABORT,'simulated storage failure'); END;").unwrap();
        let mut backend = FakePower::default();
        assert!(Controller::default().activate_with(&database, "gaming", &mut backend).is_err());
        assert!(backend.calls.is_empty());
        assert_eq!(backend.current, ORIGINAL);
        assert!(db::unfinished_sessions(&database.lock().unwrap()).unwrap().is_empty());
    }

    #[test]
    fn failed_active_write_rolls_back_verified_mutation() {
        let database = database(false);
        database.lock().unwrap().execute_batch("CREATE TRIGGER reject_active BEFORE UPDATE OF status ON tuning_sessions WHEN NEW.status='active' BEGIN SELECT RAISE(ABORT,'simulated storage failure'); END;").unwrap();
        let mut backend = FakePower::default();
        assert!(Controller::default().activate_with(&database, "gaming", &mut backend).is_err());
        assert_eq!(backend.calls, [TARGET, ORIGINAL]);
        assert_eq!(backend.current, ORIGINAL);
        let status: String = database.lock().unwrap().query_row("SELECT status FROM tuning_sessions", [], |row| row.get(0)).unwrap();
        assert_eq!(status, "failed");
    }

    #[test]
    fn missing_mapping_missing_plan_and_ac_rules_prevent_changes() {
        let database = database(true);
        let mut backend = FakePower { sources: VecDeque::from(["Battery".into()]), ..Default::default() };
        let controller = Controller::default();
        assert!(controller.activate_with(&database, "gaming", &mut backend).is_err());
        backend.sources = VecDeque::from(["Unknown".into()]);
        assert!(controller.activate_with(&database, "gaming", &mut backend).is_err());
        backend.available.retain(|guid| guid != TARGET);
        assert!(controller.activate_with(&database, "gaming", &mut backend).is_err());
        assert!(controller.activate_with(&database, "saving", &mut backend).is_err());
        assert!(controller.activate_with(&database, "unknown-profile", &mut backend).is_err());
        assert!(backend.calls.is_empty());
    }

    #[test]
    fn ac_disconnect_after_pending_is_committed_prevents_application() {
        let database = database(true);
        let mut backend = FakePower { sources: VecDeque::from(["AC power".into(), "Battery".into()]), ..Default::default() };
        assert!(Controller::default().activate_with(&database, "gaming", &mut backend).is_err());
        assert!(backend.calls.is_empty());
        assert_eq!(backend.current, ORIGINAL);
        assert!(db::unfinished_sessions(&database.lock().unwrap()).unwrap().is_empty());
    }

    #[test]
    fn denied_and_ignored_applications_do_not_claim_success() {
        for change in [Change::Denied, Change::Ignored] {
            let database = database(false);
            let mut backend = FakePower { changes: VecDeque::from([change]), ..Default::default() };
            assert!(Controller::default().activate_with(&database, "gaming", &mut backend).is_err());
            assert_eq!(backend.current, ORIGINAL);
            assert_eq!(backend.calls, [TARGET]);
            assert!(db::unfinished_sessions(&database.lock().unwrap()).unwrap().is_empty());
        }
    }

    #[test]
    fn partial_apply_and_lost_verification_restore_the_previous_plan() {
        for change in [Change::ChangedThenError, Change::LostReadback] {
            let database = database(false);
            let mut backend = FakePower { changes: VecDeque::from([change]), ..Default::default() };
            assert!(Controller::default().activate_with(&database, "gaming", &mut backend).is_err());
            assert_eq!(backend.current, ORIGINAL);
            assert_eq!(backend.calls, [TARGET, ORIGINAL]);
        }
    }

    #[test]
    fn rollback_failure_keeps_a_durable_recoverable_session() {
        let database = database(false);
        let mut backend = FakePower { changes: VecDeque::from([Change::ChangedThenError, Change::Denied]), ..Default::default() };
        let controller = Controller::default();
        assert!(controller.activate_with(&database, "gaming", &mut backend).is_err());
        assert_eq!(backend.current, TARGET);
        let session = unfinished(&database);
        assert_eq!(session.status, "conflict");
        assert!(session.error.is_some());
        assert!(controller.activate_with(&database, "gaming", &mut backend).is_err());
        let restored = controller.restore_with(&database, &session.id, false, &mut backend).unwrap();
        assert_eq!(restored.status, "restored");
        assert_eq!(backend.current, ORIGINAL);
    }

    #[test]
    fn external_override_during_application_is_not_rolled_back() {
        let database = database(false);
        let mut backend = FakePower { changes: VecDeque::from([Change::OutsideChange]), ..Default::default() };
        assert!(Controller::default().activate_with(&database, "gaming", &mut backend).is_err());
        assert_eq!(backend.current, OUTSIDE);
        assert_eq!(backend.calls, [TARGET]);
        assert_eq!(unfinished(&database).status, "conflict");
    }

    #[test]
    fn restore_respects_outside_changes_until_explicit_force() {
        let database = database(false);
        let mut backend = FakePower::default();
        let controller = Controller::default();
        let session = controller.activate_with(&database, "gaming", &mut backend).unwrap();
        backend.current = OUTSIDE.into();
        assert!(controller.restore_with(&database, &session.id, false, &mut backend).is_err());
        assert_eq!(backend.current, OUTSIDE);
        assert_eq!(backend.calls, [TARGET]);
        assert_eq!(unfinished(&database).status, "conflict");
        controller.restore_with(&database, &session.id, true, &mut backend).unwrap();
        assert_eq!(backend.current, ORIGINAL);
        assert_eq!(backend.calls, [TARGET, ORIGINAL]);
    }

    #[test]
    fn restoration_is_recorded_before_windows_and_is_idempotent() {
        let database = database(false);
        let mut backend = FakePower::default();
        let controller = Controller::default();
        let session = controller.activate_with(&database, "gaming", &mut backend).unwrap();
        let inspect = database.clone();
        backend.before_set = Some(Box::new(move |guid| {
            assert_eq!(guid, ORIGINAL);
            let conn = inspect.try_lock().expect("restoration must not hold SQLite");
            assert_eq!(db::unfinished_sessions(&conn).unwrap()[0].status, "restoring");
        }));
        controller.restore_with(&database, &session.id, false, &mut backend).unwrap();
        let calls = backend.calls.len();
        controller.restore_with(&database, &session.id, true, &mut backend).unwrap();
        assert_eq!(backend.calls.len(), calls);
    }

    #[test]
    fn failed_restoration_write_does_not_mutate_windows() {
        let database = database(false);
        let mut backend = FakePower::default();
        let controller = Controller::default();
        let session = controller.activate_with(&database, "gaming", &mut backend).unwrap();
        database.lock().unwrap().execute_batch("CREATE TRIGGER reject_restore BEFORE UPDATE OF status ON tuning_sessions WHEN NEW.status='restoring' BEGIN SELECT RAISE(ABORT,'simulated storage failure'); END;").unwrap();
        assert!(controller.restore_with(&database, &session.id, false, &mut backend).is_err());
        assert_eq!(backend.calls, [TARGET]);
        assert_eq!(backend.current, TARGET);
        assert_eq!(unfinished(&database).status, "active");
    }

    #[test]
    fn failed_restore_preserves_retry_and_missing_original_prevents_mutation() {
        let database = database(false);
        let mut backend = FakePower::default();
        let controller = Controller::default();
        let session = controller.activate_with(&database, "gaming", &mut backend).unwrap();
        backend.available.retain(|guid| guid != ORIGINAL);
        assert!(controller.restore_with(&database, &session.id, true, &mut backend).is_err());
        assert_eq!(backend.calls, [TARGET]);
        backend.available.push(ORIGINAL.into());
        backend.changes.push_back(Change::Denied);
        assert!(controller.restore_with(&database, &session.id, false, &mut backend).is_err());
        assert_eq!(unfinished(&database).status, "conflict");
        controller.restore_with(&database, &session.id, false, &mut backend).unwrap();
        assert_eq!(backend.current, ORIGINAL);
    }

    #[test]
    fn interruptions_after_apply_and_during_restore_are_recoverable() {
        for during_restore in [false, true] {
            let database = database(false);
            let controller = Controller::default();
            let mut backend = FakePower::default();
            if during_restore {
                let session = controller.activate_with(&database, "gaming", &mut backend).unwrap();
                backend.changes.push_back(Change::Panic);
                assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| controller.restore_with(&database, &session.id, false, &mut backend))).is_err());
                assert_eq!(unfinished(&database).status, "restoring");
                assert_eq!(backend.current, ORIGINAL);
            } else {
                backend.changes.push_back(Change::Panic);
                assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| controller.activate_with(&database, "gaming", &mut backend))).is_err());
                assert_eq!(unfinished(&database).status, "pending");
                assert_eq!(backend.current, TARGET);
            }
            let session = unfinished(&database);
            // Even the poisoned operation gate can recover using persisted state.
            controller.restore_with(&database, &session.id, false, &mut backend).unwrap();
            assert_eq!(backend.current, ORIGINAL);
            assert!(db::unfinished_sessions(&database.lock().unwrap()).unwrap().is_empty());
        }
    }

    #[test]
    fn active_profiles_cannot_be_remapped_or_activated_twice() {
        let database = database(false);
        let mut backend = FakePower::default();
        let controller = Controller::default();
        controller.activate_with(&database, "gaming", &mut backend).unwrap();
        assert!(controller.activate_with(&database, "gaming", &mut backend).is_err());
        assert!(controller.map_profile(&database, "gaming", "", true).is_err());
        assert_eq!(backend.calls, [TARGET]);
        assert_eq!(profile(&database.lock().unwrap(), "gaming").unwrap().scheme_guid.as_deref(), Some(TARGET));
    }

    #[test]
    fn overlapping_activation_serializes_without_blocking_database_readers() {
        let database = database(false);
        let controller = Arc::new(Controller::default());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let first_db = database.clone();
        let first_controller = controller.clone();
        let first = std::thread::spawn(move || {
            let mut backend = FakePower { before_set: Some(Box::new(move |_| { entered_tx.send(()).unwrap(); resume_rx.recv().unwrap(); })), ..Default::default() };
            first_controller.activate_with(&first_db, "gaming", &mut backend)
        });
        entered_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        // A long-running OS operation leaves history/settings reads available.
        assert!(database.try_lock().is_ok());
        let second_db = database.clone();
        let second = std::thread::spawn(move || {
            let mut backend = FakePower::default();
            let result = controller.activate_with(&second_db, "gaming", &mut backend);
            (result.is_err(), backend.calls.len())
        });
        resume_tx.send(()).unwrap();
        assert!(first.join().unwrap().is_ok());
        assert_eq!(second.join().unwrap(), (true, 0));
        assert_eq!(db::unfinished_sessions(&database.lock().unwrap()).unwrap().len(), 1);
    }
}
