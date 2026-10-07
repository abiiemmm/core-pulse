"""Verify native shutdown and remove only the manifest's QA fixture data."""
import csv
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys

root = Path('artifacts/auto-boost')
expected = json.loads((root / 'shutdown-expectation.json').read_text(encoding='utf-8'))
baseline = json.loads((root / 'database-before-native.json').read_text(encoding='utf-8'))
cleanup_only = sys.argv[1:] == ['--cleanup-only']
if sys.argv[1:] and not cleanup_only:
    raise SystemExit('Unknown arguments')

system = Path(os.environ['WINDIR']) / 'System32'
processes = subprocess.check_output(
    [str(system / 'tasklist.exe'), '/fi', f"PID eq {expected['appPid']}", '/fo', 'csv', '/nh'],
    text=True, errors='replace',
)
assert not any(len(row) > 1 and row[1] == str(expected['appPid']) for row in csv.reader(processes.splitlines())), 'Close the QA app before SQLite verification'
plan = subprocess.check_output([str(system / 'powercfg.exe'), '/getactivescheme'], text=True)
assert expected['originalPlan']['guid'].lower() in plan.lower(), 'Windows scheme changed; never force recovery in QA'

conn = sqlite3.connect(Path(os.environ['APPDATA']) / 'com.corepulse.desktop/performance.db')
conn.execute('pragma foreign_keys=on')
for session_id in [expected['tuningSessionId'], expected['previousTuningSessionId']]:
    tuning = conn.execute('select status,previous_scheme_guid,applied_scheme_guid,restored_at from tuning_sessions where id=?', (session_id,)).fetchone()
    assert tuning and tuning[0] == 'restored' and tuning[1] == tuning[2] == expected['originalPlan']['guid'] and tuning[3], 'Fixture tuning was not restored'
closed = conn.execute('select id,status,ended_at from gaming_sessions where tuning_session_id=?', (expected['tuningSessionId'],)).fetchall()
if not cleanup_only:
    assert len(closed) == 1 and closed[0][1] == 'app_closed' and closed[0][2], 'Active gaming summary was not closed durably'
for row in baseline['tuning_sessions']:
    assert list(conn.execute('select * from tuning_sessions where id=?', (row[0],)).fetchone()) == row, 'Existing recovery record changed'
assert not conn.execute("select id from tuning_sessions where status in ('pending','active','restoring','conflict')").fetchall()
sample_count = conn.execute('select count(*) from hardware_samples').fetchone()[0]
with conn:
    for game_id, name in zip(expected['gameIds'], expected['gameNames']):
        assert conn.execute('select display_name from registered_games where id=?', (game_id,)).fetchone() == (name,), 'Fixture registration identity changed'
        histories = conn.execute('select id,status from gaming_sessions where game_id=?', (game_id,)).fetchall()
        assert all(status != 'active' for _, status in histories)
        for session_id, _ in histories:
            conn.execute('delete from gaming_session_metrics where session_id=?', (session_id,))
            conn.execute('update hardware_samples set gaming_session_id=null where gaming_session_id=?', (session_id,))
            conn.execute('delete from gaming_sessions where id=?', (session_id,))
        conn.execute('delete from registered_games where id=?', (game_id,))
    for profile_id in ['balanced', 'gaming']:
        current = dict(conn.execute("select setting_key,setting_value from profile_settings where profile_id=? and setting_key in ('scheme_guid','ac_only')", (profile_id,)).fetchall())
        assert current == {'scheme_guid': expected['originalPlan']['guid'], 'ac_only': 'false'}, 'Mapping changed during QA'
        conn.execute("delete from profile_settings where profile_id=? and setting_key in ('scheme_guid','ac_only')", (profile_id,))
        for row in baseline['profile_settings']:
            if row[1] == profile_id and row[2] in ['scheme_guid', 'ac_only']:
                conn.execute('insert into profile_settings values(?,?,?,?)', row)
    prefs = conn.execute("select value from app_settings where key='preferences'").fetchone()
    assert prefs and json.loads(prefs[0]) == expected.get('preferencesBeforeExit', expected['originalSettings']), 'Preferences changed during QA'
    conn.execute("delete from app_settings where key='preferences'")
    for row in baseline['app_settings']:
        if row[0] == 'preferences':
            conn.execute('insert into app_settings values(?,?,?)', row)
    assert conn.execute('select count(*) from hardware_samples').fetchone()[0] == sample_count
    assert not conn.execute('pragma foreign_key_check').fetchall()
    assert {row[0] for row in conn.execute('select id from registered_games')} == set(expected['originalGameIds'])
    assert sorted(conn.execute('select * from profile_settings').fetchall()) == sorted(map(tuple, baseline['profile_settings']))
    assert sorted(conn.execute('select * from app_settings').fetchall()) == sorted(map(tuple, baseline['app_settings']))
assert conn.execute('pragma quick_check').fetchone()[0] == 'ok'
conn.close()
result = {'cleanupOnly': cleanup_only, 'exitSource': expected.get('exitSource', 'window_close'), 'gracefulCloseRestored': not cleanup_only, 'closedHistoryDurable': not cleanup_only, 'previousRecoveryRecordsPreserved': True, 'originalPreferencesAndMappingsRestored': True, 'fixtureRegistrationsAndHistoryRemoved': True, 'hardwareSamplesPreserved': True, 'foreignKeysValid': True, 'originalPowerGuidUnchanged': True, 'shutdownSession': expected['tuningSessionId']}
(root / ('fixture-cleanup.json' if cleanup_only else 'shutdown-verification.json')).write_text(json.dumps(result, indent=2), encoding='utf-8')
print(json.dumps(result))
