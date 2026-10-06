-- Historical schema from c330789; upgrade fixture.

    BEGIN IMMEDIATE;
    CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at TEXT NOT NULL);
    CREATE TABLE devices (id TEXT PRIMARY KEY, device_name TEXT, cpu_model TEXT, ram_total_bytes INTEGER, operating_system TEXT, os_version TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
    CREATE TABLE performance_profiles (id TEXT PRIMARY KEY, name TEXT NOT NULL, profile_type TEXT NOT NULL CHECK(profile_type IN ('system','custom')), created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
    CREATE TABLE profile_settings (id TEXT PRIMARY KEY, profile_id TEXT NOT NULL REFERENCES performance_profiles(id), setting_key TEXT NOT NULL, setting_value TEXT NOT NULL, UNIQUE(profile_id,setting_key));
    CREATE TABLE registered_games (id TEXT PRIMARY KEY, display_name TEXT NOT NULL, canonical_executable_path TEXT NOT NULL, profile_id TEXT NOT NULL REFERENCES performance_profiles(id), auto_boost INTEGER NOT NULL DEFAULT 0 CHECK(auto_boost IN (0,1)), restore_on_exit INTEGER NOT NULL DEFAULT 1 CHECK(restore_on_exit IN (0,1)), ac_only INTEGER NOT NULL DEFAULT 0 CHECK(ac_only IN (0,1)), created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
    CREATE TABLE gaming_sessions (id TEXT PRIMARY KEY, game_id TEXT NOT NULL REFERENCES registered_games(id), started_at TEXT NOT NULL, ended_at TEXT, status TEXT NOT NULL, tuning_session_id TEXT);
    CREATE TABLE device_capabilities (id TEXT PRIMARY KEY, device_id TEXT NOT NULL REFERENCES devices(id), capability_key TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('supported','unsupported','requires_permission','unknown')), reason TEXT NOT NULL, detected_at TEXT NOT NULL, UNIQUE(device_id,capability_key));
    CREATE TABLE hardware_sensors (id TEXT PRIMARY KEY, device_id TEXT NOT NULL REFERENCES devices(id), provider TEXT NOT NULL, sensor_key TEXT NOT NULL, sensor_name TEXT NOT NULL, unit TEXT, is_available INTEGER NOT NULL DEFAULT 1 CHECK(is_available IN (0,1)), UNIQUE(device_id,provider,sensor_key));
    CREATE TABLE hardware_samples (id INTEGER PRIMARY KEY AUTOINCREMENT, sensor_id TEXT NOT NULL REFERENCES hardware_sensors(id), gaming_session_id TEXT REFERENCES gaming_sessions(id), recorded_at TEXT NOT NULL, value REAL NOT NULL);
    CREATE TABLE hardware_sample_aggregates (id INTEGER PRIMARY KEY AUTOINCREMENT, sensor_id TEXT NOT NULL REFERENCES hardware_sensors(id), interval_start TEXT NOT NULL, interval_end TEXT NOT NULL, sample_count INTEGER NOT NULL, min_value REAL NOT NULL, max_value REAL NOT NULL, avg_value REAL NOT NULL, UNIQUE(sensor_id,interval_start,interval_end));
    CREATE TABLE tuning_sessions (id TEXT PRIMARY KEY, profile_id TEXT NOT NULL REFERENCES performance_profiles(id), previous_scheme_guid TEXT NOT NULL, applied_scheme_guid TEXT NOT NULL, trigger TEXT NOT NULL, status TEXT NOT NULL, started_at TEXT NOT NULL, ended_at TEXT, restored_at TEXT, error TEXT);
    CREATE TABLE cleaning_scans (id TEXT PRIMARY KEY, category_snapshot TEXT NOT NULL, estimated_bytes INTEGER NOT NULL, eligible_count INTEGER NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL, expires_at TEXT NOT NULL);
    CREATE TABLE cleaning_results (id TEXT PRIMARY KEY, scan_id TEXT NOT NULL REFERENCES cleaning_scans(id), category TEXT NOT NULL, deleted_count INTEGER NOT NULL, skipped_count INTEGER NOT NULL, error_count INTEGER NOT NULL, estimated_bytes INTEGER NOT NULL, recovered_bytes INTEGER NOT NULL, status TEXT NOT NULL, finished_at TEXT NOT NULL);
    CREATE TABLE gaming_session_metrics (id TEXT PRIMARY KEY, session_id TEXT NOT NULL UNIQUE REFERENCES gaming_sessions(id), average_fps REAL, one_percent_low_fps REAL, frame_time_p99_ms REAL, hardware_summary_json TEXT);
    CREATE TABLE activity_logs (id INTEGER PRIMARY KEY AUTOINCREMENT, event_type TEXT NOT NULL, status TEXT NOT NULL, message TEXT NOT NULL, created_at TEXT NOT NULL);
    CREATE INDEX idx_samples_sensor_time ON hardware_samples(sensor_id,recorded_at);
    CREATE INDEX idx_samples_session ON hardware_samples(gaming_session_id);
    CREATE INDEX idx_aggregates_sensor_time ON hardware_sample_aggregates(sensor_id,interval_start);
    CREATE INDEX idx_tuning_status ON tuning_sessions(status,started_at);
    CREATE INDEX idx_cleaning_finished ON cleaning_results(finished_at);
    CREATE INDEX idx_logs_created ON activity_logs(created_at);
    PRAGMA user_version=1;
    COMMIT;
