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

The Space-retention regression uses four selected, owned Alacritty windows and
one unmanaged owned app. The previous installed release reproducibly changed a
manual scroll position from 1130 to -24 after native activation returned from
the unmanaged app. With that assertion skipped to isolate Space behavior, its
cached source Space lost all four columns while on the adjacent desktop. The
updated daemon passed three real native Space round trips with reordered
columns, preserved widths and manual offsets, plus unmanaged-app return and a
real click that still reveals its selected column. Testing is on one built-in
display and desktop Spaces; this is not full arbitrary-app or fullscreen proof.

Disappearance from CG inventory is no longer sufficient to discard a window
still owned in WindowServer. AX withdrawal records are distinguished from
destruction and ignored for inactive contexts, retaining the observer identity
until Rust actually forgets the window. Current native contexts are adopted
before closure/focus processing, and asynchronous snapshots from the old context
are rejected. Resize adoption uses untransformed SkyLight surface bounds instead
of presentation bounds. Automatically observed focus preserves viewport and
manual-centering bounds; recent clicks inside the selected clip and explicit WM
commands can reveal it. App-internal keyboard focus changes update selection
without automatic viewport reveal. The two new policy regressions bring the
Rust suite to 64 tests.

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

## Desktop recovery and stability changes, 2026-10-06

The deployed service crashed in the native inventory worker with an uncaught
`NSInvalidArgumentException`: JSON serialization received an infinite rectangle.
Restarting then adopted clipped presentation dimensions. Stopping the service
alone left narrow clips and mismatched logical/presentation origins behind.
During recovery the service and backend watcher were stopped. Four affected terminal windows and
the OrbStack main window were recovered through their exact owner identities;
the user confirmed normal dragging again. OrbStack metadata sampling after
recovery showed zero position disagreement in 40 samples. That static sampling
does not establish drag behavior under the updated WM.

The source changes reject null/infinite native rectangles, defer failed
inventory reads, and use physical surface dimensions for enrollment. Floating
and normal exit settle usable current coordinates and explicitly commit full
clips before releasing compositor snapshots. Mouse capture targets the actual
mouse-down window and retries delayed application activation. Mission Control
pauses geometry adoption and exposes full surfaces while retaining layout state.

An owner drag exposed a second failure: the initial visual x301 / physical x100
separation was applied twice, so the first 11-point drag showed x513 instead of
x312. A monotonic-position check had incorrectly passed this behavior. The
regression now samples mouse-down and every step, checks both coordinates against
the pointer displacement, and rejects that initial jump. Rust reads physical
and presented geometry directly from WindowServer during capture, with no AX
reads or writes in that drag path. The captured visual grab point drives the
compositor correction; resizing retains the opposite visual edges. The monitor
clip remains active and the top cannot cross the monitor's menu-bar boundary.

The final owned Alacritty diagonal drag advances from (301,57) through (312,63)
to (411,117), matching each 11/6-point pointer step within two points. Mouse-down
samples retain the initial position. A border drag is adopted as width800 to910;
three commanded resizes retain nonempty drawing/input clips in496 samples.
Native payload lifecycle tests pass pointer correction on both axes, overview,
owner validation and committed release. The finite-rectangle native test and
67 Rust tests, formatting, Clippy and Nix package build pass. The native demo's
transformed, clipped and empty-clip input tests also pass.

Actual Mission Control exposes all four complete owned surfaces, including
hidden columns, and exit restores column widths/order and manual scroll. SIGTERM
commits full clips and usable on-display coordinates, with no origin reset.
Ten floating/detach/retile cycles preserve the released visible position,
physical size and full clip; manual floating resize is retained on retiling.
The first automated Mission Control exit attempted a synthetic Escape that did
not exit; the test now toggles the native Mission Control application and cleans
it up on failure. It is not counted as a successful run.

Current runtime hardware is one built-in display. These owned-fixture checks do
not establish every application's behavior or updated multi-display behavior.
Abrupt termination still falls back to compositor snapshots and does not
reliably recover owner geometry; it is not covered by the normal-exit guarantee.

## Native drag feedback and attached surfaces

Live OrbStack diagnosis found a first drag transaction moving presentation by
372 points while the pointer advanced roughly 0.1 point. Subsequent samples
showed alternating owner translations and absolute WM pointer corrections.
Metadata also confirmed a separate WindowServer child surface left at its
physical position while its parent was compositor-translated, matching a
misplaced macOS capture indicator. These observations invalidate a general
desktop-stability claim based only on earlier Alacritty drag tests.

The update removes repeated pointer corrections, anchors fully visible windows
at rest, avoids new AX geometry writes during mouse interaction, and moves
explicit same-owner child surfaces with their parent. A Rust regression covers
one correction followed by owner control. The native payload-handler test uses
a real AppKit parent and attached child: normal translation, clipped interactive
movement, full overview, finish, and owner movement before overview all pass.
Native overview uses current physical geometry instead of an obsolete absolute
translation. This is an owned-surface test, not Dock injection or proof of
arbitrary-application drag stability. The new live backend still needs desktop
verification; the recorded broken service was kept running throughout diagnosis.

### Attached-surface withdrawal follow-up

The installed build encountered a failed removed-surface restoration followed by a
stuck controller lease during actual desktop capture. This is not a passing
desktop verification. A regression now injects a null WindowServer rectangle for
an owned attached surface and checks that release uses its last finite native
bounds and clears the lease. Ordinary attached-window withdrawal also passes.
The payload provides a read-only `diagnostics` operation with per-surface owner
and bounds availability so a further failure can be identified rather than
inferred. Live drag and Mission Control smoothness remain unverified.

### Native-drag ownership and rejected startup

Compared yabai commit `dd845723416f5fe92af49fad5ebab00369e07edd`, specifically
`WINDOW_MOVED`, `WINDOW_RESIZED`, `MOUSE_DOWN/UP/DRAGGED` in
`src/event_loop.c`. Its ordinary native drag skips layout flushing for the
held window; explicit modifier-driven moves use a separate path. RibbonWM's
interactive parent already yielded after its initial offset correction, but its
derived child updates still wrote absolute transforms on every frame. Those
children now yield too. The native test moves an owned parent/child pair through
a clipped viewport and counts zero payload transform writes during the native
drag, while preserving the bounded clip. The one-shot correction remains tested.

A separate actual-desktop failure revealed repeated startup attempts performing
AX cleanup even though the Dock lease belonged to the previous controller.
Readiness now waits for an empty lease, the daemon checks ownership before
discovering windows, and release checks ownership again and only considers
committed/AX-touched windows. A Rust regression covers rejected initial placement
and a later partial geometry write; native tests check foreign-controller
preflight/release rejection with zero compositor writes. These checks do not
establish physical drag smoothness on the user's desktop.

The release of a mouse hold also invalidates older inventory, and a native
position rebase was allowed only while observed size matched the planned size.
A regression checks that an unobserved border resize cannot be overwritten by
a position-only anchoring attempt.

### Geometry and discovery pipeline separation

The subsequent live report exposed a remaining design problem: idle position
rebasing erased accepted size state and routed focus/scroll completion through
the AX resize/settling path. That automatic rebase is removed. `NativeSizes`
tracks accepted dimensions independently of compositor origin; moving focus,
scrolling, or releasing a move cannot invalidate that ledger. The native resize
helper no longer forces acceptance by shrinking a window 40 points and growing
it back. A layout/explicit size change still uses synchronous native sizing;
this change does not claim that every AX operation is nonblocking.

Read-only AX eligibility/initial geometry and retained-window membership now run
on the inventory worker using independent AX references. They never touch its
main-thread observer/cache tables. Exclusions are applied before probing; failed
or incomplete queries are unknown membership, not evidence of closure. Main
thread destruction notifications remain immediate, and focus notifications also
schedule reconciliation. Queries made before geometry writes or a completed
mouse interaction are discarded. Probes for separate applications are currently
serial on the worker, so a slow application can still delay another discovery,
but cannot hold the rendering loop while this discovery query waits.

`scripts/smoke-live-pipeline.py` measures five create/remove cycles against the
already running service, then samples native dimensions during focus and scroll.
It never replaces or stops the service. The installed `ff30cf9` baseline measured
437–637ms creation and 17–37ms removal in the complete run, with constant
400×1362 native dimensions in 302 samples. Thus that fixture reproduces slow
creation, but does not reproduce the reported focus-size jitter. Updated runtime
results must be recorded separately; passing unit tests cannot establish physical
drag stability.

### Selective discovery and polled settlement

The installed `730acf5` baseline measured create/enrollment times of
267.0–320.7 ms (five cycles; median 306.7 ms), removals of 15.7–51.7 ms,
and unchanged 400×1362 native dimensions over 370 focus/scroll samples with
zero new native resize requests. These are fixture measurements, not a claim
that arbitrary application drag or Mission Control behavior is stable.

A five-second service sample identified per-surface Space, sticky and bounds
queries in the inventory worker. A read-only native comparison on this desktop
measured full metadata for 550 surfaces at 26.36–59.85 ms; the cheap summary plus
seven selected detailed rows took approximately 9–10 ms. This is only metadata
query timing, not end-to-end creation latency.

The candidate adds selected-window queries, targeted event IDs, four independent
AX probe workers, partial application results, unknown incomplete membership,
and generation rejection. Size settlement now returns a main-thread handle and
is polled across frames; changed columns publish all rows together. Regression
coverage includes a blocked app alongside a ready app, stale partial/final
inventory, valid candidates from incomplete membership, stacked-row publication,
and settlement restart on transform/size changes or invalid samples.

The Mac was locked during subsequent enrollment attempts. CUA explicitly
reported the lock, and an isolated read-only AX probe returned an AXApplication
placeholder from AXWindows. Those failed attempts are **not** valid unlocked
creation timing runs. No production service was stopped/reset for this
investigation. At that point GUI verification was pending manual unlock and the candidate had
not replaced the service. Subsequent deployment and measurements follow below.

`smoke-live-pipeline.py` now checks session state before creating owned windows
and on every wait. Twenty create/remove cycles measure `presented_windows` when
available, rather than reporting early enrollment as completed layout. Old
release measurements are labelled `enrolled`. The script does not stop or
replace the running WM. Live acceptance still requires that run, owned-window
physical drag/resize, floating transitions, native Space/Mission Control return,
and multi-monitor clipping checks. The synchronous initial AX acceptance and
next-batch probe wait remain explicit limitations.

### Native activation and IPC follow-up

The `4bab26b` package was activated and the service resumed in the same process
after permissions became ready. Live fixture attempts exposed intermittent
`Invalid argument (os error 22)` from the CLI: on this macOS, setting the receive
timeout after the peer closes can fail even though the reply remains buffered.
The IPC reader now uses `poll` with the original absolute deadline. A Unix socket
pair regression writes a complete frame, closes the peer, and then reads it;
fragmented, oversized and idle-connection tests remain in place. An initial
rounding hypothesis was disproved and that proposed fix was discarded.

Native-focus regression coverage distinguishes repeated observations, return
from an unmanaged app, automatic selection in another Space (including a delayed
old-Space observation), and Mission Control return. Only the last of those
forces reveal of an unchanged selection. The normal module defaults no longer
exclude assistant apps; the user's normal service configuration also removes
those exclusions. Diagnostic fixture guards still exclude protected apps.

The supplied Mission Control recording shows a transient pile of native windows
before tiled presentation returns. Code review found a 200 ms reconciliation
hold sending only heartbeats after overview had removed transforms. The candidate
restores committed compositor frames during that hold, while still deferring AX
writes and inventory reconciliation. This removes that explicit display gap;
physical Mission Control exit animation remains a required live check.


### Deployed settlement: owned width and move measurements

`4bab26b` was activated on 2026-10-07. The existing production daemon was kept
running throughout the owned-fixture measurements. Three creation cycles reached
committed presentation in 208.1, 249.3 and 230.8 ms, with removals in 21.0, 33.8
and 31.9 ms. The initial width run then failed in the fixture's diagnostic code:
a missing onscreen metadata value was inserted into an Objective-C dictionary.
That run is not a width pass. The helper now treats absent onscreen metadata as
false; it does not change production code.

A complete retry reached presentation in 310.8 ms and removal in 25.3 ms.
Six requested widths (620, 940, 740, 1020, 520, 800 points) were independently
confirmed through the owned surface's physical dimensions. Completion took
132.8–183.0 ms. Height matched the planned 901 points and presentation stayed
below the usable viewport top. Four next/previous moves issued zero native size
requests. During subsequent focus/scroll, all 343 physical samples remained
800×901 and the native size request count did not increase. Fixtures were
removed and the original daemon remained running.

The new CLI also completed 100 consecutive status requests against that service
without the intermittent IPC error. These tests establish accepted dimensions
and absence of redundant resizing for this fixture, not physical drag smoothness,
Mission Control exit visuals, or arbitrary application behavior. The activation
and overview changes described above still need their own deployed visual check.


The activation/overview release `54be178` was subsequently installed through
nix-darwin. The new service entered live mode after its permission gate without
relaunching; the loaded Dock payload matched the packaged identity. The actual
LaunchAgent arguments no longer excluded ChatGPT. Status reported one presented
window. The width/move measurements above apply to `4bab26b`, not this later
release. Physical Mission Control exit and Dock selection behavior have not yet
been verified against the installed follow-up.


### Mission Control transition correction (2026-10-07)

The next two user recordings disproved visual acceptance of the preceding
follow-up: entry/exit jumped, and selecting a different application first showed
the old tiled strip before revealing the selection. A read-only Dock observer
recorded five native overview enter/exit cycles. Each had one active/inactive
transition; no repeated false exit was observed in this sample. This does not
prove event ordering on every system.

Two concrete write-order defects were corrected. Overview entry previously
replaced every retained surface transform with its physical anchor, even though
Dock could already be animating it. Entry now only exposes the full clip and
retains the lease, leaving both root and child transforms untouched. Exit no
longer replays the previous committed strip before a 200 ms total pause. It
refreshes native context and focus on the first resumed frame, completes only
the selected context's scroll immediately, then publishes that layout. The
200 ms hold applies to AX size writes and inventory adoption, not selection.
No second horizontal reveal animation follows Dock's animation.

75 Rust tests and Clippy pass. The native owned-surface regression installs an
in-flight scale/translation before calling overview, verifies zero transform
writes for the entire family, full 400×400 clipping, and exact preservation of
the transform. Native lifecycle/null-bounds and query/settlement tests also pass.
Core tests cover clearing stale easing without settling another monitor or
inactive Space; native-focus tests require the newly chosen offscreen window to
be fully visible in the first placement without an intervening animation tick.
These checks do not yet establish actual Dock animation smoothness. Deployment
and physical Mission Control acceptance must be recorded separately.


### Geometry handoff investigation (2026-10-07)

The overview correction `0c9c872` was activated and its matching service/payload
verified. Subsequent recordings still showed serious drag and width-transition
failures; that deployment is not a general visual acceptance pass.

A manual owned-window trace reproduced a stationary-pointer grab jumping from
(1088,57) to (1618,-168), then being corrected on a later WM frame. Its physical
origin before the grab was (558,282). The jump was exactly the stale physical /
presentation offset being applied again by the owner. Correcting after that
first move cannot prevent the already displayed bad frame.

An owned-window-only experiment aligned native and displayed origins while
idle, without resizing. In five subsequent titlebar press intervals, the maximum
displacement while the pointer remained within two points of its press was
0–1 point, versus up to about 1005 points in the baseline sample. The experiment
used the fixture's own connection, not a newly deployed Dock payload. It does
not establish clipped-window behavior, arbitrary application compatibility,
release-animation smoothness, or Mission Control quality. AppKit's cached frame
can remain stale after a native group move, so matching WindowServer coordinates
alone was not treated as sufficient evidence.

The candidate adds settled, fully visible native group anchoring, batches native
presentation writes, and adopts the final owner size at mouse release before
issuing layout writes. Neighbour spacing now follows held presentation widths
rather than future requested widths, and accepted size changes no longer wait
for the temporal stability timer before presentation. 77 Rust tests, Clippy,
and the native owned-surface tests pass. Regressions include unchanged dimensions
and child offsets during anchoring, refusing anchors for clipped/interactive
windows, balancing display-update suspension after an injected failure, and no
AX resize request when accepting a completed user width change. Candidate
production deployment and intermediate-frame width validation remain pending.


### Explicit writer policy and surface acceptance

The follow-up replaces future-width presentation with observed surface sizes,
including stacked-row spacing, and freezes unrelated placements during a native
drag. The writer policy covers all combinations of inactive session, overview,
mouse press and overview reconciliation. 78 Rust tests, Clippy and native tests
pass. An independent floating fixture (only its own ID was exempted from the
running WM) passed three native anchors followed by calibrated AX resizes at
(520,250), (220,170), and (650,190), retaining the requested origins and widths.
The initial fixture trials were invalid because the running service enrolled
it; those are not counted as independent geometry passes.

A separate-process sampler now records both owned adjacent surfaces during six
live width changes. The old deployed version produced substantial intermediate
gap/overlap even though final dimensions settled. The sampler avoids work on
the fixture's AppKit event loop, which would perturb the AX responsiveness being
measured. Candidate comparison and deployment results remain to be recorded.
Only the built-in display is currently connected; multi-monitor revalidation is
not covered by this run.


During deployment the new payload image was mapped into Dock but did not open
its listener. The same arm64e APIs passed the owned test. Startup now checks the
host's main bundle (rather than relying on workspace registration being ready)
and retries safe socket handover outside the constructor's loading thread.
Loading this candidate into the existing Dock process started a zero-lease
listener without restarting Dock. Native active-controller/non-socket handover
regressions still pass. The exact original early-return branch was not recorded,
so no narrower root-cause claim is made.


The writer-policy candidate was deployed and the service resumed in the same
process after its permission gate. Six live width changes reduced the duration
of intermediate gap errors, but still recorded 1–5 mismatching samples per step
and errors up to 482 points. These are independently sampled geometry values,
not display-synchronized video frames; this is not a smoothness pass.

A follow-up prepares the physical resize origin while preserving presentation.
The owned native test verifies unchanged surface size, attached-child offsets,
unchanged presentation after AppKit adopts the prepared origin, and rejection of
an origin outside the retained viewport. Live comparison is still pending.


The resize-origin preparation was deployed. A six-width live run recorded one
bad sample per width. A follow-up sampler reads both surfaces twice in opposite
order: some gaps persisted across both reads, so they cannot all be dismissed
as mixed reads. Those transitions still require fixing; this is not a visual
smoothness pass. Six titlebar press intervals on the production-controlled
fixture recorded 0–1 point displacement while the pointer stayed within two
points of its press, with no fixture-side anchor correction. The user reported
that this drag test felt better. Unrelated clicks were excluded.

The next change keeps presentation callbacks running while an AX request awaits
the owner application. Tests cover caller-thread callback execution during a
blocked request, cancellation joining the worker, and propagation of AX errors.
An owned floating fixture passed three calibrated native-anchor/AX-resize cycles
through this worker path, with five to nine progress callbacks per operation.
Rust tests and Clippy pass. Live width comparison remains pending for this change.
