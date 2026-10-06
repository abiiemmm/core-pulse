# Auto Boost runtime and verification gates

The controller in `src-tauri/src/automation.rs` is connected through `automation/runtime.rs` to independent discovery and protection workers, typed IPC, translated opt-in controls and local session history. Registration continues to store `auto_boost=0`; automatic power changes require explicit confirmation for each game. Runtime fixtures use real Windows executable/process identity and simulated power operations. Native UI integration, actual scheme-change lifecycle and physical AC transition evidence remain required before completing FR-06.

## Global session policy

A verified running game creates one local gaming session across its concurrent matching processes. An unknown process or an unavailable executable never supplies a new start. A previously observed game remains open while its exit cannot be verified. A complete observation of zero processes ends the gaming session; removing a registration ends its observation without terminating the executable.

Eligible games have Auto Boost enabled and a mapped profile. Their individual AC setting and the profile's AC setting both apply. When several eligible games start in the same poll, registration time followed by registration ID determines the first candidate. The first available profile pins one global tuning session, including its AC and restore-on-exit policies. Later participants link to that tuning session instead of applying another scheme. The original scheme is restored after the last eligible participant leaves, subject to the pinned restoration policy.

AC loss restores an AC-only global session even when restore-on-exit is disabled. Unknown power source is treated as ineligible for AC-only activation. Returning to AC does not silently rearm that same cycle. A manual restore, outside scheme change, disabled cycle or failed activation also suppresses further automatic applications until the cycle's observed games have exited or been removed. Switching opt-in off and back on while the same game remains running does not bypass suppression. Manual recovery before the first watcher observation is applied to that next observation as well.

With restore-on-exit disabled, the selected scheme and unfinished tuning session remain visible for manual restoration. Closing the app restores only its current automatic session when that session's policy permits restoration. An external scheme change is never forced back by automatic recovery or shutdown.

## Persistence and ownership

Gaming starts, ends and startup interruption records use SQLite transactions. A pending tuning record and all initial gaming-session links commit together before any Windows scheme mutation. The transaction rechecks that linked games remain registered, identified and opted in. Storage failure or stale/disarmed registrations roll back the whole pending transaction.

The existing power controller verifies application and conservative restoration. The manual AC watchdog excludes game-owned sessions so that two controllers do not compete. Prior-runtime unfinished tuning records require review and never grant current automatic ownership. Startup marks prior open gaming sessions interrupted without changing Windows power settings.

Stable polling avoids redundant linkage writes. Active linkage and cycle tracking discard ended observations and stay bounded by the registered-game limit. The UI shows the latest 50 historical gaming summaries. Only completed summaries can be deleted; deletion removes their derived metrics, detaches retained hardware samples and preserves tuning recovery records. Archived registrations are pruned only when no history references them.

## Verification scope

Controller tests use an isolated SQLite database and a simulated power backend. That backend verifies the database is unlocked during each OS operation and that intent and game linkage are already durable. Tests cover overlap, multiple processes, deterministic profile selection, missing mappings, opt-in defaults, uncertain discovery, AC eligibility and disconnect, manual cancellation before/after startup, opt-in toggling, outside changes, transaction/link failures, denied mutation, crash recovery, retained profiles, shutdown and bounded tracking during continuous overlap.

The 91-test Rust suite includes five runtime fixtures using real picker-token registration and Windows process handles with simulated power operations. These cover multiple matching processes, final exit, manual restoration and AC protection while discovery is blocked, responsive shutdown, expired/configuration-invalidated observations, and last-moment liveness checks. Three registration/history fixtures additionally cover stale confirmation settings, live-handle cache views and deletion that preserves hardware samples and recovery records. The native debug and frontend production builds pass, and IPC checks cover 35 commands. These results establish fixture integration; they do not verify the new native UI flows, actual Windows power mutation or a physical AC transition.

## Runtime and UI behavior

Discovery runs every two seconds on its own worker and retains one latest observation. Protection runs every second even when executable discovery is blocked. The controller rejects observations older than five seconds or captured before a configuration change. Freshness is checked after potentially waiting for SQLite; activation also verifies current eligible settings and held process liveness both before and after pending intent commits. Slow executable inspection cannot hold the automatic operation lock and delay explicit manual restoration.

Configuration commands serialize with automatic decisions. Enabling verifies the executable before taking operation locks and compares the confirmed settings with the current registration. Armed profile, AC and restoration settings require disarming before editing. Monitoring pause, the selected page and window visibility do not control either background worker. Prior unfinished recovery continues to require explicit review.

The Gaming page supplies enable confirmation, per-game AC/restoration settings, translated controller/discovery status, a link to manual recovery and local session history. A graceful main-window close stops automatic decisions, restores the current automatic session when its policy allows and closes active gaming summaries. Discovery does not need to finish for shutdown restoration. A failed restore retains durable recovery for the next launch.

## Remaining verification

- Check new native opt-in, status, editor and history UI flows in all three languages, both themes and compact/regular layouts. Previous Gaming screenshots cover registration and matching only.
- Verify the full native pipeline with actual mapped Windows schemes and picker-selected fixtures, overlapping game processes, manual cancellation, minimization and app close/restart. Always restore the original scheme and preserve user preferences and recovery records.
- Verify physical AC transitions, sleep/resume and sustained resource behavior on broader hardware. Simulated power fixtures do not establish these results.
