# Auto Boost controller and integration gates

The controller in `src-tauri/src/automation.rs` is prepared and tested against the real SQLite and power transaction code. It is not connected to a runtime worker or frontend opt-in command yet. Registration continues to store `auto_boost=0`, and the application still exposes no automatic power changes. This is implementation progress toward FR-06, not completion of that requirement.

## Global session policy

A verified running game creates one local gaming session across its concurrent matching processes. An unknown process or an unavailable executable never supplies a new start. A previously observed game remains open while its exit cannot be verified. A complete observation of zero processes ends the gaming session; removing a registration ends its observation without terminating the executable.

Eligible games have Auto Boost enabled and a mapped profile. Their individual AC setting and the profile's AC setting both apply. When several eligible games start in the same poll, registration time followed by registration ID determines the first candidate. The first available profile pins one global tuning session, including its AC and restore-on-exit policies. Later participants link to that tuning session instead of applying another scheme. The original scheme is restored after the last eligible participant leaves, subject to the pinned restoration policy.

AC loss restores an AC-only global session even when restore-on-exit is disabled. Unknown power source is treated as ineligible for AC-only activation. Returning to AC does not silently rearm that same cycle. A manual restore, outside scheme change, disabled cycle or failed activation also suppresses further automatic applications until the cycle's observed games have exited or been removed. Switching opt-in off and back on while the same game remains running does not bypass suppression. Manual recovery before the first watcher observation is applied to that next observation as well.

With restore-on-exit disabled, the selected scheme and unfinished tuning session remain visible for manual restoration. Closing the app restores only its current automatic session when that session's policy permits restoration. An external scheme change is never forced back by automatic recovery or shutdown.

## Persistence and ownership

Gaming starts, ends and startup interruption records use SQLite transactions. A pending tuning record and all initial gaming-session links commit together before any Windows scheme mutation. The transaction rechecks that linked games remain registered, identified and opted in. Storage failure or stale/disarmed registrations roll back the whole pending transaction.

The existing power controller verifies application and conservative restoration. The manual AC watchdog excludes game-owned sessions so that two controllers do not compete. Prior-runtime unfinished tuning records require review and never grant current automatic ownership. Startup marks prior open gaming sessions interrupted without changing Windows power settings.

Stable polling avoids redundant linkage writes. Active linkage and cycle tracking discard ended observations and stay bounded by the registered-game limit. Historical gaming summaries remain in SQLite for later history and deletion controls.

## Verification scope

Controller tests use an isolated SQLite database and a simulated power backend. That backend verifies the database is unlocked during each OS operation and that intent and game linkage are already durable. Tests cover overlap, multiple processes, deterministic profile selection, missing mappings, opt-in defaults, uncertain discovery, AC eligibility and disconnect, manual cancellation before/after startup, opt-in toggling, outside changes, transaction/link failures, denied mutation, crash recovery, retained profiles, shutdown and bounded tracking during continuous overlap.

The existing Windows process tests separately prove matching by canonical path, file identity, creation time and held process handles. Passing both sets does not yet prove their runtime integration or a physical AC transition.

## Work required before enabling the feature

- Connect independent bounded process discovery and the global controller to app lifecycle. Monitoring pause, minimizing or leaving the Gaming page must not stop AC protection or game exit handling.
- Serialize game-setting writes and explicit manual restores with controller decisions. Keep executable I/O outside the controller operation lock so it cannot delay a manual restore. Revalidate current registration settings, snapshot freshness and held process liveness before activation.
- Keep AC protection and outside-change handling responsive when discovery is slow or unavailable. Avoid treating stale snapshots as game starts or exits.
- Add typed opt-in/status/session-history IPC, explicit enable confirmation, AC/restore settings, status explanations, recovery navigation and translations in all three languages.
- Connect graceful shutdown to conservative automatic restoration and retain retryable recovery when storage or Windows calls fail.
- Verify the complete native pipeline with actual mapped Windows schemes and picker-selected fixtures, overlapping game processes, manual cancellation, minimization and app close/restart. Always restore the machine's original scheme and preserve user preferences. Physical AC transitions and broader hardware coverage remain separate release evidence.
