#!/usr/bin/env bash
#
# Produce the entitlements file the bundled Node must be signed with.
#
# The file is derived from the **official nodejs.org binary**, not from the copy
# inside a built bundle. That distinction is the whole point: `install`-ing a
# binary into a bundle invalidates its signature and drops its entitlements, so
# "preserve the metadata of what is already there" silently yields an empty file
# and V8 then fails with `Failed to reserve virtual memory for CodeRange`.
#
# `get-task-allow` is removed. It permits debugger attachment, is a marker of
# development signing, and notarization rejects it — the other five are load
# bearing (JIT, unsigned executable memory, dyld env, executable page protection,
# and library validation for the npm addons signed by other teams).
#
# Usage: extract-node-entitlements.sh <official-node-binary> <output.plist>
set -euo pipefail

source_binary="${1:-}"
output="${2:-}"
[ -f "$source_binary" ] || { echo "extract-node-entitlements: no such binary: $source_binary" >&2; exit 2; }
[ -n "$output" ] || { echo "extract-node-entitlements: output path is required" >&2; exit 2; }

work="$(mktemp)"
trap 'rm -f "$work"' EXIT

codesign -d --entitlements :- "$source_binary" > "$work" 2>/dev/null || true
if ! grep -q "<key>" "$work"; then
  echo "extract-node-entitlements: $source_binary carries no entitlements" >&2
  echo "extract-node-entitlements: it must be the official nodejs.org binary; a copy inside a bundle has none" >&2
  exit 1
fi

/usr/libexec/PlistBuddy -c 'Delete :com.apple.security.get-task-allow' "$work" 2>/dev/null || true
count="$(grep -c '<key>' "$work" || true)"
[ "$count" -ge 4 ] || { echo "extract-node-entitlements: refusing to write only $count entitlement(s)" >&2; exit 1; }

mkdir -p "$(dirname "$output")"
cp "$work" "$output"
echo "extract-node-entitlements: $count entitlements -> $output"
grep -o '<key>[^<]*</key>' "$output" | sed 's/<[^>]*>//g' | sed 's/^/  /'
