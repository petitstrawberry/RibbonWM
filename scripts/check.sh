#!/bin/sh
set -eu
if [ -z "${IN_NIX_SHELL:-}" ]; then
    echo 'Run: nix develop -c sh scripts/check.sh' >&2
    exit 1
fi
ribbon_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ribbon_root"
rustc --version
cargo --version
"$CC" --version
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --workspace
make -C native
