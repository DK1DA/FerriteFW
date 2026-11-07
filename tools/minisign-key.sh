#!/usr/bin/env bash
# Writes the minisign secret key held in $MINISIGN_KEY (a CI secret) to the file named by $1, for `minisign -S -s`.
# The secret may hold the key file whole or only its base64 line, on one line or wrapped: all whitespace is dropped
# first, then the base64 is the run that starts with "RW" (the comment has no capital R or W). Prints only lengths,
# never the key. Used by .forgejo/workflows/release.yml and sign-check.yml.
set -euo pipefail
out=$1
[ -n "${MINISIGN_KEY:-}" ] || { echo "minisign-key: MINISIGN_KEY is empty" >&2; exit 1; }
flat=$(printf '%s' "$MINISIGN_KEY" | tr -d ' \t\r\n')
b64=$(printf '%s' "$flat" | grep -oE 'RW[A-Za-z0-9+/]{100,}={0,2}' | tail -1 || true)
# The trailing "=" is easily lost copying the line: put the padding back (base64 comes in fours).
while [ $(( ${#b64} % 4 )) -ne 0 ]; do b64="$b64="; done
# A secret key's base64 is 212 characters (158 bytes); a public key's is 56.
echo "minisign-key: secret ${#MINISIGN_KEY} chars, ${#flat} without whitespace, key line ${#b64} chars (212 expected)"
[ ${#b64} -eq 212 ] || { echo "minisign-key: no secret key's base64 in MINISIGN_KEY" >&2; exit 1; }
printf 'untrusted comment: minisign secret key\n%s\n' "$b64" > "$out"
