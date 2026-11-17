# Shared helpers for the IN-28a review scripts (sourced).
# Needs: MORTAR (the mortar binary), FLOOR (the incr_floor example).
: "${MORTAR:?set MORTAR}" "${FLOOR:?set FLOOR}"
AS=${AS:-as}
# asm SRC OBJ: assemble, written as a new file renamed over OBJ (a new
# inode, as a compiler writes it).
asm() { $AS "$1" -o "$2.new" && mv -f "$2.new" "$2"; }
# full OUT ARGS...: an incremental-mode full link.
full() { local o=$1; shift; "$MORTAR" "$@" -o "$o" --incremental=full; }
# upd OUT ARGS...: the test path's update (positional unless
# MORTAR_FLOOR_IDENTITY is set); prints its first line.
upd() { local o=$1; shift; "$FLOOR" in-place -- "$@" -o "$o" --incremental ${UPD_EXTRA:-} 2>&1 | head -1; }
# run EXE: its exit status and output.
runx() { local out; out=$(./"$1" 2>&1); echo "exit=$? out=[$out]"; }
same() { if cmp -s "$1" "$2"; then echo "bytes: $1 == $2"; else echo "bytes: $1 != $2 ($(cmp -l "$1" "$2" | wc -l) bytes differ)"; fi; }
# equiv XMAP FMAP X F: the equivalence judge (spec/plumb.md 6.9); its
# last line.  Needs XTASK (the xtask binary, built from this tree).
equiv() { "${XTASK:?set XTASK}" plumb --equiv "$@" > "equiv-$(basename "$3").txt" 2>&1; echo "equiv rc=$? $(tail -1 "equiv-$(basename "$3").txt")"; }
