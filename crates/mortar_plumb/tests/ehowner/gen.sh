#!/bin/sh
# Regenerates WP-201's fixtures of tests/ehowner.rs (spec/plumb.md §5.7b,
# resolve.md §6.3): fields of .gcc_except_table and .eh_frame that refer
# to the local DW.ref.pp of a dropped copy of the COMDAT group DW.ref.pp
# (a.o's copy is kept), judged by the record that owns them. AArch64,
# -nostdlib, static; each case is linked by GNU ld, lld, mold and mortar,
# and the outputs of the links that succeed are kept with their maps and
# words (`.line`; mortar's map holds its own):
#   dead    g's FDE (a dropped copy) owns the word: dead
#   live    f2's FDE owns it: live
#   cie     f's live FDE uses a CIE whose personality names DW.ref.pp
#   xfile   x.o's live FDE reads its LSDA in tb.o's table, after the start
#           of tb.o's dead FDE (g): live, across files
#   x8      xfile with fx8's CIE and FDE in the 64-bit DWARF form
#   shared  a dead FDE (g) and a live one (f3) at one start: live
#   none    the word is before the table's first start: no owner
#   drop    the only FDE whose LSDA is in the table is in an .eh_frame of
#           g's dropped copy: no FDE the link reads owns the word
#   sh gen.sh <mortar>
# Run from this directory. First made with the mortar of WP-201's base
# (int/batch-12, 953f3bee).
set -e
ulimit -c 0
mortar=$1
for s in start a dead live cie x tb x8 shared none drop; do
  as $s.s -o $s.o
done
link() {
  name=$1; shift
  objs="$*"
  for l in bfd lld mold mortar; do
    case $l in
      bfd) prog=ld.bfd; map=-Map=$name.$l.map;;
      lld) prog=ld.lld-19; map=-Map=$name.$l.map;;
      mold) prog=mold; map=-Map=$name.$l.map;;
      mortar) prog=$mortar; map=--mortar-map=$name.$l.map;;
    esac
    # $objs and $map are split into words on purpose.
    set -- -static -z max-page-size=4096 -o $name.$l start.o a.o $objs $map
    if $prog "$@" 2>gen.err; then
      printf '%s\n' "$@" > $name.$l.line
      echo "$name: $l links"
    else
      # A refused link's output and map (if any) are left for the
      # caller to delete by name; none is a fixture.
      echo "$name: $l refuses: $(head -1 gen.err)"
    fi
  done
}
link dead dead.o
link live live.o
link cie cie.o
link xfile x.o tb.o
link x8 x8.o tb.o
link shared shared.o
link none none.o
link drop drop.o
