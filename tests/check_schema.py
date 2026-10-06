"""Fast schema smoke test when Rust/Cargo are not installed."""
from pathlib import Path
import re
import sqlite3

source = Path("src-tauri/src/db.rs").read_text(encoding="utf-8")
match = re.search(r'conn\.execute_batch\(r#"(.*?)"#\)', source, re.S)
assert match, "migration not found"

database = sqlite3.connect(":memory:")
database.execute("PRAGMA foreign_keys=ON")
database.executescript(match.group(1))
upgrade = re.search(r'fn migrate_games.*?conn\.execute_batch\(r#"(.*?)"#\)', source, re.S)
assert upgrade, "game upgrade migration not found"
database.executescript(upgrade.group(1))
assert database.execute("PRAGMA user_version").fetchone()[0] == 2
assert not database.execute("PRAGMA foreign_key_check").fetchall()
tables = {row[0] for row in database.execute("SELECT name FROM sqlite_master WHERE type='table'")}
assert {"devices", "hardware_samples", "tuning_sessions", "cleaning_results", "registered_games", "gaming_sessions"} <= tables
database.execute("INSERT INTO devices(id,created_at,updated_at) VALUES('device','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')")
try:
    database.execute("INSERT INTO hardware_samples(sensor_id,recorded_at,value) VALUES('missing','2026-01-01T00:00:00Z',1)")
except sqlite3.IntegrityError:
    pass
else:
    raise AssertionError("sensor foreign key was not enforced")
try:
    database.execute("INSERT INTO gaming_sessions(id,game_id,started_at,status) VALUES('session','missing','2026-01-01T00:00:00Z','active')")
except sqlite3.IntegrityError:
    pass
else:
    raise AssertionError("game foreign key was not enforced")
print(f"Schema OK: {len(tables)} tables, foreign keys valid")
