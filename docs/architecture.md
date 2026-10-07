# Architecture / v0.1

## State and spaces

`Engine -> Monitor UUID -> NativeSpaceId -> SpaceLayout -> Column -> WindowId`

A monitor's current native Space selects one horizontal layout. Existing macOS Spaces are the workspaces; there is no additional virtual workspace layer. Columns retain their width as windows are added and may contain vertical stacks. Scroll is bounded by strip width and viewport, including oversized columns. Positions use global logical points in WindowServer coordinates, including negative coordinates on secondary displays.

Each placement contains an absolute frame and its intersection with the owning monitor's usable viewport. Windows wholly outside the viewport use an empty clip. The compositor receives actual window IDs, not captured textures. Ordinary tiling does not change native Space membership. Sticky toggling follows WindowServer's native semantics, described below. Physical display isolation with separate macOS Spaces is also observed at runtime; this must not be mistaken for evidence that the explicit clip alone caused isolation.

All retained native Space contexts remain in compositor frames. Switching native Space changes the selected layout without restoring or resizing windows in the old Space; macOS controls visibility. Off-Space animation is frozen and resumes on return. Existing sizes and compositor snapshots remain leased until normal exit, window removal or watchdog expiry. Contexts live in memory only.

`Scroll` supports an analytical critically damped spring and timed quadratic easing. The service uses PaperWM's 0.25-second ease-in-out curve and configurable usable-width ratios (0.38195, 0.5, 0.61804). Retargeting starts from the displayed position. Native size acceptance and compositor placement have separate state. Resize settlement is polled on subsequent frames; every row of an affected column retains its committed presentation until the column is ready. Independent columns can animate while that settlement is pending. Ordinary focus and scrolling do not request native size changes. The initial AX write/acceptance and explicit detach/restore paths remain synchronous; this is not yet a fully nonblocking geometry pipeline or a display-synchronized native transaction.

The live preset uses PaperWM-like insertion to the right of native focus, natural initial widths, 20-point horizontal/vertical margins, centering of compact strips, and minimal scrolling to reveal focus. Per-edge padding overrides the symmetric margins; asymmetric padding also changes the usable center. Near-full-width columns center. Signed offsets permit a single column to sit in the middle. Changes to the strip bounds clamp the target while allowing the displayed offset to animate; removing a column to the left adjusts both offsets by its width plus gap, retaining the surviving selection's visual position. Recently selected adjacent windows guide selection after closing the selected window. Stacks start at equal heights; explicit or observed row resizing stores weights and allocates the remaining height to other rows, with a 100-point minimum when setting a row. Balance removes those overrides. This is a smaller allocator than PaperWM's.

Native focus observations are idempotent for scrolling but still update global monitor history when returning to a monitor's already-selected window. The core owns first/last/previous/next/MRU and stack selectors, movement, swaps, column widths and row weights. `compat.rs` parses a documented subset of yabai message syntax into the same validated IPC actions; it never executes shell commands or forwards commands to yabai. Window operations in compatibility mode use actual native focus rather than pointer location. Unsupported syntax is rejected before mutation. Runtime spacing changes validate a complete settings clone before replacing settings; they do not overwrite the TOML file.

The daemon tracks floating and sticky windows separately from the tiling engine. Either flag removes a window from its column, settles its current visible frame within the monitor padding, and releases its compositor lease. Explicit floating remains independent of sticky; clearing one flag only rejoins tiling when neither flag remains. Rejoining adopts the current width and uses the current free geometry as the new restoration baseline. Detached windows retain owner metadata and can be focused by ID. Relative layout selectors still operate on tiled columns. Mode changes currently require a visible tracked window and are session-local.

## Native boundary

The ordinary process queries WindowServer and uses AX for size and focus. Rust owns layout and lifecycle. AppKit only creates demo surfaces, forwards events and applies Rust placements. SkyLight symbols are dynamically resolved. The tested clip ABI is `SLSSetWindowClipShape(connection, window, region)` with three arguments.

The optional payload runs inside Dock. It exposes status, frame, sticky and reset requests on `/tmp/ribbonwm-UID/backend.sock`, inside an owner-checked real directory with mode 0700; the socket is mode 0600 and checks peer UID. Requests are bounded to 64 KiB and 128 unique layout roots (512 surfaces including attached children), use a controller session, and have read/write deadlines. It retains original transforms/clips and window owner PIDs. Scale, rotation and shear are rejected. Native overview and fallback release normalize translation to the current physical surface position rather than replaying an obsolete absolute snapshot after an owner move. Protocol 2 skips closed/reused IDs; Rust supplies expected PIDs on every update. On a genuine apply error it retains snapshots so Rust can restore AX geometry before compositor reset, instead of prematurely restoring the compositor. A watchdog releases compositor transforms at current native positions after approximately two seconds without a frame, restores clips (using full current geometry after a size change), and retries failed releases. Closed or reused IDs with a different owner PID are not restored onto the new owner.

Protocol 2 advertises an optional `sticky` capability. Sticky uses WindowServer tag bit 11 through `SLSSetWindowTags` / `SLSClearWindowTags`; reads use the window-query iterator. The payload saves the original bit and PID before writing, verifies each write, and restores only that bit. Frame messages retain these sticky leases even after the desired flag becomes false, preserving the original tag until controller release. `controlled` counts the union of compositor and sticky snapshots. Reset and watchdog restore both kinds. A capability-less older backend can still float, but refuses sticky before changing the layout. Sticky retains normal window level; macOS chooses stacking/focus on Space transitions. On the tested OS, clearing sticky on another Space leaves the window visible and assigns it to that current Space. Retiling follows the resulting native membership. Restoring the tag does not restore earlier native membership.

The daemon uses a separate same-user socket, `/tmp/ribbonwm-UID/wm.sock`. Status queries do not take the backend controller lease. Live frames heartbeat every 50 ms even at rest. On an ordinary exit the daemon settles current visible geometry within each monitor, then finishes compositor leases with full clips at those frames. **A killed process can leave AX sizes changed:** the Dock watchdog only knows the compositor snapshot. Recovery journaling is required before crash restoration can be considered complete.

The live daemon accepts explicitly selected IDs, or opt-in `--all`. It excludes minimized and nonzero-layer windows, and helper surfaces narrower or shorter than 100 points at discovery. Preexisting sticky standard windows are tracked outside the tiling engine. `--exclude-app` matches bundle IDs or app names, with an optional trailing prefix wildcard, before any AX query. New live candidates must be AX standard windows with settable position and size; their presentation and logical sizes must agree, with the same translation-only constraint used by the payload. This postpones enrollment during opening animations and skips scaled/rotated/sheared windows without stopping the daemon. Origins may differ, as observed in Chrome. Per-app AX observers coalesce creation, destruction, focus, movement, resizing and minimization notifications. Rust reconciles inventory on those flags, with a 100-ms fallback; failed observer registrations retry every second. App metadata is filtered before registering any AX observer. Native focus is observed on notification with a 100-ms fallback, only for already-managed, non-excluded app PIDs. NSWorkspace notifications are serviced through a bounded main run-loop iteration because Rust owns the loop. Repeated observations are idempotent, and observations during Space changes update selection without revealing it, preserving saved manual scroll. Fullscreen suspends active resizing and focus in its monitor. A change in viewport, scale or display topology exits live mode and attempts restoration. Mission Control temporarily releases clipping through the backend overview operation and pauses layout work. Automatic resume after display topology changes is not implemented. AX accepted sizes are validated; minimum-size negotiation remains unimplemented.

AX element handles are cached after resolving window IDs and guarded by the current owner PID, so cleanup can access windows in inactive native Spaces. Resizing converts the requested outer WindowServer rectangle to AX geometry using the measured per-window inset. Original AX geometry is saved independently from the presented compositor snapshot. Current AX values suppress redundant writes; the owner's AXEnhancedUserInterface flag is temporarily disabled and restored around resize work, following yabai's behavior. Each accepted-size poll is bounded to 0.25 seconds. A refused size is reported without forcing an intermediate smaller size. A changed owner inset can trigger one remeasurement/retry; a persistent refusal leaves the window floating. The final accepted size must match within two points; this does not accept an app's minimum-size constraint as success. Re-anchoring after size acceptance avoids overwriting a pending owner resize. Waiting or reordering alone did not fix the observed small full-height resize failure. Stable WindowServer size/transform is then checked for at most 0.5 seconds. AppKit may constrain the logical anchor, so ordinary resizing accepts a stable position inside the intended display; Dock independently applies the desired visual position. Restoration waits for the original size before reapplying and validating the exact original position. An owner that refuses the requested size is restored best-effort and left floating; other windows and the daemon continue. Minimum-size negotiation is not implemented. Normal exit handles SIGINT/SIGTERM and restores AX geometry before releasing compositor state. SIGKILL restoration remains incomplete.

App/user resizing is detected by comparing the native inventory against each window's last accepted managed size, not its startup geometry. A pending WM command takes precedence over an older native sample. Observed width changes update the whole column without activating the window; stacked height changes update row weights. A single row fills the usable height again. While the left mouse button is held, AX writes, scroll animation and monitor reassignment are postponed. Interactive payload frames maintain the lease and clip the owner's current surface to its retained monitor viewport, without setting its transform. Releasing the mouse resumes placement. This conservative pause applies to all managed windows, including ordinary mouse selections. Modifier-based reordering is not implemented. The initial geometry remains separately available in `status.original_geometry` for diagnostics and graceful restoration.

The usable display rectangle retains visibleFrame's side/bottom exclusions and reserves at least the primary display's measured menu-bar inset plus each display's safe top area. On the tested macOS setup a secondary visibleFrame includes the full display despite a visible menu bar, while NSStatusBar reports 22 points and the primary inset is 30. Reserving the measured inset avoids clipping a window titlebar under that secondary menu bar. User padding is applied after this OS exclusion.

Empty clips prevent mouse input; they do not remove an application's keyboard focus. There is no virtual workspace command. Native Space keyboard focus belongs to macOS. A separate synthetic-key test targeting a disposable Alacritty process has not passed, so successful AX focus calls are not treated as proof of keyboard input routing.

## ABI and source references

Runtime scope verified on macOS 26.6.2, arm64: Dock loading by the user; cross-process transforms/clips and mouse input; watchdog restoration; real Alacritty processes with native Space transitions and graceful restoration; a shadowed fixture at the Studio Display/VG270U physical seam with separate Spaces per display. See [verification](verification.md) for exact limits. Private API compatibility is not inferred from a successful build.

- [yabai Mach loader](https://github.com/asmvik/yabai/blob/dd845723416f5fe92af49fad5ebab00369e07edd/src/osax/loader.m): MIT-derived loader, retaining Jeremy Legendre's arm64e attribution. Local changes supply an explicit payload path and bound the load handshake.
- [CGSInternal](https://github.com/NUIKit/CGSInternal/tree/c4f6f559d624dc1cfc2bf24c8c19dbf653317fcf): historical private API declarations used as investigation material. Local symbol/ABI inspection and own-window experiments determine the clip ABI used here.
- [PaperWM tiling](https://github.com/paperwm/PaperWM/blob/8bf6dd264f60d6c0c402b63df7b424b888959a48/tiling.js): behavioral reference for insertion, width preservation, focus reveal and compact-strip centering. The Rust implementation is independently written; GPL source expressions are not copied.

No service is installed by building the project. The manual load script uses a content-addressed dylib path and performs one sudo invocation when a new build is needed. Socket handover requires an idle payload reporting the same Dock PID and UID, with unchanged socket inode; active controllers are not evicted. The script also refuses while a WM socket is listening. The legacy payload's idle server thread remains until Dock restarts. The native Makefile builds arm64e; Intel payload support has not been integrated or tested. Nix supplies Rust 1.91.1, Clang 21 and SDK/tool dependencies; the host supplies the running OS frameworks and `/usr/bin/codesign`.

## nix-darwin service

The flake exports `darwinModules.default` / `ribbonwm` and a release package containing the Rust service entry, loader and signed content-addressed payload. `services.ribbonwm` configures a generated TOML, login user and exclusions. The user LaunchAgent executes `bin/ribbonwm service …` directly: a shell wrapper was observed to have different TCC attribution from the trusted binary. Rust requests its own Accessibility permission once per startup if untrusted, waits for the grant and sticky-capable backend, then calls the same daemon lifecycle. The OS prompt is asynchronous and never grants permission itself. `request-permissions` explicitly requests it; `doctor` stays silent. Successful quit stays stopped; error exit restarts after throttling. SIGTERM reaches geometry cleanup; SIGKILL still cannot restore AX sizes.

Optional `enableDockInjection` installs a root LaunchDaemon that selects the configured user's exact Dock PID, drops credentials to that user for probes, and invokes only the fixed packaged loader/payload. It declines handover while a WM or snapshot lease is active. A backend failure causes the Rust daemon to exit and clean up, so the loader can reconnect after Dock restarts. Automatic Dock restart recovery remains untested on this host. Building does not install these services; activating the user's system configuration does.

## Native interaction and attached surfaces

Scrolling and focus update the compositor frame. Once scrolling and native size
settlement finish, fully visible roots align their physical origin with their
presented origin using a native group move, without an AX size write. This
prevents the stale physical/display offset from being added to the next native
drag. Partly clipped columns retain compositor scrolling; physical
placement never crosses a monitor merely to emulate scroll. Geometry writes stop when a
mouse press begins. A capture uses the last committed display position as its
grab anchor. If physical and displayed geometry already agree, AppKit owns the
entire drag; otherwise one initial offset correction removes the stale
translation, after which interactive frames update clips without overwriting
the owner's transform. Releasing a drag invalidates older inventory and reads the final native size
before normal layout resumes. This prevents the previous width from being
written back while asynchronous discovery catches up. Native anchoring is
disabled while the mouse is held, during overview reconciliation, on inactive
Spaces, and for partly clipped windows. A bounded payload display-update block
publishes group moves, transforms, and clips together, with no AX calls inside it.

The Dock payload resolves `SLSCopyAssociatedWindows` with query-iterator parent
IDs. Only descendants with the same owner PID join a root's lease. Attached
AppKit surfaces and macOS window capture indicators retain their native offset
from that root, share its monitor clipping, and participate in overview and
finish/detach. Their lifecycle is separate from layout columns. The
`window_groups` capability prevents a new controller from silently using a
backend that only transforms the root surface.

## Discovery and settlement scheduling

AX creation/move/resize callbacks retain their window IDs. A worker queries full
Space/sticky/surface metadata only for notified and retained windows; a cheap
summary scan provides fallback discovery at least every 250 ms. A four-worker
AX probe pool publishes each application's result independently within an
inventory batch. An incomplete membership query may still admit valid new
windows but cannot prove that retained siblings closed. Generation invalidation
rejects both partial and final results sampled before a geometry transaction.
The next inventory batch still waits for the current batch to finish; bounded
per-app AX calls do not yet provide a total batch deadline.

Native resizing issues the owner's size/anchor writes, then returns a settlement
handle. The main loop polls it once per frame without a sleep loop. A changed or
invalid sample resets the 50 ms stability interval; the overall settlement has a
500 ms deadline. Polls still contain AX reads with 10 ms per-call timeouts. Only
one native resize is in flight. Floating/sticky requests are queued while it is
pending, with the target resolved when the command arrives; layout/focus/status
commands remain available. Stacked rows publish their new layout together.
`status.presented_windows` distinguishes enrollment from completed native sizing
and a committed presentation. It is a compositor submission acknowledgement,
not a measurement of when the physical display scanned out the frame.

An inactive or locked console session suspends mutations, observation, and
reconciliation while maintaining the existing backend lease. On return the
inventory generation is invalidated and discovery resumes. Locked-session AX
placeholders are not interpreted as vanished windows. A pending acknowledged
resize is cancelled without automatically rebasing it after unlock or a mouse
press. Real lock/unlock interaction still requires runtime verification.


## Geometry ownership and observed presentation

The control phase is explicit: inactive, Dock overview, pointer interaction,
overview reconciliation, or layout. Only layout admits mutating commands and
inventory geometry adoption. Reconciliation can publish the selected layout but
cannot issue AX geometry writes. A pointer press blocks geometry commands even
before its target is known; interactive clipping uses the last committed strip
rather than sending new placements to unrelated windows during the drag.

A requested layout, an AX-accepted size, and a drawable WindowServer surface are
three different states. While a column is settling, presentation samples its
actual surface dimensions. Neighbours and stacked rows follow those dimensions;
acceptance alone cannot publish a future size. This sampling also occurs inside
AX progress callbacks, not just after the call returns.

The outer-surface to AX-frame calibration comes from the pre-control snapshots.
It is not re-measured from an AppKit origin left behind by a native group move:
that would interpret the scroll displacement as a titlebar/chrome inset. The
snapshot assumes stable client-frame decoration during a lease; applications
that change their chrome need separate calibration lifecycle verification.


Before AX resize, the payload prepares the physical origin inside the retained
monitor while preserving the current presentation and clip in the same bounded
update. AppKit then adopts that already prepared origin before changing size.
This avoids letting an AX position write temporarily add the difference between
physical and presented origins. Preparation refuses pointer ownership and an
origin outside the retained viewport; it does not resize or change Space membership.

AX position/size requests run on a worker while the calling layout thread keeps
publishing observed geometry through its progress callback. The worker never
mutates layout or invokes the callback. Cancellation stops publication, joins
the in-flight request before releasing AX values, and prevents later geometry
writes. No WindowServer update suspension spans an AX wait.
