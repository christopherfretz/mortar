# lib.sh -- sourced by the review's scenario scripts (WP-238 step 1
# adversarial review). Needs: PROBE (mortar built with
# --features mortar_live/wp238-probe), W (a scratch directory of the
# caller's). Links are static, --gc-sections, through gcc/clang -B.
ulimit -c 0
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
design=$here/../../incr-design/wp238
mkdir -p "$W/bin"
ln -sf "$PROBE" "$W/bin/ld"

# probe_link DIR CC ARGS... : links in DIR (cwd) with the probe's dump in
# DIR/dump; prints the .rows path on stdout ("" when the link failed).
probe_link() {
  local d=$1 cc=$2; shift 2
  mkdir -p "$d/dump"
  find "$d/dump" -mindepth 1 -delete
  if ! (cd "$d" && MORTAR_WP238=$d/dump MORTAR_WP238_ROWS=1 $cc -B"$W/bin" "$@" > link.txt 2>&1); then
    echo "LINK FAILED in $d: $(head -c 600 "$d/link.txt")" >&2
    echo ""
    return
  fi
  ls "$d"/dump/*.rows 2>/dev/null | head -1
}

# emu OLD NEW [design-emulator args...]: the author's emulate.py and the
# review's design-faithful one.
emu() {
  local o=$1 n=$2; shift 2
  echo "  emulate.py:        $(python3 -I "$design/emulate.py" "$o" "$n" | tr '\n' ' ' | cut -c1-600)"
  echo "  emulate_design.py: $(python3 -I "$here/emulate_design.py" "$o" "$n" "$@" | tr '\n' ' ' | cut -c1-900)"
}

# sec_live ROWS NAME: "file:index:name:kept:live:bits" lines of the
# sections named NAME in a dump.
sec_live() {
  awk -F'\t' -v n="$2" '$1=="F"{f[$2]=$3} $1=="S" && $5==n {print f[$3]":"$4":"$5":kept="$6":live="$7":bits="$8}' "$1"
}
