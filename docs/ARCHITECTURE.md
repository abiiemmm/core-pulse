# Architecture and extension boundaries

Core Pulse is a Windows desktop application with three components. The React UI communicates with the Tauri Rust core through registered IPC commands and events. The Rust core owns SQLite and Windows operations. A separate read-only .NET sensor process supplies hardware readings through a validated protocol.

| Directory | Responsibility | Extension rule |
| --- | --- | --- |
| `src/` | React views, navigation, settings, localization, analytics | Put frontend IPC in `api.ts` and shared payloads in `types.ts`; keep Windows operations in Rust. |
| `src-tauri/src/` | Commands, persistence, processes, sensors, power control, cleanup | Extend the relevant module; keep Windows-specific implementations under their module boundaries. |
| `sidecar/` | Isolated hardware provider, locked dependencies, source manifest and notices | Preserve read-only access, bounded frames, explicit availability, and protocol validation. |
| `tests/` | SQLite and frontend/backend command contracts | Update these checks when command registration or schema behavior changes. |
| `scripts/` | Builds, analytics checks, native QA and packaging checks | Generate local evidence under ignored `artifacts/`; avoid machine-specific source configuration. |
| `.github/` | CI, ownership and review template | Keep workflow permissions minimal and action versions pinned. |
| `docs/` | Engineering guides | Keep feature status in `PRODUCTION.md` and requirements in `PRD.md`. |

## Runtime flow

`App.tsx`, `views.tsx`, and specialized views render state managed by `usePulse.ts`. `api.ts` centralizes Tauri calls and the labeled browser demo. `i18n.ts` and `locales.json` cover Indonesian, English, and Spanish; analytics calculations live in `analytics-data.ts`.

`src-tauri/src/lib.rs` wires application state, commands, events, and background workers. `models.rs` defines shared backend payloads. `db.rs` owns schema and query behavior; `persistence.rs` separates live sampling from bounded history writes and retention. Hardware discovery, process snapshots, power sessions, and cleanup use their own modules. Do not hold the database mutex across blocking Windows operations.

`sensors.rs` supervises the packaged sensor host with frame validation, process containment, timeouts, and bounded restart backoff. `sidecar/Program.cs` implements the provider. Missing or unverifiable readings remain unavailable; never replace them with zero. Sensor identity must constrain both live readings and retained history.

`games.rs` owns local registrations and ephemeral native-picker tokens. `games/windows.rs` validates PE files, resolves file identity from held handles, and holds read-only process handles so PID reuse does not replace a tracked process. Gaming discovery runs outside the SQLite lock. Registration defaults to automatic tuning off; only the power module may change Windows schemes. Cached observations carry held process handles, and their views recheck liveness against current registration metadata.

`automation.rs` owns the global Auto Boost lifecycle and gaming-session persistence on top of the power controller's durable protocol. `automation/runtime.rs` separates process discovery from AC protection and serializes configuration, manual restore and controller decisions. One cached observation is bounded by age and configuration generation; activation rechecks those bounds, current settings and held process liveness before and after pending intent is committed. Executable inspection stays outside the controller operation lock. Graceful close invokes conservative restoration on a worker. Its policies, evidence and remaining verification gates are in [AUTO_BOOST.md](AUTO_BOOST.md).

## Adding a feature

Start with a clear payload and ownership boundary. Add an IPC command only when the UI needs backend behavior, register it in `lib.rs`, and update `api.ts`, `types.ts`, and command checks together. Add database changes through the existing migration mechanism and verify existing data survives. Keep rendering and calculations separate so calculations can be checked without starting the desktop app.

As a feature grows, give it a focused module or directory instead of expanding shared startup code. Preserve the existing import and command contracts during a move; make broad structural changes separately from behavior changes. Add translation keys in all three languages and cover changed interpolation placeholders.

Operations that change Windows state need explicit user intent, constrained inputs, durable recovery where applicable, and meaningful failure tests. Cleanup accepts opaque scan IDs rather than arbitrary paths. Power changes operate on discovered Windows schemes and detect external conflicts before restoration.

## Build and artifact lifecycle

The npm, Cargo, and NuGet lockfiles are checked in. The .NET SDK is pinned in `global.json`. Build the sensor host before a fresh Rust/Tauri check because `tauri.conf.json` bundles `sidecar/publish/`. Dependency source archives are downloaded from `source-manifest.json` and verified by hash; their notices and sources are bundled with the installer.

Git tracks source, configuration, lockfiles, and static icons. CI produces an unsigned x64 installer and checksum manifest as downloadable Actions artifacts. User databases, private QA evidence, generated schemas, runtime bundles, and portable toolchains stay outside Git. See [CONTRIBUTING.md](../CONTRIBUTING.md) for the workflow and [PRODUCTION.md](../PRODUCTION.md) for release limitations.
