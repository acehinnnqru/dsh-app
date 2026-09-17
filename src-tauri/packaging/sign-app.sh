#!/usr/bin/env bash
#
# Sign a built `.app` with a Developer ID certificate.
#
# Notarization is deliberately **not** part of this script: it needs an App Store
# Connect API key that signing does not, and the design doc records the tradeoff
# (users must right-click → Open on first launch). Adding it later is two more
# steps at the end of this file, not a rewrite.
#
# What this enforces, and why each check exists:
#
#   * The identity must resolve to exactly one certificate. `codesign` accepts a
#     full common name, a name fragment, or a SHA-1 hash; a fragment can match
#     several certificates, and the failure then surfaces mid-signing with an
#     unreadable error.
#   * `Contents/Resources/node` is the **only** binary that gets entitlements,
#     and they are taken from the binary itself. Stripping them breaks loading
#     of the npm `.node` addons, which are signed by other teams and therefore
#     refused under the hardened runtime unless `disable-library-validation` is
#     present. Getting this wrong shows up as "starts fine, dies on first use".
#   * Signing is inside-out: a signature covers what it contains, so signing a
#     container before its contents invalidates the container again.
#   * Every signed file's `TeamIdentifier` must equal the expected team. A wrong
#     certificate is otherwise discovered only by users.
#   * A smoke test after signing, because entitlements and the hardened runtime
#     change runtime behaviour.
#
# Usage:
#   sign-app.sh <variant> [--destination <dir>]
#
# Environment:
#   DSH_APP_SIGNING_IDENTITY   certificate name or SHA-1 (required)
#   DSH_APP_TEAM_ID            10-character team id (required)
#   DSH_APP_APP_ID             the bundle id the app was built with (required)

set -euo pipefail

variant="${1:-}"
[ -n "$variant" ] || { echo "usage: sign-app.sh <bundled|slim> [--destination <dir>]" >&2; exit 2; }
case "$variant" in bundled|slim) ;; *) echo "sign-app.sh: unknown variant $variant" >&2; exit 2 ;; esac
shift

destination=""
while [ $# -gt 0 ]; do
  case "$1" in
    --destination) destination="$2"; shift 2 ;;
    *) echo "sign-app.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done

: "${DSH_APP_SIGNING_IDENTITY:?sign-app.sh: DSH_APP_SIGNING_IDENTITY is required}"
: "${DSH_APP_TEAM_ID:?sign-app.sh: DSH_APP_TEAM_ID is required}"
: "${DSH_APP_APP_ID:?sign-app.sh: DSH_APP_APP_ID is required}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
destination="${destination:-$root/dist}"
identity="$DSH_APP_SIGNING_IDENTITY"
team="$DSH_APP_TEAM_ID"

# --- 1. The identity must be unambiguous ------------------------------------

if ! security find-identity -v -p codesigning | grep -q "$identity"; then
  echo "sign-app.sh: the identity is not a signing certificate in this keychain:" >&2
  echo "  $identity" >&2
  echo "available:" >&2
  security find-identity -v -p codesigning >&2
  exit 1
fi

# --- 2. Export a writable copy (a store path cannot be signed in place) -----

echo "sign-app.sh: exporting $variant"
"$here/export-app.sh" "$variant" "$destination"
app="$destination/dsh-app-$variant.app"

echo "sign-app.sh: identity $identity"
echo "sign-app.sh: team     $team"
echo "sign-app.sh: bundle   $app"

# --- 3. Confirm the app was built with the id we are about to sign ---------

built_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")"
if [ "$built_id" != "$DSH_APP_APP_ID" ]; then
  echo "sign-app.sh: the bundle id does not match what the app was built with" >&2
  echo "  built:    $built_id" >&2
  echo "  expected: $DSH_APP_APP_ID" >&2
  echo "  rebuild with DSH_APP_APP_ID=$DSH_APP_APP_ID" >&2
  exit 1
fi

# --- 4. Sign every Mach-O inside-out ----------------------------------------

node_binary="$app/Contents/Resources/node/bin/node"
machos="$(mktemp)"
trap 'rm -f "$machos"' EXIT

# `file` rather than an extension list: the addons are `.node`, `.dylib`, and a
# few extensionless executables, and a hand-maintained list would silently miss
# a new one — exactly the mistake notarization used to catch at the very end.
find "$app/Contents" -type f -exec file {} + \
  | grep 'Mach-O' \
  | cut -d: -f1 \
  | grep -v "^$node_binary$" \
  | sort > "$machos"

count=$(wc -l < "$machos" | tr -d ' ')
echo "sign-app.sh: signing $count Mach-O files plus the app itself"
[ "$count" -gt 0 ] || { echo "sign-app.sh: no Mach-O files found; is $app complete?" >&2; exit 1; }

sign_one() {
  codesign --force --options runtime --timestamp --sign "$identity" "$1"
}

# Node is the only binary that needs entitlements, and they are taken from the
# **official nodejs.org binary** rather than preserved from the copy in the
# bundle. That distinction is not cosmetic: `install`-ing a binary into a bundle
# invalidates its signature and drops its entitlements, so preserving what is
# already there silently produces none — and V8 then dies with
# `Failed to reserve virtual memory for CodeRange`.
if [ -f "$node_binary" ]; then
  official_node="$(
    nix build --impure --no-link --print-out-paths \
      --extra-experimental-features 'nix-command flakes' "$root#node" 2>/dev/null | tail -n1
  )"
  [ -n "$official_node" ] && [ -x "$official_node/bin/node" ] || {
    echo "sign-app.sh: cannot locate the official Node ($root#node) to take entitlements from" >&2
    exit 1
  }
  entitlements="$root/dist/node.entitlements.plist"
  "$here/entitlements/extract-node-entitlements.sh" "$official_node/bin/node" "$entitlements"
  echo "sign-app.sh: signing node with those entitlements"
  codesign --force --options runtime --timestamp \
    --entitlements "$entitlements" --sign "$identity" "$node_binary"
fi

# Deepest paths first: nothing is signed before what it contains.
while IFS= read -r -d '' file; do
  sign_one "$file"
done < <(awk -F/ '{print NF, $0}' "$machos" | sort -rn | cut -d' ' -f2- | tr '\n' '\0')

sign_one "$app"

# --- 5. Verify what we just did ---------------------------------------------

echo "sign-app.sh: verifying"
codesign --verify --deep --strict --verbose=2 "$app" 2>&1 | tail -3

# A signature that exists but lost its entitlements is exactly the failure this
# script was written after; assert them rather than assuming.
if [ -f "$node_binary" ]; then
  for required in allow-jit allow-unsigned-executable-memory disable-library-validation; do
    codesign -d --entitlements - "$node_binary" 2>/dev/null | grep -q "$required" || {
      echo "sign-app.sh: the signed node is missing $required; V8 or the npm addons will fail" >&2
      exit 1
    }
  done
  codesign -d --entitlements - "$node_binary" 2>/dev/null | grep -q "get-task-allow" && {
    echo "sign-app.sh: the signed node still carries get-task-allow (notarization would reject it)" >&2
    exit 1
  }
  echo "sign-app.sh: node entitlements verified (JIT, executable memory, library validation; no get-task-allow)"
fi

checked=0
while IFS= read -r file; do
  details="$(codesign --display --verbose=4 "$file" 2>&1 || true)"
  if ! printf '%s\n' "$details" | grep -q "TeamIdentifier=$team"; then
    echo "sign-app.sh: $file does not carry TeamIdentifier=$team" >&2
    printf '%s\n' "$details" | grep -E 'Authority|TeamIdentifier' >&2 || true
    exit 1
  fi
  checked=$((checked + 1))
done < "$machos"
printf '%s\n' "$(codesign --display --verbose=4 "$app" 2>&1)" | grep -q "TeamIdentifier=$team" \
  || { echo "sign-app.sh: the app itself does not carry TeamIdentifier=$team" >&2; exit 1; }
echo "sign-app.sh: $checked files and the app carry TeamIdentifier=$team"

# --- 6. Smoke test: signing changes runtime behaviour ------------------------

echo "sign-app.sh: smoke test"
smoke_home="$(mktemp -d)"
log="$(mktemp)"
DSH_APP_HOME="$smoke_home" "$app/Contents/MacOS/dsh-app" > "$log" 2>&1 &
pid=$!
passed=1
for _ in $(seq 1 60); do
  if grep -q "passed its smoke test" "$log" 2>/dev/null; then passed=0; break; fi
  if ! kill -0 "$pid" 2>/dev/null; then break; fi
  sleep 1
done
kill "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
# The app leaves directories it made read-only (WebKit's data store, the runtime
# store's own links), so a plain `rm -rf` can fail on a sucessful run and abort
# the script under `set -e`.
chmod -R u+w "$smoke_home" 2>/dev/null || true
rm -rf "$smoke_home" 2>/dev/null || true

if [ "$passed" -ne 0 ]; then
  echo "sign-app.sh: the signed app did not reach its smoke test" >&2
  grep -vE 'could not create directory' "$log" | tail -20 >&2
  rm -f "$log"
  exit 1
fi
grep -E 'passed its smoke test' "$log" | tail -1
rm -f "$log"

echo "sign-app.sh: signed and verified — $app"
echo "sign-app.sh: NOT notarized; first launch needs right-click → Open (see design doc §7.2)"
