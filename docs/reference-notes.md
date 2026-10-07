# Behavioral references

These are inspected implementations, not runtime comparisons on this host.

- [yabai frame writes](https://github.com/asmvik/yabai/blob/dd845723416f5fe92af49fad5ebab00369e07edd/src/window_manager.c#L729) and [enhanced-UI workaround](https://github.com/asmvik/yabai/blob/dd845723416f5fe92af49fad5ebab00369e07edd/src/misc/helpers.h#L524): owner AX animation and stale notification caches matter. RibbonWM measures current AX/WindowServer geometry and temporarily restores the enhanced-UI flag around a resize. Its observer transport follows the per-application AXObserver model.
- [PaperWM target geometry](https://github.com/paperwm/PaperWM/blob/8bf6dd264f60d6c0c402b63df7b424b888959a48/tiling.js#L561), [easing](https://github.com/paperwm/PaperWM/blob/8bf6dd264f60d6c0c402b63df7b424b888959a48/utils.js#L578) and [defaults](https://github.com/paperwm/PaperWM/blob/8bf6dd264f60d6c0c402b63df7b424b888959a48/schemas/org.gnome.shell.extensions.paperwm.gschema.xml): avoid repeated target-size requests; use bounded layout reconciliation. RibbonWM's service uses quadratic ease-in-out over 0.25 seconds and the approximately 38/50/62 percent width presets. These algorithms are implemented independently.
- [OmniWM parking](https://github.com/OmniNull/OmniWM/blob/ee9ba550e28137be325808eacf49f0b16faa4d33/Sources/OmniWM/Core/Controller/LayoutRefreshController%2BWindowParking.swift#L104) and [display diagnostics](https://github.com/OmniNull/OmniWM/blob/ee9ba550e28137be325808eacf49f0b16faa4d33/Sources/OmniWM/Core/Diagnostics/DiagnosticsIssue.swift#L143): its parking path applies SkyLight positions and AX frames; diagnostics identify neighboring-display leakage under vertically overlapping arrangements. This is evidence of that documented constraint, not a claim that all multi-monitor configurations fail. RibbonWM instead keeps actual surfaces under Dock-owned transforms and explicit drawing/input clips, including while an owner is dragging.

PaperWM and OmniWM source is used as design research; no GPL source is copied into RibbonWM.

## Focus and overview follow-up (2026-10-07)

- OmniWM inspected at `2e08501ec4e0759d6f6a0cfa2ca3f2005fa062c0`:
  [GPL-2.0-only source notice and activation observation](https://github.com/OmniNull/OmniWM/blob/2e08501ec4e0759d6f6a0cfa2ca3f2005fa062c0/Sources/OmniWM/Core/Controller/AXEventHandler%2BFocusObservation.swift).
  Its observation path distinguishes native activation, focus changes and probes,
  and guards against background observations overruling the active application.
  This is design research only; no OmniWM source was copied or translated.
- paneru inspected at `b1b6abbd3f1a4be138152b6f0389c9ff1b27a269`:
  [MIT license](https://github.com/karinushka/paneru/blob/b1b6abbd3f1a4be138152b6f0389c9ff1b27a269/LICENSE.txt),
  [activation and Mission Control triggers](https://github.com/karinushka/paneru/blob/b1b6abbd3f1a4be138152b6f0389c9ff1b27a269/src/ecs/triggers.rs#L414).
  It suspends scrolling during native overview and reconciles membership on exit.
  No source was copied; future substantial reuse must retain its copyright and
  permission notice.

RibbonWM's independent implementation records native PID/window transitions,
reveals an application on return even when its column was already selected, and
retains the offset for automatic selection on a native Space change. Mission
Control return requests one explicit reveal. The first follow-up restored cached placements during the reconciliation hold,
but subsequent user video exposed an old-layout flash. The transition correction
now resolves native selection before the first resumed frame and reserves that
hold for AX geometry reconciliation. Overview entry exposes full clips without
replacing transforms already controlled by Dock animation. Those
transform/clip details are specific to RibbonWM; the other projects' behavior
is not runtime evidence for this backend.
