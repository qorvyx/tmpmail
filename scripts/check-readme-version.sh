#!/usr/bin/env bash
# Usage: check-readme-version.sh VERSION [README]
# Fails unless the release version README names is VERSION everywhere it
# names one: the attestation example, the install URL and its tar path
# (`tmpmail-VERSION-TARGET`, `/download/vVERSION/`), and the library's
# `version = "MAJOR.MINOR"`. Run by release.yml on a `v*` tag.
set -euo pipefail
if [ $# -lt 1 ] || [ -z "$1" ]; then
  echo "usage: $0 VERSION [README]" >&2
  exit 2
fi
v="$1"
readme="${2:-README.md}"
core="${v%%[-+]*}"   # 0.4.0-rc.1 -> 0.4.0
minor="${core%.*}"   # -> 0.4
fail=0
need() {
  if ! grep -Fq -- "$1" "$readme"; then
    echo "::error file=$readme::$readme doesn't name $2: expected \`$1\`"
    fail=1
  fi
}
need "gh attestation verify tmpmail-$v-" "version $v in the attestation example"
need "releases/download/v$v/tmpmail-$v-" "version $v in the install URL"
need "tmpmail-$v-x86_64-unknown-linux-musl/tmpmail" "version $v in the install tar path"
need "tmpmail = { version = \"$minor\"" "library version $minor"
# Every version named in those forms, whole (a pre-release suffix included).
while IFS= read -r hit; do
  line="${hit%%:*}"
  text="${hit#*:}"
  case "$text" in
    "tmpmail = { version = \"$minor\"" | "/download/v$v/" | "tmpmail-$v-x86_64" | "tmpmail-$v-aarch64") ;;
    *)
      echo "::error file=$readme,line=$line::$readme names another version than $v: $text"
      fail=1
      ;;
  esac
done < <(grep -noE 'tmpmail-[0-9][0-9A-Za-z.+-]*-(x86_64|aarch64)|/download/v[^/[:space:]]+/|tmpmail = \{ version = "[^"]*"' "$readme" || true)
if [ "$fail" = 0 ]; then
  echo "$readme names version $v in every release link and the library version"
fi
exit "$fail"
