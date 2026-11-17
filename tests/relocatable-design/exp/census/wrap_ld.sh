#!/bin/bash
# ld wrapper: record argv in $CENSUS_ARGS, then exec the real GNU ld of the
# arch named by this wrapper's directory (aarch64 | x86_64 | i686).
arch=$(basename "$(dirname "$0")")
[ -n "$CENSUS_ARGS" ] && printf '%s\n' "$@" > "$CENSUS_ARGS"
case "$arch" in
  aarch64) real=/usr/bin/ld.bfd ;;
  x86_64) real=/usr/bin/x86_64-linux-gnu-ld.bfd ;;
  i686) real=/usr/bin/i686-linux-gnu-ld.bfd ;;
esac
exec "$real" "$@"
