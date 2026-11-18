#!/bin/bash
# Opens office in a WezTerm window of its own, not attached to the
# wezterm-mux-server. Through the mux, every image is handed from the server to
# the window, and replacing images many times a second makes that hand-over
# fail and drop the whole domain with all its panes. In a window of its own the
# images never leave the process. The window closes when office quits.
#
# Usage, from WSL: ./window.sh [office flags...]
set -euo pipefail

dir=$(cd "$(dirname "$0")" && pwd)
repo=$dir
bin="$dir/target/release/office"
[ -x "$bin" ] || cargo build --release --manifest-path "$dir/Cargo.toml"

wezterm=$(command -v wezterm.exe || echo "/mnt/c/Program Files/WezTerm/wezterm.exe")
distro=${WSL_DISTRO_NAME:?not running in WSL}

# wezterm.exe is a Windows program: give it a Windows working directory.
cd /mnt/c
exec "$wezterm" start --always-new-process --domain "WSL:$distro" --cwd "$repo" -- "$bin" "$@"
