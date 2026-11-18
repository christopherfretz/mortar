#!/bin/bash
# WP-249 step 1, item 2: GNU ld's "kept section of the same name and size"
# rule, found by experiment, beside lld, mold and mortar. Two hand-written
# objects: k.o (first: keeps every COMDAT group) and d.o (second: its
# copies are dropped). d.o's non-group .debug_macro (the referrer) starts
# with the marker WP249REF and holds one word per case against its own
# group member; every member starts with an 8-byte tag. words.py reads the
# linked referrer's words and names each as kept+N (the kept copy's tag
# offset in the output section, plus N), all ones, 0, or raw.
#   [KREF=1] rule.sh OUT a64|x64 [MORTAR]   last line RULE DONE
ulimit -c 0
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$1"; T="$2"; MORTAR="${3:-/Volumes/AgentWork/mortar-helper/.build/target-wp-143/release/mortar}"
mkdir -p "$OUT/$T${KREF:+-kref}"; W="$(cd "$OUT/$T${KREF:+-kref}" && pwd)"
case $T in
  a64) AS=as; LD=ld.bfd; EM=aarch64linux; MOLDM=aarch64linux;;
  x64) AS=x86_64-linux-gnu-as; LD=x86_64-linux-gnu-ld.bfd; EM=elf_x86_64; MOLDM=elf_x86_64;;
  *) echo "target a64|x64"; exit 2;;
esac
# KREF=1: k.o also refers to each of its own copies (from a non-group
# .debug_info), as a GCC unit refers to the groups it imports: the
# assembler then gives each copy a section symbol in k.o.
KREFS="  .4byte .Lk_c1\n  .4byte .Lk_c2\n  .4byte .Lk_c3\n  .4byte .Lk_c4\n  .4byte .Lk_c5\n  .4byte .Lk_c7\n  .4byte .Lk_c8\n  .4byte .Lk_c10\n  .4byte .Lk_c11\n  .4byte .Lk_wm4_c14\n  .4byte .Lk_c13"
# grp NAME SIG FLAGS: a group member section (FLAGS: comdat, or empty)
grp() { echo "  .section $1,\"G\",@progbits,$2${3:+,$3}"; }
cat > "$W/k.s" <<EOF
  .text
  .globl _start
_start:
  .4byte 0
  /* pads: no tagged copy lies at offset 0 of its output section */
  .section .debug_macro,"",@progbits
  .ascii "PADPADPAD......."
  .section .debug_types,"",@progbits
  .ascii "PADPADPAD......."
  .section .debug_line,"",@progbits
  .ascii "PADPADPAD......."
$(grp .debug_macro c1 comdat)
.Lk_c1:
  .ascii "C1tag..."
  .8byte 0
$(grp .debug_macro c2 comdat)
.Lk_c2:
  .ascii "C2tag..."
  .8byte 0
$(grp .debug_macro c3 comdat)
.Lk_c3:
  .ascii "C3keep.."
  .8byte 0
$(grp .debug_types c4 comdat)
.Lk_c4:
  .ascii "C4types."
  .8byte 0
$(grp .debug_macro c5 "")
.Lk_c5:
  .ascii "C5keep.."
  .8byte 0
$(grp .debug_macro c7 comdat)
.Lk_c7:
  .ascii "C7tag..."
  .8byte 0
  .globl c7_global
c7_global:
  .8byte 0
$(grp .debug_macro c8 comdat)
.Lk_c8:
  .ascii "C8tag..."
  .8byte 0
c8_local:
  .8byte 0
$(grp .debug_line c10 comdat)
.Lk_c10:
  .ascii "C10tag.."
  .8byte 0
$(grp .debug_macro c11 comdat)
.Lk_c11:
  .ascii "C11tag.."
  .4byte .Lks
  .4byte 0
$(grp .debug_macro wm4.c14 comdat)
.Lk_wm4_c14:
  .ascii "C14tag.."
  .8byte 0
$(grp .debug_macro c13 comdat)
.Lk_c13:
  .ascii "C13tag.."
c13_k:
  .8byte 0
$( [ -n "$KREF" ] && printf '  .section .debug_info,"",@progbits\n%b\n' "$KREFS" )
  .section .debug_str,"",@progbits
.Lks:
  .asciz "wp249"
EOF
cat > "$W/d.s" <<EOF
$(grp .debug_macro c1 comdat)
.Lc1:
  .ascii "C1tag..."
  .8byte 0
$(grp .debug_macro c2 comdat)
.Lc2:
  .ascii "C2tag..."
  .8byte 0
  .8byte 0
$(grp .debug_macro c3 comdat)
.Lc3:
  .ascii "C3drop.."
  .8byte 0
$(grp .debug_macro c4 comdat)
.Lc4:
  .ascii "C4macro."
  .8byte 0
$(grp .debug_macro c5 "")
.Lc5:
  .ascii "C5drop.."
  .8byte 0
$(grp .debug_macro c7 comdat)
  .ascii "C7tag..."
  .8byte 0
  .globl c7_global
c7_global:
  .8byte 0
$(grp .debug_macro c8 comdat)
  .ascii "C8tag..."
  .8byte 0
c8_local:
  .8byte 0
$(grp .debug_line c10 comdat)
.Lc10:
  .ascii "C10tag.."
  .8byte 0
$(grp .debug_macro c11 comdat)
.Lc11:
  .ascii "C11tag.."
  .4byte .Lds
  .4byte 0
$(grp .debug_macro wm4.c14 comdat)
.Lc14:
  .ascii "C14drop."
  .8byte 0
$(grp .debug_macro c13 comdat)
.Lc13:
  .ascii "C13tag.."
c13_d:
  .8byte 0
  .section .debug_str,"",@progbits
.Lds:
  .asciz "wp249"
  .section .debug_macro,"",@progbits
  .ascii "WP249REF"
  .4byte .Lc1            /* C1 same name, same size, same bytes */
  .4byte .Lc1+4          /* C1+4 the same with an addend */
  .4byte .Lc2            /* C2 same name, other size */
  .4byte .Lc3            /* C3 same name and size, other bytes */
  .4byte .Lc4            /* C4 the kept group has no member of this name */
  .4byte .Lc5            /* C5 a group without GRP_COMDAT */
  .4byte c7_global+0     /* C7 a global symbol in the member (resolves to the kept copy's) */
  .4byte c8_local        /* C8 a local named symbol in the member */
  .8byte .Lc1            /* C9 a 64-bit word, same as C1 */
  .4byte .Lc10           /* C10 a .debug_line group member */
  .4byte .Lc11           /* C11 a member with its own relocation (a .rela member in the group) */
  .4byte .Lc13           /* C13 a member whose local named symbols differ (c13_k, c13_d) */
  .4byte .Lc14           /* C14 C3 (other bytes, same size) under a GCC-style signature wm4.* */
  .section .debug_info,"",@progbits
  .ascii "WP249REF"
  .4byte .Lc1            /* I1 another referring name, same as C1 */
EOF
$AS "$W/k.s" -o "$W/k.o" && $AS "$W/d.s" -o "$W/d.o" || { echo "assembly failed"; exit 1; }
python3 -I "$HERE/classes.py" groups "$W/k.o" "$W/d.o" > "$W/groups.txt"
: > "$W/link.log"
lk() { # tag command...
  local t=$1; shift
  "$@" > "$W/$t.err" 2>&1; echo "$t rc=$? $(head -c 300 "$W/$t.err" | tr '\n' ' ')" >> "$W/link.log"
}
lk gnu $LD -m $EM -static -o "$W/gnu" "$W/k.o" "$W/d.o"
lk lld19 ld.lld-19 -m $EM -static -o "$W/lld19" "$W/k.o" "$W/d.o"
lk lld22 ld.lld-22 -m $EM -static -o "$W/lld22" "$W/k.o" "$W/d.o"
lk mold mold -m $MOLDM -static -o "$W/mold" "$W/k.o" "$W/d.o"
lk mortar "$MORTAR" -m $EM -static -o "$W/mortar" "$W/k.o" "$W/d.o"
cat "$W/link.log"
for t in gnu lld19 lld22 mold mortar; do
  [ -f "$W/$t" ] || { echo "$T $t: no output"; continue; }
  python3 -I "$HERE/words.py" "$W/$t" "$T $t" \
    .debug_macro C1,C1+4,C2,C3,C4,C5,C7,C8,C9:8,C10,C11,C13,C14 .debug_info I1
done
echo "RULE DONE"
