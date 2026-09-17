#!/usr/bin/env bash
#
# Export a built `.app` out of the Nix store as a writable bundle.
#
# This exists as its own step because it is the *only* reason to copy the app at
# all. The store path is read-only, which is correct for something that is only
# run; signing has to modify the bundle, so it needs a copy — and a copy of a
# read-only tree arrives read-only, because `fs.cp` / `cp -R` carry directory
# permission bits. Widening it is therefore mandatory for anything that will be
# signed, not a workaround for one tool.
#
# Deliberately NOT part of `package-release.mjs`: an unsigned writable bundle
# that looks shippable is worse than no bundle, and signing needs credentials
# that a packaging run must not assume (design doc §7.2).
#
# Usage:
#   export-app.sh <variant> <destination>
#     variant       bundled | slim
#     destination   directory to create the `.app` in
#
# Then, per §7.2: sign every Mach-O inside (including `.node` files, which
# Tauri's own bundler skips), `codesign --verify --deep --strict`, notarize,
# staple, and only then build the dmg.

set -euo pipefail

variant="${1:-}"
destination="${2:-}"

case "$variant" in
  bundled|slim) ;;
  *) echo "usage: export-app.sh <bundled|slim> <destination>" >&2; exit 2 ;;
esac
[ -n "$destination" ] || { echo "export-app.sh: destination is required" >&2; exit 2; }

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"

store="$(nix build --no-link --print-out-paths ".#dsh-app-$variant" --extra-experimental-features 'nix-command flakes' 2>/dev/null | tail -n1)"
[ -n "$store" ] || { echo "export-app.sh: nix build produced no path" >&2; exit 1; }

source_app="$store/Applications/dsh-app.app"
[ -d "$source_app" ] || { echo "export-app.sh: $source_app is missing" >&2; exit 1; }

mkdir -p "$destination"
target="$destination/dsh-app-$variant.app"
rm -rf "$target"

# `ditto` rather than `cp -R`: it preserves extended attributes and resource
# forks, which a macOS bundle can carry and codesign inspects.
ditto "$source_app" "$target"

# The mandatory widening: everything from the store is 0555/0444.
chmod -R u+w "$target"

echo "export-app.sh: $target"
echo "export-app.sh: unsigned — sign, notarize, and staple before distributing (§7.2)"
