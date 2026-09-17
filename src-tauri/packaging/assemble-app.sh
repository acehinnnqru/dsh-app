#!/usr/bin/env bash
#
# Assemble a macOS `.app` around an already-built shell binary.
#
# Shared by local packaging and the Nix derivation on purpose: the bundle layout
# is a contract (the shell resolves its seed from `Contents/Resources/runtimes`)
# and having two copies of it would let them drift.
#
# Note what this does NOT do: it never calls `tauri build`. That would run the
# frontend toolchain, which this project deliberately does not need — the shell
# page is three static files and the real GUI is served by the runtime. The Nix
# derivation therefore builds the crate and assembles the bundle here.
#
# Usage:
#   assemble-app.sh --binary <path> --out <dir> [--seed <runtime-dir>] [--version <v>]
#
# The result is unsigned. Signing is a separate, documented step because it needs
# credentials and cannot be deterministic (design doc §7.2).

set -euo pipefail

binary=""
out=""
seed=""
version="0.0.0"

while [ $# -gt 0 ]; do
  case "$1" in
    --binary) binary="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --seed) seed="$2"; shift 2 ;;
    --version) version="$2"; shift 2 ;;
    *) echo "assemble-app: unknown argument $1" >&2; exit 2 ;;
  esac
done

[ -n "$binary" ] || { echo "assemble-app: --binary is required" >&2; exit 2; }
[ -n "$out" ] || { echo "assemble-app: --out is required" >&2; exit 2; }
[ -f "$binary" ] || { echo "assemble-app: binary not found: $binary" >&2; exit 2; }

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
crate="$(cd "$here/.." && pwd)"

app="$out/dsh-app.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

echo "assemble-app: $app"
install -m 755 "$binary" "$app/Contents/MacOS/dsh-app"

# Sample presets, seeded into the shell's home on first run.
if [ -d "$crate/presets" ]; then
  mkdir -p "$app/Contents/Resources/presets"
  cp -R "$crate/presets/." "$app/Contents/Resources/presets/"
fi

# The runtime seed. Absent in a development build, where the shell reads
# `.dsh-build/runtimes` instead; present in a shipped bundle so first run works
# offline (design doc D8.2 U3).
if [ -n "$seed" ]; then
  [ -f "$seed/runtime.json" ] || { echo "assemble-app: $seed is not a runtime (no runtime.json)" >&2; exit 2; }
  # The directory name becomes the installed version id, so it must agree with
  # the manifest rather than with whatever the caller's directory happened to be
  # called (a build directory is `seed`, a store path is a hash).
  name="$(sed -n 's/.*"dshVersion"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$seed/runtime.json" | head -n1)"
  [ -n "$name" ] || { echo "assemble-app: $seed/runtime.json has no dshVersion" >&2; exit 2; }
  echo "assemble-app: seed runtime $name"
  mkdir -p "$app/Contents/Resources/runtimes/$name"
  cp -R "$seed/." "$app/Contents/Resources/runtimes/$name/"
fi

# The icon, when one has been generated.
if [ -f "$crate/icons/icon.icns" ]; then
  install -m 644 "$crate/icons/icon.icns" "$app/Contents/Resources/icon.icns"
fi

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>dsh-app</string>
  <key>CFBundleDisplayName</key><string>DeepSeek Harness</string>
  <key>CFBundleExecutable</key><string>dsh-app</string>
  <key>CFBundleIdentifier</key><string>com.acehinnnqru.dsh-app</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST

printf 'APPL????' > "$app/Contents/PkgInfo"
echo "assemble-app: done"
