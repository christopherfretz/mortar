#!/bin/bash
# census ld wrapper: record argv (one per line, argv[0] basename first) in
# $CENSUS_ARGS (or a fresh file under $CENSUS_ARGS_DIR), then exec the real
# GNU ld for the arch that is this wrapper's directory name
# (aarch64 | x86_64 | i686).
name=$(basename "$0")
arch=$(basename "$(dirname "$0")")
if [ -n "$CENSUS_ARGS" ]; then printf '%s\n' "$name" "$@" > "$CENSUS_ARGS"; fi
if [ -n "$CENSUS_ARGS_DIR" ]; then
  n=0; while [ -e "$CENSUS_ARGS_DIR/line$n.args" ]; do n=$((n+1)); done
  printf '%s\n' "$name" "$@" > "$CENSUS_ARGS_DIR/line$n.args"
fi
case "$arch" in
  aarch64) real=/usr/bin/ld.bfd ;;
  x86_64) real=/usr/bin/x86_64-linux-gnu-ld.bfd ;;
  i686) real=/usr/bin/i686-linux-gnu-ld.bfd ;;
esac
exec "$real" "$@"
