#!/usr/bin/env bash
# Headless screenshots of the Slint UI on Linux (for development): runs the demo under Xvfb with a script.
#   scripts/snap.sh "go games; shot snaps/games.png" [--lang fr] [--resume show]
# Needs Xvfb, plus libxkbcommon-x11 / libxcb-xkb in ~/.local/lounge-libs if the system lacks them.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
script="$1"; shift
bin="${CARGO_TARGET_DIR:-$here/target}/debug/lounge"
export LD_LIBRARY_PATH="$HOME/.local/lounge-libs${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export SLINT_BACKEND="${SLINT_BACKEND:-winit-skia-software}"
cd "$here"
exec timeout 120 xvfb-run -a -s "-screen 0 1600x1000x24" "$bin" --demo --size 1600x1000 --script "$script" "$@"
