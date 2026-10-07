# RibbonWM

An experimental Rust proof of concept for a PaperWM/Niri-style scrolling window
manager on macOS. Windows form horizontal columns with optional vertical stacks.
Each native macOS **Space × monitor** retains its own layout and scroll position.

**Development concluded at the PoC stage. This is not a stable daily-driver
replacement for an established window manager.** The implementation demonstrates
real-window scrolling and clipping, but resize transitions, dragging, native
activation and Mission Control still have visible failures. The final experimental
Mission Control changes are committed without production visual acceptance.

## Approach

RibbonWM manipulates real WindowServer surfaces through a small backend injected
into Dock. Rust owns layout, selection, scrolling, window tracking and IPC. The
native layer provides Accessibility/AppKit integration and privileged SkyLight
operations.

The backend combines presentation transforms, monitor clipping and selective
native window anchoring. Accessibility requests negotiate application sizes.
This means physical application geometry and displayed geometry can differ;
coordinating them during drag, resize and macOS animations is a central unresolved
problem. Windows are not replaced with screenshots or parked at screen edges.

Native macOS Spaces remain the workspace system. RibbonWM does not create an
additional virtual desktop layer or use a Space for each horizontal column.
Layouts are retained in memory, not persisted across daemon restarts.

## Implemented scope

- Horizontal columns, vertical stacks, focus, reorder, swap and width presets.
- Per-monitor/per-Space layout and scroll state, with real-window drawing/input
  clipping at monitor boundaries.
- Window creation/destruction tracking and native focus observation, including
  clicks, application activation and Mission Control selection.
- Configurable gaps, per-edge padding, centering and keyboard scroll animation.
- Floating windows centered within the usable area and placed above ordinary
  windows. Their size is preserved where it fits; floating does not automatically
  make them smaller. Original window levels are restored on release.
- Sticky windows across native Spaces of their monitor. Sticky alone does not
  raise the window level.
- Experimental configurable trackpad scrolling, optional modifier keys and
  routing of macOS momentum events when available.
- A subset of yabai's command syntax for existing skhd bindings.
- A pinned Nix build and an optional nix-darwin service module.

These describe implemented code paths, not a guarantee of reliable behavior in
arbitrary applications. See [verification records](docs/verification.md) for the
scope and failures of individual checks.

## Requirements and limitations

Development and runtime experiments used **Apple Silicon and macOS 26.6.2**.
Dock injection requires relaxing the relevant SIP restrictions. RibbonWM does
not change SIP configuration. The private APIs and injection mechanism are
version-sensitive; Intel Dock injection and other macOS versions are unverified.

Accessibility permission is needed for live management. Input Monitoring is
needed for gesture capture. Terminal permission does not necessarily authorize
the launchd service. New Nix store executable paths may need separate macOS
approval. The service waits for permission and retries, but a stale Accessibility
trust state was observed even after approval and required a relaunch.

Known unresolved issues include:

- Visible intermediate gaps/overlap during width changes. Sampled geometry still
  showed large brief errors despite correct final dimensions.
- Drag handoff and native/presentation coordinate synchronization. An owned-window
  test improved, but arbitrary-app drag stability is not established.
- Mission Control entry/exit flicker and selection transitions. The final scene
  staging experiment passed bounded native tests but was not deployed for visual
  acceptance before development concluded.
- Application minimum/fixed-size negotiation and restoration under changing
  constraints. Failed resizes may leave a window floating.
- Display topology changes, fullscreen transitions and Dock restart recovery.
  Automatic Dock reinjection is implemented but not runtime-verified.
- Incomplete crash recovery: the watchdog restores compositor state, sticky tags
  and leased levels, but cannot restore application sizes after SIGKILL.
- Gesture conflicts with application scrolling and macOS Spaces.

Mouse-driven window swapping, cross-monitor window transfer (including dragging),
BSP operations, full yabai rule/signal
compatibility and persistent layout state are not implemented. A configured
`frame_rate = 120` is a scheduling target; measurements do not establish sustained
120 fps or display-synchronized rendering.

## Build with Nix

```sh
git clone https://github.com/petitstrawberry/RibbonWM.git
cd RibbonWM
nix develop -c sh scripts/check.sh
nix build
./result/bin/ribbonwm doctor
```

The flake pins **Rust 1.91.1** and **Clang 21**. Run development commands through
`nix develop`; no ambient rustup toolchain is needed. The Apple Silicon package
includes the CLI, Dock loader and signed backend. Building does not install or
start services.

The owned-window demo can be run independently of live management:

```sh
nix develop -c cargo run --locked -- demo
nix develop -c cargo run --locked -- demo --test
```

## Manual live experiment

Use a disposable session or selected test windows. Stop other window managers
before enabling live management. Loading the backend requires administrator
authentication:

```sh
nix develop -c sh scripts/load-backend.sh
nix develop -c cargo run --locked -- run --all --config config/live.toml \
  --exclude-app com.apple.systempreferences
```

Use `--windows ID,ID` instead of `--all` to restrict the managed set. `--dry-run`
exercises layout and IPC without changing application sizes or compositor state.
`--exclude-app` accepts an app name or bundle ID with an optional trailing `*`.

Useful diagnostics:

```sh
ribbonwm doctor
ribbonwm request-permissions --input-monitoring
ribbonwm backend-status
ribbonwm status
ribbonwm quit
```

Normal quit attempts to settle windows into the current usable area and release
transforms, clips and leases. It does not restore an old startup layout or old
Space membership. This is best-effort cleanup, not arbitrary-app recovery proof.
Stop RibbonWM before starting another window manager.

## nix-darwin module

Add the input and module to your system flake:

```nix
inputs.ribbonwm.url = "github:petitstrawberry/RibbonWM";

# Inside darwinSystem.modules:
modules = [
  inputs.ribbonwm.darwinModules.default
  {
    services.yabai.enable = false;
    services.ribbonwm = {
      enable = true;
      user = "YOUR_LOGIN_NAME";
      enableDockInjection = true;
      settings = {
        padding_top = 24.0;
        padding_bottom = 24.0;
        padding_left = 24.0;
        padding_right = 24.0;
        gap = 6.0;
        preserve_window_width = true;
        center_content = true;
        focus_alignment = "visible";
        animation_curve = "ease_out";
        animation_duration = 0.10;
        cycle_width_ratios = [ 0.38195 0.5 0.61804 ];
        frame_rate = 120;
      };
      excludeApps = [ "com.apple.systempreferences" ];
    };
  }
];
```

The module rejects simultaneous yabai/RibbonWM enablement. The user LaunchAgent
`org.nixos.ribbonwm` starts the Rust binary directly. With Dock injection enabled,
root LaunchDaemon `org.nixos.ribbonwm-backend` watches the user's Dock and loads
the packaged payload. A payload identity check gates live management.

To update an existing system input, run `nix flake update ribbonwm` in the system
flake directory before rebuilding. A rebuild alone continues using its locked
revision. Apply through your normal `darwin-rebuild switch` workflow.

Logs: `~/Library/Logs/RibbonWM/wm.log` and `/var/log/ribbonwm-backend.log`.
A normal `ribbonwm quit` leaves the service stopped; launchd restarts failures.
To disable it, set `services.ribbonwm.enable = false` and rebuild your system.

## Configuration and gestures

See [config/live.toml](config/live.toml) for settings and [config/skhdrc](config/skhdrc)
for key bindings. Runtime configuration changes do not persist:

```sh
ribbonwm config gap 6
ribbonwm config padding_top 24
ribbonwm config animation_duration 0.10
```

An animation duration of `0` requests immediate movement. For experimental
Option + two-finger horizontal scrolling:

```toml
gesture_scroll = true
gesture_fingers = 2
gesture_modifier = "alt"
gesture_sensitivity = 1.0
gesture_reverse = false
gesture_momentum = true
```

Gestures are disabled by default. Modifiers can be `alt`, `ctrl`, `super` or
`shift`; omitting `gesture_modifier` requests keyless capture. Finger modes are
2, 3 and 4. Two-finger mode uses precise scroll events, which do not identify
finger count and may also come from devices such as Magic Mouse. Three/four-finger
modes track touch contacts. Four-finger physical input remains unverified.

Momentum is consumed only when macOS supplies it; no synthetic inertia is added.
The observed two-finger session supplied momentum, while the three-finger session
did not. Keyless capture and matching macOS Space gestures can conflict. OS
settings are not changed automatically.

`ribbonwm gesture-monitor --seconds 30 --fingers 2` opens a visible diagnostic
window and records after its Start button is pressed. It does not consume input.

## Commands

| Command | Purpose |
| --- | --- |
| `focus left/right/up/down` | Select an adjacent column or stacked row |
| `focus prev/next/first/last/recent/largest/smallest/mouse/ID` | Select by order, history, size, pointer or ID |
| `focus stack.next/stack.prev/stack.first/stack.last/stack.N` | Select within a stack; N starts at 1 |
| `focus-monitor east/west/north/south/next/prev/N/UUID` | Focus a monitor's selected window |
| `move SELECTOR` / `swap SELECTOR` | Reorder within the current monitor/Space |
| `stack SELECTOR` / `unstack` / `balance` | Stack, separate or equalize row heights |
| `resize WIDTH` / `resize --by DELTA` / `resize --ratio RATIO` | Change column width |
| `resize-height HEIGHT` / `resize-height --by DELTA` | Change stacked row height |
| `toggle-full-width` / `cycle-width` | Toggle usable full width or cycle width presets |
| `mirror columns` / `mirror rows` | Reverse columns or rows while retaining selection |
| `center` / `scroll DELTA` | Center the selected column or scroll horizontally |
| `float on/off/toggle` / `sticky on/off/toggle` | Toggle floating or native sticky state |
| `status` / `windows` / `displays` / `backend-status` | Inspect state |
| `quit` | Release managed state and exit normally |

Native commands accept `--monitor UUID`; the default target is the monitor under
the pointer. The yabai syntax adapter instead targets the keyboard-focused
monitor. Both `ribbonwm -m …` and `ribbonwm yabai -m …` are accepted:

```sh
ribbonwm -m window --focus east
ribbonwm -m window --warp west
ribbonwm -m window --swap next
ribbonwm -m window --toggle float
ribbonwm -m window --toggle sticky
ribbonwm -m window --toggle zoom-fullscreen
ribbonwm -m window --resize cycle
ribbonwm -m window --center
ribbonwm -m window --focus stack.next
```

This is a syntax subset with scrolling-layout semantics. `zoom-fullscreen` means
full column width. `space --mirror y-axis` reverses columns; `x-axis` reverses
stacked rows. `--center` and `--resize cycle` are RibbonWM extensions. Supported
operations also include `--stack`, relative edge resize, `space --balance`,
`display --focus`, spacing config and window/display queries. Unsupported syntax
returns an error rather than silently approximating it.

## Development and license

- `ribbon-core`: platform-independent layout, scrolling and selection.
- `ribbon-macos`: Accessibility/AppKit and Dock backend interfaces.
- `ribbonwm`: CLI, IPC, tracking, lifecycle and service entry points.

See [architecture](docs/architecture.md) and [verification](docs/verification.md).
The final suite passed 79 Rust tests, formatting, Clippy, the native build,
owned-surface regression and socket lifecycle checks, plus a Nix package build.
Those bounded checks do not establish desktop stability or visual smoothness.

MIT licensed. The Mach loader includes yabai-derived code with its original
copyright and attribution; the [upstream license](native/vendor/yabai-LICENSE.txt)
is included. PaperWM/Niri inspire the interaction model; the Rust layout
implementation is independent.
