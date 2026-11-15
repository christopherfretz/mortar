# The lane's link timer (tests/mason/README.md "Link time"), sourced by
# each `ld` shim. `mason_timed <linker> <name> <args>...` runs the linker
# and, with $MASON_TIMES set, appends one line to it:
#   <cwd> TAB <name> TAB <status> TAB <wall-us> TAB <peak-rss-kib>
# wall time from bash's $EPOCHREALTIME around GNU time (whose fork is in
# every linker's figure alike), peak RSS from GNU time's %M.
mason_timed() {
  local linker=$1 name=$2; shift 2
  if [ -z "${MASON_TIMES:-}" ]; then "$linker" "$@"; return; fi
  local out=a.out prev= a
  for a in "$@"; do [ "$prev" = -o ] && out=$a; prev=$a; done
  local t0=$EPOCHREALTIME st rss
  /usr/bin/time -f '%M' -o "$out.ldrss" "$linker" "$@"; st=$?
  local t1=$EPOCHREALTIME
  rss=$(tail -n 1 "$out.ldrss" 2>/dev/null); rm -f "$out.ldrss"
  local us=$(( (${t1/./} - ${t0/./}) ))
  printf '%s\t%s\t%s\t%s\t%s\n' "$PWD" "$name" "$st" "$us" "${rss:--}" >> "$MASON_TIMES"
  return $st
}
