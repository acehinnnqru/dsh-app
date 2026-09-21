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
# There is no variant argument: the shell carries no Node, no pnpm, and no
# runtime closure, so there is exactly one bundle to export.
#
# Usage:
#   export-app.sh <destination>
#     destination   directory to create the `.app` in
#
# Then, per design doc §7.2: sign every Mach-O inside, `codesign --verify
# --deep --strict`, notarize, staple, and only then build the dmg.

set -euo pipefail

destination="${1:-}"
[ -n "$destination" ] || { echo "usage: export-app.sh <destination>" >&2; exit 2; }

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"

# The bundle id must be the one the app was built with: signing under a different
# id changes the designated requirement. It reaches the build through the
# environment, which is why `--impure` is required (flakes reject `--argstr`).
: "${DSH_APP_APP_ID:?export-app.sh: DSH_APP_APP_ID is required to build an app bundle}"
store="$(nix build --impure --no-link --print-out-paths \
  ".#dsh-app" --extra-experimental-features 'nix-command flakes' 2>/dev/null | tail -n1)"
[ -n "$store" ] || { echo "export-app.sh: nix build produced no path" >&2; exit 1; }

source_app="$store/Applications/dsh-app.app"
[ -d "$source_app" ] || { echo "export-app.sh: $source_app is missing" >&2; exit 1; }

mkdir -p "$destination"
target="$destination/dsh-app.app"
rm -rf "$target"

# `ditto` rather than `cp -R`: it preserves extended attributes and resource
# forks, which a macOS bundle can carry and codesign inspects.
ditto "$source_app" "$target"

# The mandatory widening: everything from the store is 0555/0444.
chmod -R u+w "$target"

echo "export-app.sh: $target"
echo "export-app.sh: unsigned — sign, notarize, and staple before distributing (§7.2)"
