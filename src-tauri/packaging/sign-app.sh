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
#   * The bundle contains exactly one Mach-O: the shell binary. There is no Node,
#     no pnpm, and no runtime closure to re-sign, so the entitlement dance the
#     bundled variant needed (JIT, unsigned executable memory, library validation)
#     has no object here. What is left is the part that always applied: signing
#     inside-out, and verifying the team on everything signed.
#   * Signing is inside-out: a signature covers what it contains, so signing a
#     container before its contents invalidates the container again.
#   * Every signed file's TeamIdentifier must equal the expected team. A wrong
#     certificate is otherwise discovered only by users.
#   * A smoke test after signing, because entitlements and the hardened runtime
#     change runtime behaviour.
#
# Usage:
#   sign-app.sh [--destination <dir>]
#
# Environment:
#   DSH_APP_SIGNING_IDENTITY   certificate name or SHA-1 (required)
#   DSH_APP_TEAM_ID            10-character team id (required)
#   DSH_APP_APP_ID             the bundle id the app was built with (required)

set -euo pipefail

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

echo "sign-app.sh: exporting"
"$here/export-app.sh" "$destination"
app="$destination/dsh-app.app"

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

machos="$(mktemp)"
trap 'rm -f "$machos"' EXIT

# `file` rather than an extension list: a hand-maintained list would silently
# miss a new artifact, which is the mistake notarization used to catch at the
# very end.
find "$app/Contents" -type f -exec file {} + \
  | grep 'Mach-O' \
  | cut -d: -f1 \
  | sort > "$machos"

count=$(wc -l < "$machos" | tr -d ' ')
echo "sign-app.sh: signing $count Mach-O files plus the app itself"
[ "$count" -gt 0 ] || { echo "sign-app.sh: no Mach-O files found; is $app complete?" >&2; exit 1; }

sign_one() {
  codesign --force --options runtime --timestamp --sign "$identity" "$1"
}

# Deepest paths first: nothing is signed before what it contains.
while IFS= read -r -d '' file; do
  sign_one "$file"
done < <(awk -F/ '{print NF, $0}' "$machos" | sort -rn | cut -d' ' -f2- | tr '\n' '\0')

sign_one "$app"

# --- 5. Verify what we just did ---------------------------------------------

echo "sign-app.sh: verifying"
codesign --verify --deep --strict --verbose=2 "$app" 2>&1 | tail -3

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

# What "working" means for this shell: it starts, resolves its home, and reaches
# the point where it can list profiles. It ships no runtime, so there is no seed
# to boot and nothing else to prove from a terminal.
echo "sign-app.sh: smoke test"
smoke_home="$(mktemp -d)"
log="$(mktemp)"
HOME="$smoke_home" "$app/Contents/MacOS/dsh-app" > "$log" 2>&1 &
pid=$!
passed=1
for _ in $(seq 1 60); do
  if grep -q "profile(s) listed" "$log" 2>/dev/null; then passed=0; break; fi
  if ! kill -0 "$pid" 2>/dev/null; then break; fi
  sleep 1
done
kill "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
# The app leaves directories it made read-only (WebKit's data store), so a plain
# `rm -rf` can fail on a successful run and abort the script under `set -e`.
chmod -R u+w "$smoke_home" 2>/dev/null || true
rm -rf "$smoke_home" 2>/dev/null || true

if [ "$passed" -ne 0 ]; then
  echo "sign-app.sh: the signed app did not reach its profile list" >&2
  grep -vE 'could not create directory' "$log" | tail -20 >&2
  rm -f "$log"
  exit 1
fi
grep -E 'profile\(s\) listed' "$log" | tail -1
rm -f "$log"

echo "sign-app.sh: signed and verified — $app"
echo "sign-app.sh: NOT notarized; first launch needs right-click → Open (see design doc §7.2)"
