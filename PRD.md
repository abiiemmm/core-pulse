# Product Requirements Document (PRD)

# PC Performance Tuner & Hardware Monitor

| Field | Value |
|---|---|
| Version | 1.0 |
| Status | Draft for implementation |
| Target platform | Windows 10/11 x64 (Windows-first) |
| Desktop | Tauri 2 |
| UI | React, TypeScript, Tailwind CSS, shadcn/ui |
| Native backend | Rust |
| Hardware sensor provider | C#/.NET sidecar with LibreHardwareMonitorLib, subject to sensor support and license review |
| Local database | **SQLite only** |
| Product model | Offline-first, single-user, local desktop application |

> **Product promise:** Display trustworthy hardware statistics, offer explicitly approved, reversible Windows performance settings, reclaim disk space safely, and correlate gaming stutters with recorded hardware metrics. No guaranteed FPS increase.

## 1. Background and problem

Users often switch between hardware monitoring, Windows power settings, temporary-file cleanup and game performance utilities. Hardware sensors vary across PCs and laptops; an optimization offered by one vendor may not be available on another. Unrestricted cleaners and so-called boosters risk deleting useful data, disrupting background tasks or changing important system settings without the user's knowledge.

The product combines clear hardware visibility and a controlled set of system operations in one lightweight Windows desktop app. It discovers supported capabilities rather than assuming every device has fan control, a particular OEM boost mode or particular sensors.

## 2. Goals, success metrics and non-goals

### Goals

- Provide real-time CPU, GPU, RAM, disk and network statistics, distinguishing valid zero values from unavailable sensors.
- Detect hardware and supported system capabilities; show why unavailable features cannot be used.
- Allow manual activation of an existing Windows power plan and restoration of the prior plan.
- Scan approved temporary-file categories, preview recoverable space and clean only after explicit confirmation.
- Allow opt-in Auto Boost for registered games, with robust configuration restoration.
- Store settings, device metadata, history, logs and session summaries locally in SQLite.
- Correlate recorded hardware readings with optional game FPS/frame-time capture in a later release.

### Measurable launch criteria (targets, not guaranteed performance claims)

| Metric | Target / verification |
|---|---|
| Monitoring refresh | Configurable 1, 2 or 5 seconds; default 1 second |
| UI responsiveness | Routine navigation and toggle interactions do not block during sensor reads, scans or DB writes |
| Read-only operation | Monitoring and viewing historical data work without running the whole app as administrator |
| Sensor failure | One failed sensor/provider does not crash the application or fabricate a zero reading |
| Power plan safety | Before change, record prior GUID; verify result; expose restore; recover unfinished sessions after crash |
| Cleaning safety | No deletion of Documents, Desktop, Downloads, arbitrary paths, browser sessions or active files |
| Retention | Raw sample retention and aggregates configurable; bounded DB growth verified by tests |
| Resource usage | Profile on a representative desktop and laptop; set release budgets from measured baselines rather than promising universal RAM/CPU ceilings |

### Non-goals

No overclocking, undervolting, BIOS/UEFI editing, firmware fan control, driver patching, kernel driver development, registry-cleaning gimmicks, disabling Defender/security features, force-closing arbitrary background processes, game injection, bypassing anti-cheat, or advertising cache deletion as a guaranteed performance improvement. OEM-specific modes such as OMEN Gaming Hub Performance Mode are **not** controlled unless a documented, supported integration is independently implemented and tested.

## 3. Users and principal journeys

1. **Everyday PC user:** Opens dashboard, sees sensor availability and disk consumption, scans temporary files, confirms selected cleanup, reviews actual recovered bytes.
2. **Gamer:** Registers a game executable, opts in to Auto Boost, chooses an available Windows power plan; on verified game launch the app applies the plan, then restores it when the final applicable game exits.
3. **Troubleshooter:** Records hardware samples during a gameplay session, inspects coincident temperature, clock, memory and GPU-utilization changes; in V2 compares these with actual captured frame times.
4. **Laptop user:** Sees AC/battery state; may restrict a boost profile to AC power; receives unsupported or permission-required explanations for inaccessible sensors and controls.

## 4. Releases and priorities

| Feature | MVP / P0 | V1 / P1 | V2 / P2 |
|---|:---:|:---:|:---:|
| Device discovery and capability status | ✓ | | |
| CPU/GPU/RAM/disk/network monitoring, availability indicators | ✓ | | |
| Live charts, time-window controls, local history | ✓ | | |
| Discover and manually switch existing Windows power plans; restore | ✓ | | |
| Safe temporary-file scan, preview, confirmed cleanup and history | ✓ | | |
| Device/system information and app settings | ✓ | | |
| Per-process resource list and user-confirmed allowed actions | | ✓ | |
| Registered games and opt-in Auto Boost | | ✓ | |
| Historical aggregation and analytics | | ✓ | |
| Optional game session capture and FPS/frame-time integration | | | ✓ |
| Session comparison, diagnosis with uncertainty, export | | | ✓ |

**Scope rule:** Complete reliable read-only monitoring and reversible manual tuning before implementing automatic tuning or game measurement.

## 5. Technology and architectural decisions

### 5.1 Stack

- Tauri 2 packages the desktop UI and exposes narrowly scoped commands and events.
- React + TypeScript handles presentation; Tailwind/shadcn/ui handles components; Zustand may hold transient view state; Recharts handles charts.
- Rust owns capability checks, command validation, process discovery, cleanup planning/execution, power-plan lifecycle, buffering and SQLite writes.
- Windows APIs and supported Windows command-line interfaces provide generic device/process/power information. Any process invocation uses a fixed executable and structured allowlisted arguments, **never** frontend-supplied shell text.
- A bundled .NET sensor sidecar wraps LibreHardwareMonitorLib for supported sensors. The Rust service normalizes sensor readings and validates the sidecar protocol. Some sensors are unavailable or require elevation; do not promise coverage for every CPU/GPU/laptop.
- SQLite is the **only database**. No Express, PostgreSQL or cloud backend is required.
- Later: opt-in PresentMon integration may supply FPS/frame time on compatible systems and games. It must not inject into or manipulate games or bypass anti-cheat protections.

### 5.2 Component flow

```text
React UI / charts / settings
      | Tauri invoke (requests) + events (live samples/status)
      v
Rust application core
  |-- Device & capability discovery -> Windows APIs
  |-- Monitoring orchestrator ------> C# sensor sidecar / Windows counters
  |-- Performance controller ------> Windows power scheme interface
  |-- Cleaner planner/executor -----> approved locations / Windows cleanup interfaces
  |-- Game watcher (V1) -----------> verified process identity
  |-- Analytics / history ---------> SQLite
      ^
      | batched, validated sensor readings; no generic SQL exposure to UI
```

### 5.3 Threads and background work

Sensor polling, filesystem scanning, power operations and SQLite persistence must not block the Tauri/UI event loop. Retain recent points in a bounded in-memory ring buffer, stream snapshots to UI at the selected refresh cadence, write validated records to SQLite in batches, and report task progress/cancellation. Stop unused polling when disabled; tray behavior is user-configurable. Restart a failed sensor agent with bounded backoff; surface a degraded state rather than silently returning stale readings.

### 5.4 Packaging and privileges

Distribute a signed installer where feasible; package the expected sidecar/runtime dependencies and verify compatibility with target Windows builds. The main app launches unelevated. Any privileged sensor or system action uses a narrowly scoped, authenticated helper only when necessary and after a Windows UAC prompt. Do not run a persistent privileged helper with a generic execute/delete endpoint. A declined elevation leaves read-only supported functions operational.

## 6. Functional requirements

### FR-01 — Device discovery and capability detection (P0)

On launch and user-initiated refresh, identify OS/build/architecture, CPU model/cores, available GPU adapters, RAM, storage volumes/devices, power source and display info where available. Store a minimal device snapshot; do not collect serial numbers by default. Discover current/available Windows power scheme GUIDs and sensor provider capabilities.

Each feature receives a status: `supported`, `unsupported`, `requires_permission`, or `unknown`, plus a user-readable reason and last check timestamp. Revalidate capability at execution time; a cached database flag never grants OS permission. Multi-GPU machines must expose sensors by stable provider/device key and allow adapter selection. Unknown and unsupported controls are disabled with an explanation.

**Acceptance:** Missing sensor or denied permission never crashes app; multiple GPUs are distinguished; manual detection refresh updates device records and capabilities; no OEM mode is labeled supported merely from a device brand.

### FR-02 — Real-time hardware monitoring (P0)

Capture available metrics:

| Area | Metrics, when supported |
|---|---|
| CPU | Aggregate usage, optional per-core usage, temperature, effective/nominal clocks where available, package power |
| GPU | Adapter identity, utilization, temperature, core/memory clock, VRAM used/total, power |
| RAM | Total, available, used, percent used |
| Storage | Volume capacity/free space, device read/write rates and optional temperature |
| Network | Adapter and receive/transmit rates; optional on-demand latency test |
| Power | AC/battery state, active scheme |

Sample interval selectable: 1/2/5 seconds. Use monotonic timestamps for rates where necessary and UTC timestamps for persistence. Unavailable measurements are `null` with quality/status metadata, **not zero**. Show measurement units, sensor source and last-update age; show a stale warning if polling stops. Display trends in bounded live charts; provide pause/resume. The app must distinguish a measured `0` from `null`/not available. Sampling and DB persistence cadences may differ (default persistence: 5 seconds).

**Acceptance:** Charts update while app is responsive; dual-GPU identification works; stopping a provider degrades affected metrics only; minimized state follows user's tray/background preference; bad readings are rejected or marked invalid.

### FR-03 — Manual performance profiles and restoration (P0)

Show existing Windows power schemes by actual GUID and name; identify the active scheme. Built-in app profiles (`Balanced`, `Gaming Boost`, `Power Saving`) are **labels/mappings** to schemes available on the current machine, not a promise that Windows contains named schemes. Missing schemes must not be silently created or forced. The user selects the actual scheme for each supported profile. A `Custom` profile may store user-selected supported settings, initially limited to existing power scheme mapping and AC-only rule.

Application sequence: check support/power source/permission; persist the currently active scheme and a pending transaction **before** applying; apply the selected GUID; read back the active scheme to verify; mark session active. On disable, restore the saved GUID if still available **and** if the active scheme has not changed externally. If another app or user changed the scheme, show the mismatch and ask before restoring. On command failure, report failure and attempt rollback. On crash/restart, present an unfinished-session recovery prompt; never overwrite newer user changes without consent.

**Acceptance:** Toggle applies only supported existing scheme; prior scheme can be restored; failed apply is visible; user changes while Boost is active are respected; crash recovery is idempotent. A scheme change is not claimed to guarantee increased FPS.

### FR-04 — Smart temporary-file cleaner (P0)

**Purpose:** Reclaim disk space, not indiscriminately clear caches to "free RAM" or boost FPS.

Initial allowed category: eligible old files in the **current user's temporary directory**. Optional later categories: explicitly supported application/browser cache (browser must be closed and cache-only paths verified), Windows cleanup exposed through supported OS facilities, and Recycle Bin with separate confirmation. Documents, Desktop, Downloads, login sessions, cookies, passwords and files from other user accounts are never in default cleaner scope.

**Two-phase workflow:**

1. `scan`: select known categories; canonicalize and validate allowlisted roots; enumerate eligible entries without following links/junctions/reparse points outside approved roots; reject unsupported directories; calculate estimated bytes, count, age and skipped/in-use entries. Default auto-eligibility is last modified more than 24 hours ago (configurable for supported category).
2. `preview`: show category sizes, known exclusions and warnings, then obtain explicit user confirmation. The scan produces a short-lived opaque plan ID; UI may choose supported category IDs but cannot submit arbitrary filesystem paths.
3. `clean`: revalidate root, entry type, identity/metadata, age, links, permissions and current file status immediately before each action; skip changed/locked/unsafe files; prefer a narrowly scoped OS-supported mechanism where relevant. Report real deleted-file counts, actual bytes released where measurable, skipped counts and errors. Never imply that all deletions can be undone.

No automatic cleanup by default. No recursive deletion of arbitrary paths. A cleanup operation can be cancelled and may produce partial results that must be accurately displayed. Store summaries, not full scanned private paths, in SQLite.

**Acceptance:** Directory traversal/symlink redirection cannot escape allowlist; newly changed/locked files are skipped; deleting a selected category cannot affect another; invalid/stale plan IDs are rejected; user sees estimated versus actual reclaimed space and partial failures.

### FR-05 — Process viewer (V1)

List process name, PID, approximate CPU/RAM usage and executable identity when accessible; sort/search. Do not equate high memory use with unnecessary memory use. User-initiated process termination, if implemented, needs confirmation, a protected-process exclusion list, owner/access validation and graceful-close preference. **Never** terminate background apps automatically as part of Boost.

### FR-06 — Registered games and opt-in Auto Boost (V1)

User registers a game through an executable picker, chooses a mapped power profile, enables/disables Auto Boost and optional AC-only/restore-on-exit settings. Validate matching running processes by canonical executable path and process identity when accessible; name-only matches are insufficient. Detect starts/exits with bounded polling; do not inject code or interfere with anti-cheat.

On the first applicable game start, create a tuning session and snapshot the active scheme. While multiple registered games are running, avoid repeated profile thrashing; use a documented deterministic conflict policy (initially one global active Auto Boost profile, chosen at first activation). When the final relevant game ends, restore according to FR-03. If AC-only condition ceases to hold, reevaluate and safely return to the prior scheme. Auto Boost is opt-in and off by default.

**Acceptance:** A similarly named unrelated executable does not trigger Boost; repeated detection does not reapply; two games do not cause early restore; app crash recovery does not clobber a user's intervening manual change.

### FR-07 — Local history and analytics (MVP basics; V1 full)

MVP stores timestamped valid sensor readings and exposes short-range charts; V1 adds min/avg/max and 5 min/15 min/1 hr/24 hr/custom views. Null readings are excluded from averages and missing sensors are labeled as such. Aggregate raw samples to minute summaries before expiring raw data. Proposed defaults: raw samples 24 hours, minute aggregates 30 days, cleaner/tuning activity 90 days, session summaries until manual deletion; expose user-configurable retention and manual purge. Maintenance runs incrementally and must not stall charts.

**Acceptance:** Historical queries use indices and pagination/downsampling; app can restart without losing committed samples; changing retention takes effect; DB size remains bounded under the configured policy.

### FR-08 — Gaming session analysis (V2)

With explicit opt-in, associate verified game process and hardware-sampling window with optional compatible frame-time capture. Store FPS/frame-time values **only** when provided by a validated capture source; no estimation from GPU load. Show average FPS, clearly defined 1% low calculation, frame-time spikes, hardware trends and synchronized timestamps. Provide session comparison only with context (resolution/settings/test conditions if supplied). Show possible correlations, never definitive bottleneck diagnoses from single metrics. Failure or unavailability of frame-time capture must not prevent ordinary hardware monitoring.

### FR-09 — Settings and accessibility (P0)

Settings: dark/light/system theme, refresh cadence, sensor/device selection, background/tray monitoring, startup (off by default), history retention, optional notifications, explicit cleanup categories, profile mapping and app data reset/export. Show keyboard focus, labels and high-contrast accessible status indicators; avoid relying on color alone. No telemetry or cloud sync by default.

## 7. Screens and navigation

```text
Dashboard
Hardware Monitor
  ├─ CPU / GPU (adapter selector) / Memory / Storage / Network
Performance Tuner
  ├─ Available power schemes / mapped profiles / active session / Restore
Smart Cleaner
  ├─ Scan / Preview & confirmation / Results / History
System Information
Analytics (basic MVP; expanded V1)
Process Manager (V1)
Gaming (V1)
  ├─ Registered games / Auto Boost / Sessions (V2 metrics)
Settings
```

**Dashboard:** prominent device/active scheme/AC state, current sensor cards, last-update status, a switch to a supported mapped profile, and recent charts. Do not show unsupported functionality as clickable. Every potentially destructive action has clear preview, consent and progress/error state. Display disk-space cleaning separately from FPS/performance claims.

## 8. SQLite data design

### 8.1 Location and connection

Use Tauri's per-user app-data location to resolve the SQLite path (for example, a `data/performance.db` under the app's local data directory); never hard-code another user's Windows account path or put the DB under Program Files. Rust owns database reads/writes and migrations. Use parameterized queries, transaction boundaries and migrations. Set `foreign_keys=ON` on **every connection**; enable WAL and a busy timeout at DB initialization. Maintain one managed writer or a bounded pool and batch sensor inserts. ISO-8601 UTC `TEXT` timestamps are acceptable if consistently normalized; integer epoch milliseconds are an alternative, but do not mix formats within a column.

**Migration ordering:** Create base tables (`devices`, `performance_profiles`, `gaming_sessions`, `hardware_sensors`) before foreign-key-dependent tables. Version schema via migrations, not by silently dropping user data. All IDs are opaque UUID/ULID `TEXT` except append-only sample/log row IDs.

### 8.2 Logical tables

| Table | Purpose / key fields |
|---|---|
| `app_settings` | `key` PK, JSON-string `value`, `updated_at` |
| `devices` | `id` PK, display metadata, OS, CPU summary, RAM, `created_at`, `updated_at` |
| `device_capabilities` | `id` PK, `device_id` FK, `capability_key`, `status`, `reason`, `detected_at`; unique `(device_id, capability_key)` |
| `hardware_sensors` | `id` PK, `device_id` FK, unique `(device_id, provider, sensor_key)`, hardware identity, unit, availability |
| `hardware_samples` | integer PK, `sensor_id` FK, `gaming_session_id` nullable FK, `recorded_at`, `value` (only valid readings), quality if needed |
| `hardware_sample_aggregates` | integer PK, `sensor_id` FK, interval, count/min/max/avg; unique `(sensor_id, interval_start, interval_end)` |
| `performance_profiles` | `id` PK, label, `profile_type`, `created_at`, `updated_at` |
| `profile_settings` | `id` PK, `profile_id` FK, `setting_key`, `setting_value`; unique `(profile_id, setting_key)` |
| `tuning_sessions` | `id` PK, `profile_id` FK, previous/applied scheme GUID, trigger, status, start/end/restore timestamps, error |
| `cleaning_scans` | `id` PK, category snapshot, estimate/count, status, timestamps, plan expiry |
| `cleaning_results` | `id` PK, `scan_id` FK, category, deleted/skipped counts, recovered bytes, status, finished time |
| `registered_games` | `id` PK, display name, canonical executable path, `profile_id` FK, opt-in/restore/AC preferences, timestamps |
| `gaming_sessions` | `id` PK, `game_id` FK, start/end, status, optional active tuning-session ID |
| `gaming_session_metrics` | `id` PK, `session_id` unique FK, captured FPS/frame-time aggregates and hardware summary; metrics nullable |
| `activity_logs` | integer PK, event type/status/message, UTC timestamp; never store tokens or sensitive file inventories |

A GPU list should not be squeezed into a single `gpu_model` string if adapter-level identity is required; persist detailed adapter/sensor identities separately. The scanner's short-lived file plan and per-file metadata should live in a bounded protected in-memory structure, **not** a client-editable SQLite list of deletion targets.

### 8.3 Core migration example (illustrative, not the entire schema)

```sql
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS app_settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS devices (
  id TEXT PRIMARY KEY,
  device_name TEXT,
  cpu_model TEXT,
  ram_total_bytes INTEGER,
  operating_system TEXT,
  os_version TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS performance_profiles (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  profile_type TEXT NOT NULL CHECK(profile_type IN ('system', 'custom')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS gaming_sessions (
  id TEXT PRIMARY KEY,
  game_id TEXT, -- add game_id FK in full migration after registered_games is created
  started_at TEXT NOT NULL,
  ended_at TEXT,
  status TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS hardware_sensors (
  id TEXT PRIMARY KEY,
  device_id TEXT NOT NULL REFERENCES devices(id),
  provider TEXT NOT NULL,
  sensor_key TEXT NOT NULL,
  sensor_name TEXT NOT NULL,
  unit TEXT,
  is_available INTEGER NOT NULL DEFAULT 1 CHECK(is_available IN (0,1)),
  UNIQUE(device_id, provider, sensor_key)
);

CREATE TABLE IF NOT EXISTS hardware_samples (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  sensor_id TEXT NOT NULL REFERENCES hardware_sensors(id),
  gaming_session_id TEXT REFERENCES gaming_sessions(id),
  recorded_at TEXT NOT NULL,
  value REAL NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_samples_sensor_time
  ON hardware_samples(sensor_id, recorded_at);
CREATE INDEX IF NOT EXISTS idx_samples_session
  ON hardware_samples(gaming_session_id);
```

The production migration must build all tables in dependency order, add the actual `gaming_sessions.game_id -> registered_games.id` foreign key and create all remaining tables and indices. Do not execute the illustrative `gaming_sessions` definition as the final production schema.

### 8.4 Write/read policy

Live values stay in memory; persist approximately every 5 seconds using batch transactions. Read history with bounded time range and chart downsampling. SQLite is local, not an IPC transport; never expose a generic `execute_sql` Tauri command. Check and safely recover from DB errors; offer diagnostics/export of existing data rather than silently resetting a corrupt DB.

## 9. Tauri command/event contract

| Command | Input / behavior |
|---|---|
| `get_system_information` | Read current device summary |
| `detect_hardware` | Trigger validated re-detection; return capabilities |
| `get_device_capabilities` | Read supported/unsupported/permission state |
| `get_hardware_snapshot` | Return latest values including status/source/time |
| `start_monitoring` / `stop_monitoring` | Start/stop subscribed monitoring according to preferences |
| `get_hardware_history` | Validated sensor IDs, bounded UTC time range, pagination/downsampling |
| `get_power_plans` / `get_active_power_plan` | Read actual Windows schemes/current GUID |
| `activate_performance_profile` | Validated stored profile ID only; create reversible tuning session |
| `restore_previous_profile` | Validated tuning session ID; check external changes |
| `scan_cleanable_files` | Allowed category IDs only; return summary and opaque expiring plan ID |
| `execute_cleanup` | Valid plan ID + user-confirmed allowed categories; server-side revalidation |
| `get_cleaning_history` | Paged local result summaries |
| `register_game` / `update_registered_game` | File-picker-derived game path, validated stored profile, opt-in settings (V1) |
| `get_registered_games` / `set_auto_boost` | Per-game opt-in only (V1) |
| `get_gaming_sessions` / `get_gaming_session_metrics` | Historical summaries (V2 metrics) |
| `get_settings` / `update_settings` | Typed, validated settings keys/values |

Events: `hardware:update`, `monitoring:status`, `cleanup:progress`, `cleanup:completed`, `tuning:changed`, `game:detected`, `app:error`. Include timestamp and machine-readable status; event payloads never contain arbitrary system-command instructions. Define strongly typed TypeScript/Rust payloads and version the sidecar protocol.

## 10. Security, permissions and data handling

- Least privilege: keep UI/core unelevated; broker only strictly required privileged operations with user consent.
- Strict Tauri capabilities: no broad shell or filesystem grants; validate each command in Rust regardless of frontend state.
- Path safety: allowlisted roots, no arbitrary client paths, recheck canonical path/reparse points and entry identity immediately prior to deletion; skip locked, changed, protected or unfamiliar files.
- Process safety: no game injection or blanket termination; distinguish game identity by resolved path and process metadata; respect anti-cheat terms.
- Power safety: snapshot before mutate, verify after mutate, persist recovery state, protect against conflicting external changes.
- Privacy: no cloud account, analytics upload, serial-number collection, or telemetry by default. Provide explicit export/delete history controls and scrub private details from logs.
- Dependencies: review sidecar redistribution licenses and security of third-party binaries; pin versions, verify downloads, test upgrades and sign release artifacts where practical.

## 11. Error, recovery and edge cases

| Scenario | Expected behavior |
|---|---|
| Sensor absent or requires administrator | Show `Unavailable` or `Requires permission`; other metrics still work |
| Sidecar crashes/disconnects | Mark affected readings stale, bounded restart, show error |
| Two GPUs report similarly named sensors | Keep separate provider/device keys; never merge incorrectly |
| Device switches AC to battery while AC-only boost active | Reevaluate profile and restore conservatively, communicating change |
| Windows/OEM changes power scheme during boost | Do not overwrite silently; show conflict and require confirmation |
| App crashes during boost | Detect unfinished durable session on next launch; offer safe recovery |
| Another game remains running | Do not restore until no applicable sessions remain |
| Cleaner scan plan expires or files changed | Rescan/reject unsafe entries; report skipped instead of deleting blindly |
| App lacks file permission | Skip item and report partial result; no global elevated retry |
| Database is locked/corrupt/full | Keep live monitoring if possible; surface persistence failure; do not discard recovery metadata silently |
| Gaming FPS provider unsupported | Show `FPS unavailable`; hardware monitoring remains functional |

## 12. QA and acceptance test matrix

| Area | Essential tests |
|---|---|
| Discovery | Desktop vs laptop; AMD/Intel; integrated/dedicated/dual GPU; no admin; missing sensors |
| Monitoring | Real zero vs null; rate conversions; provider restart; long-run resource use; sleep/resume |
| Profiles | Available/missing GUIDs; denied elevation; external override; multiple activations; interrupted restore |
| Cleaner | Allowlisted old temp file; new file; in-use file; symlink/junction escape; race between scan and clean; cancellation; empty results; partial errors |
| Game watcher | Same exe name at different path; rapid starts/exits; multiple games; AC disconnect; app restart |
| SQLite | Migrations, FK enforcement, batch writes, retention, WAL recovery, data export, large-history query |
| UI/accessibility | Disabled capability explanation, confirmations, keyboard navigation, stale-data indicators, light/dark themes |
| Distribution | Fresh install, upgrade preserving DB, uninstall policy, dependency/runtime compatibility, standard-user account |

**Definition of done for each feature:** implementation + typed interface + error handling + unit/integration tests + user-visible status + documented limitations; destructive or system-mutating features additionally require restore/recovery tests.

## 13. Implementation roadmap

### Phase 0 — Foundation

Initialize Tauri 2 + React/TypeScript, project structure, typed IPC, logging, app-data directory, SQLite migration layer, CI/build and basic UI shell. Build a fake sensor provider for development and demos.

### Phase 1 — Monitoring MVP

Build discovery and capabilities, Windows basic metrics, sensor sidecar adapter, live dashboard, adapter selection, availability/stale indicators, bounded sampling and SQLite history. Test on a desktop and a hybrid-GPU laptop.

### Phase 2 — Safe manual tuning + cleaner MVP

Implement Windows scheme discovery, mapped profiles, durable restore transactions, recovery UI and AC check. Add allowlisted user-temp scan/preview/confirmed cleanup with race-safe revalidation, progress and history. Release an internal MVP.

### Phase 3 — V1

Add registered games, opt-in process watcher/Auto Boost with overlap management, process viewer and longer-range aggregated analytics; measure app resource impact during gameplay and revise sampling budgets.

### Phase 4 — V2

Add optional validated FPS/frame-time provider, linked gaming sessions, session comparison and report export. Label diagnosis as correlation with clear measurement limitations.

## 14. Risks and decisions to validate early

| Risk / open decision | Mitigation / next experiment |
|---|---|
| Sensor availability varies by hardware and permissions | Prototype read-only sidecar on representative AMD/NVIDIA desktop and laptop; map gaps before promising metrics |
| OEM performance/boost settings differ from Windows schemes | MVP only controls verified Windows schemes; OEM control remains excluded without documented API |
| Hardware polling might affect game performance | Benchmark CPU/RAM/latency with polling 1/2/5s and tune default rates |
| Cleanup may harm app data or save time only briefly | Start with current-user temp allowlist, exclusions, preview and explicit consent; avoid unsupported cleanup promises |
| Sidecar/runtime packaging and license obligations | Confirm .NET deployment model and transitive redistributable licenses before publishing installer |
| SQLite raw sensor data can grow quickly | Batch insert, downsample/aggregate, retention and indexed time-range queries |
| FPS capture compatibility/anti-cheat constraints | Make opt-in, read-only, separate V2 provider with fallback and game-specific compatibility testing |

## 15. Final MVP release checklist

- [ ] Installer opens under a standard Windows user without global administrator privileges.
- [ ] Dashboard detects and displays supported CPU/GPU/RAM/disk/network metrics; unavailable sensors are clearly labeled.
- [ ] Sensor errors do not crash UI, and monitoring does not require external cloud/backend services.
- [ ] SQLite migrations, batched history and bounded retention work across upgrades.
- [ ] App lists actual power schemes, applies a valid selected scheme and can safely restore the prior GUID.
- [ ] Crash and external-setting-change scenarios do not silently overwrite user choices.
- [ ] Cleaner previews eligible current-user temporary files, requires confirmation and cannot leave the allowlisted scope.
- [ ] Cleanup shows actual outcome and partial/skipped failures without promising FPS improvement.
- [ ] All system-changing actions are logged, permission-checked and documented.
- [ ] Windows desktop and hybrid-GPU laptop smoke tests and fresh-install/upgrade tests pass.
