# Verification

Development and release builds use the flake-pinned Rust 1.91.1 and Clang 21. Run:

```sh
nix develop -c sh scripts/check.sh
nix build
nix develop -c cargo run --locked -- demo --test
```

The automated suite covers layout, native Space/monitor context isolation, focus,
move/swap/stack operations, clipping rectangles, spring bounds, configuration,
float/sticky state, IPC framing and the supported yabai syntax adapter. It also
checks that full-width toggles restore per-column widths, width presets respect
padding, and mirroring preserves window identity, focus and row weights.

## Runtime coverage

Tests on macOS 26.6.2 / Apple Silicon have exercised the user's loaded Dock
backend against disposable app windows: translation and clipping, transformed
mouse input, fully clipped input rejection, watchdog restoration, scrolling in
both directions, adjacent displays with independent native Spaces, window
creation/destruction, click focus, manual resizing, stacked columns, float and
sticky. Clearing sticky on a different native Space leaves the window on that
Space; restoring the sticky bit does not restore the old Space membership.

A temporary LaunchAgent running the packaged Rust binary directly verified its
own Accessibility permission, startup, graceful quit and successful-exit restart
suppression beyond the launchd throttle interval. That startup test excluded
existing user apps; it does not prove arbitrary-app stability. Shell-wrapper
startup had different TCC attribution, so the module starts Rust directly.

Local logs and screenshots are deliberately kept outside the published Git tree.
They may contain desktop app titles. Build success alone is not runtime proof.

The socket regression executes the payload's actual handoff function without
Dock injection: missing/stale sockets, idle handoff, active controller retention
and non-socket preservation. Packaged loader tests cover identity mismatches and
delayed readiness. A disposable AppKit window verifies that a compositor
translation leaves AX geometry independent, and resize/restoration use the saved
AX frame. Native display conversion checks menu-bar reservations with positive
and negative monitor origins. These checks do not prove arbitrary-app restoration
or root-watcher recovery in the installed service.

The responsiveness update has 52 passing Rust tests, a flake release build,
native demo input checks and an owned-window payload-handler test. The latter
verifies that interactive frames preserve ten owner-driven translations, keep
the actual clip within a retained viewport, and restore placement on release.
It does not inject into Dock or simulate an AppKit mouse drag. `smoke-events.py`
checks real AX notifications, creation/resize latency and resize-refusal isolation;
`smoke-interaction.py` additionally samples an actual title-bar drag. These tests
must pass against the new loaded backend before claiming desktop drag stability.

## Known limits

- App minimum/fixed-size negotiation is incomplete. An app refusing an AX resize
  is restored best-effort and left floating. Restoration can still fail if an app's constraints change.
- SIGKILL/watchdog recovery restores compositor state and sticky tags, not AX sizes.
- Root-watcher recovery after a Dock restart is implemented but not runtime-tested.
- The latest column-width/mirror actions have automated layout coverage; direct
  skhd key delivery and fullscreen transitions need separate runtime verification.
- Global mouse-modifier dragging, cross-monitor window transfer, automatic app
  rules for sticky/above, BSP parent zoom/rotation, rule/signal API compatibility,
  automatic display-topology recovery and state persistence remain incomplete.
- Intel Dock injection and other macOS versions are not verified.
