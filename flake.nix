{
  description = "RibbonWM Rust development environment";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/3e41b24abd260e8f71dbe2f5737d24122f972158";
  inputs.rust-overlay = {
    url = "github:oxalica/rust-overlay";
    inputs.nixpkgs.follows = "nixpkgs";
  };
  outputs = { self, nixpkgs, rust-overlay, ... }: let
    systems = [ "aarch64-darwin" "x86_64-darwin" ];
    forSystem = system: let
      pkgs = import nixpkgs { inherit system; overlays = [ rust-overlay.overlays.default ]; };
      toolchain = pkgs.rust-bin.stable."1.91.1".default.override {
        extensions = [ "rust-src" "rust-analyzer" ];
      };
      nativeCompiler = pkgs.llvmPackages_21.clang;
      compilerEnvironment = {
        CC = "${nativeCompiler}/bin/clang";
        CXX = "${nativeCompiler}/bin/clang++";
        CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER = "${nativeCompiler}/bin/clang";
        CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER = "${nativeCompiler}/bin/clang";
        MACOSX_DEPLOYMENT_TARGET = "14.0";
      };
      rustPlatform = pkgs.makeRustPlatform { cargo = toolchain; rustc = toolchain; };
      package = (rustPlatform.buildRustPackage.override { stdenv = pkgs.llvmPackages_21.stdenv; }) (compilerEnvironment // {
        pname = "ribbonwm";
        version = "0.1.0";
        # Keep screenshots, verification logs and local build outputs out of the derivation.
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type: let
            relative = pkgs.lib.removePrefix "${toString ./.}/" (toString path);
          in toString path == toString ./.
            || builtins.elem relative [ "Cargo.toml" "Cargo.lock" "LICENSE" "crates" "native"
              "native/demo.m" "native/query.m" "native/bridge.h" "native/skylight.h"
              "native/Makefile" "native/payload.m" "native/loader.m"
              "native/vendor" "native/vendor/yabai-LICENSE.txt" "scripts"
              "scripts/backend-service.py" ]
            || pkgs.lib.hasPrefix "crates/" relative;
        };
        cargoLock.lockFile = ./Cargo.lock;
        nativeBuildInputs = [ nativeCompiler pkgs.makeWrapper ];
        postBuild = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isAarch64 ''
          ${pkgs.gnumake}/bin/make -C native IN_NIX_SHELL=1
        '';
        postInstall = ''
          mkdir -p "$out/share/licenses/ribbonwm"
          cp LICENSE "$out/share/licenses/ribbonwm/LICENSE"
          cp native/vendor/yabai-LICENSE.txt "$out/share/licenses/ribbonwm/yabai-LICENSE.txt"
          mkdir -p "$out/libexec/ribbonwm"
          cp scripts/backend-service.py "$out/libexec/ribbonwm/"
        '' + pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isAarch64 ''
          cp native/build/dock-loader "$out/libexec/ribbonwm/dock-loader"
          mkdir -p "$out/lib/ribbonwm"
          ribbon_hash=$(sha256sum native/build/ribbon-payload.dylib | cut -c1-20)
          cp native/build/ribbon-payload.dylib "$out/lib/ribbonwm/ribbon-payload-$ribbon_hash.dylib"
          ln -s "ribbon-payload-$ribbon_hash.dylib" "$out/lib/ribbonwm/ribbon-payload.dylib"
          makeWrapper ${pkgs.python3}/bin/python3 "$out/bin/ribbonwm-load-backend" \
            --add-flags "$out/libexec/ribbonwm/backend-service.py --loader $out/libexec/ribbonwm/dock-loader --payload $out/lib/ribbonwm/ribbon-payload.dylib"
        '';
        meta = {
          description = "Experimental scrollable tiling window manager for macOS";
          license = pkgs.lib.licenses.mit;
          platforms = systems;
          mainProgram = "ribbonwm";
        };
      });
    in {
      devShell = (pkgs.mkShell.override { stdenv = pkgs.llvmPackages_21.stdenv; }) (compilerEnvironment // {
        packages = [ toolchain nativeCompiler pkgs.gnumake pkgs.python3 ];
        RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
      });
      inherit package;
    };
  in {
    devShells = nixpkgs.lib.genAttrs systems (system: { default = (forSystem system).devShell; });
    packages = nixpkgs.lib.genAttrs systems (system: { default = (forSystem system).package; });
    checks = nixpkgs.lib.genAttrs systems (system: { default = (forSystem system).package; });
    darwinModules.default = { pkgs, ... }: {
      imports = [ (import ./nix/module.nix { ribbonwmPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.default; }) ];
    };
    darwinModules.ribbonwm = self.darwinModules.default;
  };
}
