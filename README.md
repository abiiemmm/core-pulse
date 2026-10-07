# Core Pulse

A Windows-first, offline desktop app based on [PRD.md](PRD.md). The source tree contains a Tauri 2 Rust core and a React/TypeScript UI. SQLite is the only database.

## Current implementation

- Windows device identity, CPU/RAM/disk/network readings, a bundled read-only LibreHardwareMonitor GPU provider, persistent GPU selection, explicit unavailable states, a live dashboard, and bounded charts.
- Analytics over 15 minutes, 1 hour, 6 hours, 24 hours, or 3 days: retained history, utilization/temperature/network charts, sensor visibility, weighted statistics, and CSV clipboard export.
- Persistent dark/light/system appearance and Indonesian, English, or Spanish interface language, available from the toolbar and Settings. Number and date formatting follows the selected language.
- SQLite migration with foreign keys, WAL, batch sample writes about every five seconds, indexed history, minute aggregates before raw retention, manual monitoring-history purge, and local settings.
- A read-only process resource viewer with search, CPU/RAM sorting, pagination, executable paths where accessible, and independent pause/refresh.
- A local Gaming library with a native executable picker, persistent registration/edit/removal, verified process identity, and per-game running-process counts. Registration leaves Auto Boost off. Explicit confirmation enables a global automatic tuning session with overlap, AC and restoration policies; local session history supports deletion of completed summaries.
- Independent, bounded history persistence with visible degradation/recovery and background retention.
- Existing Windows power-plan discovery, profile-to-GUID mapping, durable pending sessions, verified application, restore, conflict detection, and startup recovery prompt.
- Current-user temp scan for files older than 24 hours, expiring in-memory plan IDs, preview/confirmation, handle-based Windows deletion, root/file identity validation, pinned directory ancestry, hard-link/reparse exclusions, live progress, cancellation, and summary history.
- A clearly labeled browser demo provider for UI development. Demo readings are simulated and cannot change the system.

## Important release gaps

This is an internal build. GPU temperature and utilization are integrated with an isolated, self-contained sensor host and per-adapter history. CPU temperature is withheld because the upstream driver API can return a zero-filled buffer on failure; verified per-call CPU driver access is still required. Registered games and read-only process matching are implemented. Auto Boost runtime, explicit opt-in controls and local session history are implemented and covered by native-process fixtures with simulated power operations. Native opt-in, overlap, manual recovery, minimization and shutdown checks now pass while using the already-active Windows scheme. Actual scheme changes and physical AC transitions, optional FPS/session analysis, cross-version installer upgrades, broader hardware coverage, long-run checks, and signing remain open. See [PRODUCTION.md](PRODUCTION.md) for current evidence and remaining work.

The app never presents unavailable readings as zero or claims that cleaning improves FPS.

## Build

Requires Node.js 22+, Rust stable with the Windows MSVC toolchain, Visual Studio C++ Build Tools, and WebView2. Building the sensor host requires the exact .NET SDK 10.0.401 specified in `global.json`. An SDK under `.tools/dotnet` is supported; otherwise install it normally. `tauri:dev` and `tauri:build` restore locked sensor dependencies, run the host self-test, and bundle the self-contained runtime. End users do not need a separate .NET installation.

```powershell
npm ci
npm run sensors:build
npm run build
npm run check
npm run tauri:dev
npm run tauri:build
```

Run `npm run dev` for the labeled web preview. It has no access to Windows metrics or system changes.

The database is created in Tauri's per-user app-data directory as `performance.db`. No account, telemetry, or cloud service is used.

## Repository workflow

The sole maintainer is [@abiiemmm](https://github.com/abiiemmm). See [CONTRIBUTING.md](CONTRIBUTING.md) for the branch, commit, dependency, and verification workflow, and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for component boundaries and extension guidance.

Git tracks source and lockfiles. Installer binaries, SDKs, dependency caches, generated schemas, databases, and local QA evidence under `artifacts/` are excluded. Paths to evidence below refer to the development workspace. Windows [GitHub Actions](https://github.com/abiiemmm/core-pulse/actions/workflows/build.yml) builds an unsigned internal installer and uploads it with a SHA-256 manifest as a downloadable run artifact.

## Safety notes

- The UI cannot supply an arbitrary command or cleanup path. Power operations accept only stored profile IDs mapped to schemes currently returned by Windows. Cleanup accepts only `user_temp` and an opaque scan plan ID.
- The power session is committed before calling the native Windows power API. On restart, unfinished sessions are displayed; restore checks for outside changes.
- Cleanup uses the current user's Windows Local App Data `Temp` folder; custom `TEMP` locations and a redirected/junction `Temp` folder are excluded. No client-supplied or environment-controlled deletion root is accepted.
- Cleanup validates full Windows file IDs, length, modification time, link count, and the final path through an exclusive handle. It deletes through that same handle while parent directories remain pinned. Changed, locked, hard-linked, and reparse entries are skipped. It does not delete directories. Failed and skipped items are counted separately.
- Retention moves only complete minutes, merges delayed samples with weighted statistics, and commits aggregation/deletion in one SQLite transaction. A failed maintenance operation preserves raw samples.
- `recovered_bytes` is the sum of successfully deleted file lengths, not a guaranteed change in free disk space.
- Some Windows sensors require hardware-specific providers or permissions. The main application runs unelevated.

## Validation in this workspace

The frontend build and 94 Rust library tests pass. The previously verified native debug build is being refreshed for the bounded-discovery change. Repository checks enforce sole-maintainer commit authorship and reject co-author trailers. `npm run check` also verifies the SQLite schema, IPC registration, analytics weighting, missing readings versus measured zero, CSV serialization, and translation placeholders. The previous native Tauri/WebView2 review covered all nine pages in three languages at 1440 × 860 and 860 × 610, all analytics ranges and chart groups, sensor visibility, theme switching, and preferences after reload. Results are stored in `artifacts/redesign/feature-verification.json`. That review predates the new Auto Boost and session-history controls. A separate 44-check native review now covers these controls, three-language status/history/armed editors, compact footer access, overlap, monitoring pause, actual minimization, manual recovery and graceful close. Its target equals the already-active power scheme, so it does not establish actual Windows scheme switching or physical AC transitions. Evidence is in `artifacts/auto-boost/native-verification.json` and `shutdown-verification.json`.

For native visual checks, start Tauri with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`, then run `node scripts/inspect-desktop.mjs features`. Signing, cross-version upgrades, and broader hardware coverage remain outside this verification.

Run `node scripts/inspect-desktop.mjs cleaner` to check cleanup progress, confirmation, cancellation, terminal results, unrelated-event rejection, and compact layouts in Indonesian, English, and Spanish. That mode simulates the cleanup command boundary inside the native WebView and deletes no user files. Actual Windows deletion and junction tests use isolated workspace fixtures in the Rust suite. The UI report and screenshots are in `artifacts/hardening/`.

The unsigned internal x64 installer is available at `artifacts/releases/CorePulse-0.1.0-internal-x64-setup.exe`. Its build passed and the standalone release executable opened with the Vite server stopped. Current-user fresh installation, same-version reinstallation, and uninstall passed with an unelevated token while preserving the existing SQLite files. Cross-version upgrades, signing, verified CPU temperature access, and other production gaps remain open.

Run `node scripts/inspect-desktop.mjs stability` for native process search/sort/pause, multilingual layouts, and storage-status rendering. Real process readings are used; storage notice events are simulated. Isolated Rust fixtures exercise actual persistence failures. New evidence is saved in `artifacts/stability/`.

Run `node scripts/inspect-desktop.mjs sensors` for real GPU selection, host interruption/recovery, pause/resume, and multilingual sensor controls. That QA mode requires `artifacts/sensors/launch.json` with the PID of the workspace release executable; it stops only that application’s verified sensor child. Sensor sources, locked dependencies, license notices, and self-contained runtime resources are prepared by `npm run sensors:build`.

Run `node scripts/inspect-desktop.mjs games` for native picker cancellation/selection, registration, overlapping verified processes, removal without stopping a game, and multilingual theme/layout checks. It requires `artifacts/games/native-launch.json` with the QA application PID and an executable fixture under `artifacts/games/`; it removes only its own registration and restores the prior preferences. No power scheme is changed. Evidence is saved in `artifacts/games/native-verification.json`.

Run `node scripts/inspect-desktop.mjs auto-boost` for the new native lifecycle checks. Prepare `artifacts/auto-boost/native-launch.json` with the workspace debug application PID, Vite PID and two copied executable fixtures under `artifacts/games/`. Before launching, save a consistent SQLite backup and capture the original `app_settings`, `profile_settings`, `registered_games`, `tuning_sessions` and `gaming_sessions` rows in `database-before-native.json`. The verifier refuses existing unfinished tuning or armed registrations. It temporarily maps Gaming and Balanced to the current power GUID, exercises actual picker/opt-in/manual recovery controls and closes the QA application while a fixture is running. `verify-auto-shutdown.py` then verifies durable restoration and restores the exact preferences/profile rows while deleting only manifest-identified fixture registrations/history; it retains all power recovery and hardware samples. Stop the recorded Vite process after QA. The scoped window helper selects the PID-owned `Tauri Window`; `Get-Process.MainWindowHandle` can instead identify Tao's internal window.
