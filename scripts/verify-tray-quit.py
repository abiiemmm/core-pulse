"""Verify explicit UI exit and restore only the exact QA preference row."""
import csv
import json
import os
from pathlib import Path
import sqlite3
import subprocess

root = Path("artifacts/tray")
expected = json.loads((root / "ui-quit-expectation.json").read_text(encoding="utf-8"))
baseline = json.loads((root / "ui-quit-baseline.json").read_text(encoding="utf-8"))
system = Path(os.environ["SystemRoot"]) / "System32"
processes = subprocess.check_output(
    [str(system / "tasklist.exe"), "/fi", f"PID eq {expected['appPid']}", "/fo", "csv", "/nh"], text=True,
)
assert all(len(row) < 2 or row[1] != str(expected["appPid"]) for row in csv.reader(processes.splitlines())), "QA app still running"
plan = subprocess.check_output([str(system / "powercfg.exe"), "/getactivescheme"], text=True)
assert expected["originalPlan"]["guid"].lower() in plan.lower(), "Windows scheme changed; do not force restore"
assert expected["preferencesBeforeExit"]["close_to_tray"], "Explicit quit must be checked with tray opt-in"

conn = sqlite3.connect(Path(os.environ["APPDATA"]) / "com.corepulse.desktop/performance.db")
conn.execute("pragma foreign_keys=on")
assert sorted(conn.execute("select * from tuning_sessions").fetchall()) == sorted(map(tuple, baseline["tuning_sessions"])), "Existing recovery changed"
assert not conn.execute("select id from tuning_sessions where status in ('pending','active','restoring','conflict')").fetchall()
current = conn.execute("select value from app_settings where key='preferences'").fetchone()
assert current and json.loads(current[0]) == expected["preferencesBeforeExit"], "QA preferences changed unexpectedly"
samples = conn.execute("select count(*) from hardware_samples").fetchone()[0]
with conn:
    conn.execute("delete from app_settings where key='preferences'")
    for row in baseline["app_settings"]:
        if row[0] == "preferences":
            conn.execute("insert into app_settings values(?,?,?)", row)
    assert sorted(conn.execute("select * from app_settings").fetchall()) == sorted(map(tuple, baseline["app_settings"]))
    assert conn.execute("select count(*) from hardware_samples").fetchone()[0] == samples
    assert not conn.execute("pragma foreign_key_check").fetchall()
assert conn.execute("pragma quick_check").fetchone()[0] == "ok"
conn.close()
result = {"explicitUiQuitWithTrayEnabled": True, "originalPreferencesAndRecoveryPreserved": True,
          "hardwareSamplesPreserved": True, "originalPowerGuidUnchanged": True, "checks": expected["checks"]}
(root / "ui-quit-verification.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
print(json.dumps(result))
