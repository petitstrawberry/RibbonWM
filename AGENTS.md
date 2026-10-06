# RibbonWM development

- Run Cargo, rustc, rustfmt, Clippy, and tests only through the project's Nix shell:
  `nix develop -c cargo ...`.
- Rust 1.91.1 is pinned in `flake.nix`; preserve `flake.lock` and `Cargo.lock`.
  Do not use an ambient rustup toolchain or install tools into the user's home.
- Rust owns the layout, workspace policy, animation, IPC and lifecycle. Native code
  is limited to AppKit / Accessibility glue and the arm64e Dock injection boundary.
- Each monitor/native macOS Space has one horizontal scroll layout. Preserve
  window sizes, transforms and clips across native Space switches; do not add
  another virtual workspace layer or use native Spaces as horizontal columns.
- Preserve real-window clipping and transformed input; do not substitute captured
  window images for live app windows or park windows at screen edges.
- Record build, core tests and native demo evidence before claiming a feature works.
  Do not report Dock injection or multi-display runtime behavior as verified without
  actually running it.
