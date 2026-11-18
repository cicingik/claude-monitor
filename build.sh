#!/usr/bin/env bash
# Builds release archives of office for several OS/arch targets.
#
# Usage: ./build.sh [--setup] [TARGET...]
#
# With no TARGET, builds every default target. Every target builds with
# cargo-zigbuild (zig is the cross linker), except a host target that is not
# Linux, which builds with plain cargo. Linux targets link against glibc
# 2.17 (GLIBC below), so the binary runs on distros older than the build
# machine.
# --setup installs what is missing first: rustup targets, cargo-zigbuild, and
# zig (a tarball in ~/.local/opt, linked from ~/.local/bin/zig).
#
# Output, in <repo>/dist:
#   office-<version>-<target>.tar.gz  (.zip for Windows when zip exists)
#     each holds office-<version>-<target>/ with the binary, assets/office.png,
#     assets/character.gif and README.md
#   SHA256SUMS  (check with: cd dist && sha256sum -c SHA256SUMS)
set -euo pipefail

DEFAULT_TARGETS=(
  x86_64-unknown-linux-gnu
  aarch64-unknown-linux-gnu
  x86_64-apple-darwin
  aarch64-apple-darwin
  x86_64-pc-windows-gnu
  aarch64-pc-windows-gnullvm
)
ZIG_VERSION=0.14.1
GLIBC=2.17

dir=$(cd "$(dirname "$0")" && pwd)
repo=$dir
dist="$repo/dist"

setup=0
targets=()
for a in "$@"; do
  case $a in
    --setup) setup=1 ;;
    -h | --help) sed -n '2,/^set /{/^set /!p}' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "unknown flag: $a" >&2; exit 2 ;;
    *) targets+=("$a") ;;
  esac
done
[ ${#targets[@]} -gt 0 ] || targets=("${DEFAULT_TARGETS[@]}")

command -v cargo >/dev/null && command -v rustup >/dev/null || {
  echo "cargo and rustup are missing. Install Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
  exit 1
}
host=$(rustc -vV | sed -n 's/^host: //p')
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$dir/Cargo.toml" | head -n1)

have_zig() { command -v zig >/dev/null || python3 -c 'import ziglang' 2>/dev/null; }

install_zig() {
  local arch os
  case $(uname -m) in x86_64 | amd64) arch=x86_64 ;; aarch64 | arm64) arch=aarch64 ;; *) arch=$(uname -m) ;; esac
  case $(uname -s) in Linux) os=linux ;; Darwin) os=macos ;; *) echo "install zig by hand: https://ziglang.org/download/" >&2; return 1 ;; esac
  local name="zig-$arch-$os-$ZIG_VERSION"
  mkdir -p "$HOME/.local/opt" "$HOME/.local/bin"
  curl -fsSL "https://ziglang.org/download/$ZIG_VERSION/$name.tar.xz" | tar -xJ -C "$HOME/.local/opt"
  ln -sf "$HOME/.local/opt/$name/zig" "$HOME/.local/bin/zig"
  export PATH="$HOME/.local/bin:$PATH"
  echo "zig $(zig version) installed at ~/.local/bin/zig (put ~/.local/bin on PATH)"
}

# Prerequisites: what is missing, and the command that installs it.
installed=$(rustup target list --installed)
missing_targets=()
need_zig=0
for t in "${targets[@]}"; do
  grep -qx "$t" <<<"$installed" || missing_targets+=("$t")
  { [ "$t" = "$host" ] && [[ $t != *-linux-gnu ]]; } || need_zig=1
done
missing=()
[ ${#missing_targets[@]} -eq 0 ] || missing+=("rustup target add ${missing_targets[*]}")
if [ $need_zig = 1 ]; then
  command -v cargo-zigbuild >/dev/null || missing+=("cargo install --locked cargo-zigbuild")
  have_zig || missing+=("zig $ZIG_VERSION: brew install zig, or $0 --setup (tarball into ~/.local/opt), or https://ziglang.org/download/")
fi
if [ ${#missing[@]} -gt 0 ]; then
  if [ $setup = 0 ]; then
    echo "Missing prerequisites. Install them with:" >&2
    printf '  %s\n' "${missing[@]}" >&2
    echo "or run: $0 --setup ${targets[*]}" >&2
    exit 1
  fi
  [ ${#missing_targets[@]} -eq 0 ] || rustup target add "${missing_targets[@]}"
  if [ $need_zig = 1 ]; then
    command -v cargo-zigbuild >/dev/null || cargo install --locked cargo-zigbuild
    have_zig || install_zig
  fi
fi

for f in assets/office.png assets/character.gif README.md; do
  [ -f "$repo/$f" ] || { echo "missing $repo/$f: every archive ships it" >&2; exit 1; }
done

if command -v sha256sum >/dev/null; then sha256() { sha256sum "$@"; }; else sha256() { shasum -a 256 "$@"; }; fi

mkdir -p "$dist"
built=()
failed=()
for t in "${targets[@]}"; do
  echo "==> $t"
  if [[ $t == *-linux-gnu ]]; then
    build=(cargo zigbuild --target "$t.$GLIBC")
  elif [ "$t" = "$host" ]; then
    build=(cargo build --target "$t")
  else
    build=(cargo zigbuild --target "$t")
  fi
  if ! "${build[@]}" --release --locked --manifest-path "$dir/Cargo.toml"; then
    echo "!! $t: build failed" >&2
    failed+=("$t")
    continue
  fi
  exe=office
  [[ $t == *windows* ]] && exe=office.exe
  name="office-$version-$t"
  stage=$(mktemp -d)
  mkdir "$stage/$name"
  mkdir "$stage/$name/assets"
  cp "$dir/target/$t/release/$exe" "$repo/README.md" "$stage/$name/"
  cp "$repo/assets/office.png" "$repo/assets/character.gif" "$stage/$name/assets/"
  rm -f "$dist/$name.tar.gz" "$dist/$name.zip"
  if [[ $t == *windows* ]] && command -v zip >/dev/null; then
    (cd "$stage" && zip -qr "$dist/$name.zip" "$name")
  else
    tar -czf "$dist/$name.tar.gz" -C "$stage" "$name"
  fi
  rm -rf "$stage"
  built+=("$t")
done

(
  cd "$dist"
  shopt -s nullglob
  files=(office-*.tar.gz office-*.zip)
  [ ${#files[@]} -eq 0 ] || sha256 "${files[@]}" >SHA256SUMS
)

echo
echo "built (${#built[@]}): ${built[*]:-none}"
[ ${#failed[@]} -eq 0 ] || { echo "failed (${#failed[@]}): ${failed[*]}" >&2; exit 1; }
echo "archives and SHA256SUMS in $dist"
