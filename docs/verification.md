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

## Keyboard latency and permission waiting

`scripts/smoke-scroll-latency.py` samples actual WindowServer transforms every
2ms while alternating focus between the first and last of four owned Alacritty
windows. It waits for initial AX sizing to finish before measuring. On the
single built-in display, six baseline motions with the installed release and
0.25s ease-in-out had median completion 251.03ms and 74.48 transform updates/s.
The updated debug build with 0.10s ease-out completed in 108.92ms with median
99.84 updates/s. First movement was 46.77ms versus 35.08ms. These measure
command-process launch through compositor updates, not display refresh or
physical key delivery; they do not establish sustained 120fps.

Read-only CG/SkyLight inventory is now requested on one worker, with at most one
pending snapshot; AX and layout/compositor changes remain on the main thread.
Snapshots begun before an AX write are rejected, and notifications received
during a pending query coalesce into one subsequent query.
An earlier attempt to defer inventory throughout animation failed the creation
latency budget (0.653s) and was replaced by the worker. The final event test
passed creation enrollment 0.300s, manual width adoption 0.172s, menu-bar padding,
520 nonempty resize clip samples, resize-refusal recovery and cleanup. The
interaction regression passed border dragging 800 to 910 points, ten successive
title-bar translations and 497 nonempty clip samples across three resizes.
All 62 Rust tests, formatting, Clippy, native build and transformed/clipped/hidden
input demo passed.

Permission waiting pumps the native run loop with a deadline timer even before
there are observer sources. A retained, windowless accessory child permits an
actual AXWindows read when the trust preflight is stale. An owned-host probe
passed from a trusted CLI and was denied from an untrusted temporary LaunchAgent.
System-wide attribute-name enumeration was found to succeed without permission
and is deliberately not used as authorization. The service resumes directly in
the same process after a successful gate; a real grant during this new wait has
not yet been verified. Input tap creation is retried without trusting a cached
preflight false. Permission database edits and automatic service relaunch after
a grant are not used.

## Known limits

The optional gesture implementation has 61 passing Rust tests, including touch
identity reordering, staggered finger lift, exact optional modifier matching,
horizontal/vertical intent, duplicate-stream suppression, native momentum,
monitor retention and cancellation on Space/viewport/settings changes. An owned
AppKit window and HID/session event taps verified four synthetic pixel-scroll
packets across the native transport, AppKit phase conversion (begin/end and
momentum begin/end), event consumption/passthrough and the timestamp clock.
These are synthetic scroll packets, not physical three/four-finger gestures.
A first background probe was not tied to an explicit human gesture and is not
treated as physical-input evidence. A visible probe starts only when the user
clicks its Start button and always passes input to macOS. The 30-second
three-finger session recorded 926 contact packets, 661 horizontal classifications
and no scroll/momentum packets. The two-finger session recorded 4864 packets,
including 39 native momentum beginnings and endings; 32 horizontal tails were
classified for routing. Four-finger input remains unverified. These observations
apply to this hardware/configuration, not every macOS installation. Receipt time
is used for capture expiry because incoming touch and scroll timestamps did not
share an epoch. A five-window Alacritty session then enabled only two-finger
gestures for an isolated real-Dock manual check; the user confirmed movement
and reported concern about conflicts with app scrolling. The recommended binding
is now Option plus two fingers. Gesture settings remain disabled
by default. Physical Spaces coexistence while capturing three/four fingers,
touch-to-wheel momentum handoff speed and multi-display gesture scrolling need
separate checks.

The owned-app event regression now passes with the loaded interactive-clip
backend: creation enrollment 0.238s, manual width adoption 0.064s, rendered
top/menu-bar padding, resize-refusal isolation, subsequent creation and cleanup.
Destroyed AX window identities are recorded at registration and sent to Rust,
and successful AX window-list queries reconcile withdrawn retained surfaces
once per second. Query failure does not remove windows. This check
ran on one built-in display after the external displays were disconnected.
The native demo's transformed, clipped and hidden input tests passed after
administrator authentication completed. The earlier run while authentication
was pending failed; it is retained as unsuccessful evidence.

The resize update removes the deliberate empty clip during AX geometry writes.
Rust retains committed placements and refreshes the compositor synchronously
from bounded native geometry-progress callbacks, including owner acceptance and
settling. An owned AppKit resize retained nonempty clips in 174 samples. An
isolated Alacritty test passed a real border drag (800 to 910 points), ten
successive owner-driven title-bar translations, and three commanded resizes
with nonempty drawing/input clips in 168 samples. Both tests release all leases.
These sample-based checks do not prove that every app or every display frame is
free from flicker; current hardware had one built-in display.
The five-window synthetic HID round trip passed with Option at acquisition and
no modifier on the end/momentum packets: position -24 to 336 and back to -24.
This confirms native phase routing through the real event tap and Rust layout,
not physical-finger smoothness. An initial attempt overlapped the startup
layout animation; the test now waits for that animation to settle first.

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
