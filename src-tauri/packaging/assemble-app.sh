#!/usr/bin/env bash
#
# Assemble a macOS `.app` around an already-built shell binary.
#
# Shared by local packaging and the Nix derivation on purpose: the bundle layout
# is a contract, and having two copies of it would let them drift.
#
# Note what this does NOT do: it never calls `tauri build`. That would run the
# frontend toolchain, which this project deliberately does not need — the shell
# page is three static files and the real GUI is served by the runtime. The Nix
# derivation therefore builds the crate and assembles the bundle here.
#
# Usage:
#   assemble-app.sh --binary <path> --out <dir> [--version <v>]
#
# The result is unsigned. Signing is a separate, documented step because it needs
# credentials and cannot be deterministic (design doc §7.2).

set -euo pipefail

binary=""
out=""
version="0.0.0"
# The bundle identifier is a signing input — it becomes part of the designated
# requirement — so it is declared rather than defaulted silently, and
# `DSH_APP_APP_ID` is how the packaging environment states it.
bundle_id="${DSH_APP_APP_ID:-}"

while [ $# -gt 0 ]; do
  case "$1" in
    --binary) binary="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --version) version="$2"; shift 2 ;;
    --bundle-id) bundle_id="$2"; shift 2 ;;
    *) echo "assemble-app: unknown argument $1" >&2; exit 2 ;;
  esac
done

[ -n "$bundle_id" ] || {
  echo "assemble-app: the bundle identifier is required (--bundle-id or DSH_APP_APP_ID)" >&2
  echo "assemble-app: signing a bundle whose id nobody chose fails much later, and confusingly" >&2
  exit 2
}

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
  <key>CFBundleIdentifier</key><string>$bundle_id</string>
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
