#!/bin/bash
# WP-245 step 5: a retired GOT slot (`HoleKind::DeadSlot`) against the
# verifier BINARY. The delta layout keeps a retired slot at its index and
# reports a hole over its words; U-6 (the bytes) zeroes them. Nothing yet
# writes such an output, so this script HAND-MAKES the output an update
# would give, from real links:
#   old.o  `main` loads `g` through the GOT (a slot is demanded);
#   new.o  the same bytes and sizes, but `main` takes `g`'s address
#          directly (no GOT demand): x86-64 `mov g@GOTPCREL` -> `lea g`,
#          AArch64 `adrp :got:` + `ldr :got_lo12:` -> `adrp` + `add :lo12:`.
# The update old.o -> new.o retires g's slot: the delta layout keeps every
# address, so the update's output is the old link with (1) main's
# instruction(s) rewritten as new.o's relocations give them, (2) g's slot
# word zeroed, (3) the map naming new.o and carrying `hole .got <off> 8`.
# (1)-(3) are hand-made below (python, from nm/readelf values); the rest
# is mortar's full link of old.o. Cases run through `cargo xtask plumb`:
#   old      the full link of old.o, as linked
#   new      the full link of new.o, as linked
#   dead     the hand-made update (1)+(2)+(3)
#   no-hole  (1)+(2) with the map's hole line left out
#   live     (2)+(3) on the old text: the hole is over a DEMANDED slot
#   split    `dead`, with the map's one `.got` chunk line split in two
#            around the hole (a map form a writer could choose)
# Usage: dead-slot.sh <mortar binary> <scratch dir>  (from the worktree).
set -eu
mortar=$1 out=$2
mkdir -p "$out/bin"
ln -sf "$mortar" "$out/bin/ld"
plumb() { # <case dir> <binary> <map>
  cargo xtask plumb "$2" --map "$3" --map-format mortar > "$1.plumb" 2>&1 || true
  echo "--- $(basename "$1")"
  grep -E '^mortar-plumb|^findings|not judged' "$1.plumb" || true
}
for t in x86_64 aarch64; do
  cc=$t-linux-gnu-gcc d=$out/$t
  mkdir -p "$d"
  if [ "$t" = x86_64 ]; then
    load='movq g@GOTPCREL(%rip), %rax'
    addr='leaq g(%rip), %rax'
    tail='movl (%rax), %eax
  subl $5, %eax
  ret'
  else
    load='adrp x0, :got:g
  ldr x0, [x0, :got_lo12:g]'
    addr='adrp x0, g
  add x0, x0, :lo12:g'
    tail='ldr w0, [x0]
  sub w0, w0, #5
  ret'
  fi
  for v in old new; do
    if [ $v = old ]; then ins=$load; else ins=$addr; fi
    printf '  .text\n  .globl main\n  .type main, %%function\nmain:\n  %s\n  %s\n  .size main, .-main\n  .data\n  .globl g\n  .type g, %%object\n  .p2align 2\ng:\n  .long 5\n  .size g, 4\n  .section .note.GNU-stack,"",%%progbits\n' "$ins" "$tail" > "$d/$v.s"
    $cc -c -Wa,-mrelax-relocations=no "$d/$v.s" -o "$d/$v.o" 2>/dev/null ||
      $cc -c "$d/$v.s" -o "$d/$v.o"
    $cc -static -B"$out/bin/" -Wl,--incremental=full -Wl,--build-id=none \
      -Wl,--mortar-map="$d/$v.map" "$d/$v.o" -o "$d/$v"
  done
  readelf -rW "$d/old.o" | grep -E 'GOT|PC32|PG_HI|LO12' || true
  readelf -rW "$d/new.o" | grep -E 'GOT|PC32|PG_HI|LO12' || true
  main=$(nm "$d/old" | awk '$3=="main"{print $1}')
  g=$(nm "$d/old" | awk '$3=="g"{print $1}')
  got=$(readelf -SW "$d/old" | awk '{for(i=1;i<=NF;i++) if($i==".got"){print $(i+2), $(i+3), $(i+4); exit}}')
  text=$(readelf -SW "$d/old" | awk '{for(i=1;i<=NF;i++) if($i==".text"){print $(i+2), $(i+3); exit}}')
  echo "$t: main=$main g=$g .got(addr off size)=$got .text(addr off)=$text"
  python3 -I - "$t" "$d" "$main" "$g" $got $text <<'PY'
import sys
t, d = sys.argv[1], sys.argv[2]
main, g, got_a, got_o, got_s, tx_a, tx_o = (int(x, 16) for x in sys.argv[3:10])
img = bytearray(open(d + "/old", "rb").read())
fo = lambda a: a - tx_a + tx_o
p = fo(main)
if t == "x86_64":
    assert img[p:p + 3] == b"\x48\x8b\x05", img[p:p + 3].hex()
    disp = int.from_bytes(img[p + 3:p + 7], "little", signed=True)
    slot = main + 7 + disp
    text = bytearray(img)
    text[p + 1] = 0x8D
    text[p + 3:p + 7] = (g - (main + 7)).to_bytes(4, "little", signed=True)
else:
    adrp = int.from_bytes(img[p:p + 4], "little")
    ldr = int.from_bytes(img[p + 4:p + 8], "little")
    assert adrp & 0x9F00001F == 0x90000000, hex(adrp)
    assert ldr & 0xFFC003FF == 0xF9400000, hex(ldr)
    imm = ((adrp >> 29) & 3) | (((adrp >> 5) & 0x7FFFF) << 2)
    if imm & (1 << 20):
        imm -= 1 << 21
    slot = (main & ~0xFFF) + (imm << 12) + ((ldr >> 10) & 0xFFF) * 8
    pages = ((g & ~0xFFF) - (main & ~0xFFF)) >> 12
    new_adrp = 0x90000000 | ((pages & 3) << 29) | (((pages >> 2) & 0x7FFFF) << 5)
    new_add = 0x91000000 | ((g & 0xFFF) << 10)
    text = bytearray(img)
    text[p:p + 4] = new_adrp.to_bytes(4, "little")
    text[p + 4:p + 8] = new_add.to_bytes(4, "little")
assert got_a <= slot < got_a + got_s, (hex(slot), hex(got_a))
so = slot - got_a + got_o
print(f"{t}: g's slot at {slot:#x} (.got+{slot - got_a:#x}) holds "
      f"{int.from_bytes(img[so:so + 8], 'little'):#x}")
for name, buf, zero in (("dead", text, True), ("no-hole", text, True), ("live", img, True)):
    b = bytearray(buf)
    if zero:
        b[so:so + 8] = bytes(8)
    open(f"{d}/{name}", "wb").write(b)
old_o, new_o = d + "/old.o", d + "/new.o"
src = open(d + "/old.map").read().splitlines()
def emit(name, hole, obj, split=False):
    lines = []
    for ln in src:
        ln = ln.replace(old_o, obj)
        f = ln.split()
        if split and f[:3] == ["chunk", "synthetic", ".got"]:
            a, s = int(f[3], 16), int(f[4], 16)
            assert (a, s) == (got_a, got_s), ln
            lines.append(f"chunk synthetic .got {a:x} {slot - a:x}")
            lines.append(f"chunk synthetic .got {slot + 8:x} {a + s - slot - 8:x}")
            continue
        lines.append(ln)
        if ln == "incremental" and hole:
            lines.append(f"hole .got {slot - got_a:x} 8")
    open(f"{d}/{name}.map", "w").write("\n".join(lines) + "\n")
emit("dead", True, new_o)
emit("no-hole", False, new_o)
emit("live", True, old_o)
emit("split", True, new_o, split=True)
open(f"{d}/split", "wb").write(open(f"{d}/dead", "rb").read())
print(f"{t}: the hand-made line: hole .got {slot - got_a:x} 8")
PY
  if [ "$(uname -m)" = "$t" ]; then
    chmod u+x "$d/dead"
    rc=0; "$d/dead" || rc=$?
    echo "$t: the hand-made update runs: exit $rc (0 = g read as 5)"
  fi
  for c in old new dead no-hole live split; do
    plumb "$d/case-$c" "$d/$c" "$d/$c.map"
  done
done
echo "dead-slot: DONE"
