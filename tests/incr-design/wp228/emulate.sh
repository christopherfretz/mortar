#!/bin/bash
# emulate.sh OUTDIR [CONFIG...]: WP-228 step 1, item 6. For each compiler
# configuration (default: every AArch64 one; x64 ones are static checks
# unless DEBUGGERS_X64=1: then under qemu-x86_64's gdb stub) and DWARF 4 and 5: compiles src/{a,b,b2,c}.cc,
# makes bold.o (b.o with every defined global, weak and group signature
# renamed, so its sections are collected and its debug contributions are
# dead), links the scenarios below in incremental mode with --gc-sections
# by mortar, neutralizes the dead file's contributions with place.py
# (variants), and compares each result with the reference full link of the
# same allocated layout:
#   S1 in place      a b2 c                (b2 in b's allotment: reported)
#   S2 dead reused   a b2 bold c  vs a b2 c  (live, then a dead remainder)
#   S3 tail          a bold c b2  vs a c b2
#   S4 dead unit     a b c bold   vs a b c
#   S5 dead at 0     bold a c b2  vs a c b2
# Checks: readelf --debug-dump warnings, llvm-dwarfdump --verify errors
# (counts equal to the reference's), gdb and lldb transcripts equal to the
# reference's (addresses masked), the verifier (xtask plumb) on the edited
# map (findings 0). EXTRA adds compiler flags (TAGX to the tag). One line per check in OUTDIR/emulate.log; FINAL line
# "EMULATE-DONE runs=N same=N differ=N".
set -u
ulimit -c 0
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$1"; shift
W="$(cd "$(dirname "$OUT")" && pwd)/$(basename "$OUT")"
mkdir -p "$W"
XT="${XTASK:-/Volumes/AgentWork/mortar-helper/.build/target-wp-154/debug/xtask}"
BIN="${MORTAR_BIN_DIR:-/Volumes/AgentWork/mortar-helper/.build/target-wp-154/wp228/bin}"
KILN="${MASON_KILN:-}"; HOD="${MASON_HOD:-}"
LOG="$W/emulate.log"; : > "$LOG"
CONFIGS="${*:-gcc14-a64 clang19-a64 clang22-a64 kiln-a64}"
runs=0; same=0; differ=0
note() { echo "$*" >> "$LOG"; }
check() { # tag what result(SAME|DIFF) detail
  runs=$((runs+1))
  if [ "$3" = SAME ]; then same=$((same+1)); else differ=$((differ+1)); fi
  note "$1 $2 $3 $4"
}
cc_of() {
  case $1 in
    gcc14-a64) echo "g++";;
    gcc14-x64) echo "x86_64-linux-gnu-g++";;
    clang19-a64) echo "clang++-19";;
    clang19-x64) echo "clang++-19 --target=x86_64-linux-gnu";;
    clang22-a64) echo "clang++-22";;
    clang22-x64) echo "clang++-22 --target=x86_64-linux-gnu";;
    kiln-a64) echo "env KILN_HOD=$HOD $KILN -fkiln-backend=hod --target=aarch64-linux-gnu";;
    kiln-x64) echo "env KILN_HOD=$HOD $KILN -fkiln-backend=hod --target=x86_64-linux-gnu";;
  esac
}
ld_of() { case $1 in *-x64) echo "x86_64-linux-gnu-g++";; *) echo "g++";; esac; }
mask() { sed -e 's/0x[0-9a-f]*/ADDR/g' -e 's/[Pp]rocess [0-9]*/process N/' \
  -e 's#/[^ ]*/\(s[0-9a-z]*\|ref[0-9a-z]*\)\b#X#g' -e 's/\b\(s[1-5][a-z]*\|ref[0-9]\)\b/X/g' \
  -e 's/`[a-z_]*`/`X`/' -e '/^(lldb) quit/d' -e '/Current executable set to/d' \
  -e 's/gdb-remote [0-9]*/gdb-remote PORT/' -e 's/^\([ri]\) = -\{0,1\}[0-9]*$/\1 = N/' -e 's/^\((int) [ri]\) = -\{0,1\}[0-9]*$/\1 = N/'; }
debuggers() { # exe src line -> exe.gdb exe.lldb
  local x=$1 src=$2 line=$3
  cat > "$x.g" <<EOF
set pagination off
set confirm off
break b_f
run
bt
info line
print g_b
print g_msg
ptype BS
next
next
print s
info locals
step
bt
finish
info line $src:$line
info scope b_f
info functions b_f
quit
EOF
  cat > "$x.l" <<EOF
breakpoint set -n b_f
run
bt
frame variable
target variable g_b g_msg
image lookup -t BS
thread step-over
thread step-over
frame variable
thread step-in
bt
finish
image lookup -n b_f
breakpoint set -f $src -l $line
breakpoint list
quit
EOF
  if [ "${QEMU:-0}" = 1 ]; then
    # x86-64 on this AArch64 host: the program under qemu-x86_64's gdb
    # stub, gdb-multiarch and lldb's gdb-remote attached to it
    local port
    port=$((20000 + RANDOM % 20000))
    sed -i "s/^run$/target remote :$port\ncontinue/; s/^quit$/kill\nquit/" "$x.g"
    (timeout 120 qemu-x86_64 -g $port "./$x" > /dev/null 2>&1 &)
    sleep 1
    timeout 60 gdb-multiarch -batch -nx -x "$x.g" "./$x" 2>&1 | mask | grep -v -E '^\[Inferior|^Remote debugging|^Reading|^warning: (remote|Remote)' > "$x.gdb"
    port=$((20000 + RANDOM % 20000))
    sed -i "s/^run$/gdb-remote $port\nprocess continue/; s/^quit$/process kill\nquit/" "$x.l"
    (timeout 120 qemu-x86_64 -g $port "./$x" > /dev/null 2>&1 &)
    sleep 1
    timeout 60 lldb -b -O "target create ./$x" -s "$x.l" 2>&1 | mask | grep -v -E 'failed to send k packet|exited with status' > "$x.lldb"
  else
    timeout 60 gdb -batch -nx -x "$x.g" "./$x" 2>&1 | mask > "$x.gdb"
    timeout 60 lldb -b -s "$x.l" "./$x" 2>&1 | mask > "$x.lldb"
  fi
  rm -f "$x.g" "$x.l"
}
static_checks() { # exe -> exe.rw (readelf warnings), exe.vf (verify errors)
  local x=$1
  readelf --debug-dump=info,abbrev,line,ranges,aranges,loc,str-offsets,addr,frames,macro,rawline "$x" > "$x.rd" 2>&1
  grep -E 'Warning|warning:|Error|Corrupt' "$x.rd" | sed -e 's/0x[0-9a-f]*/ADDR/g' -e 's/[0-9][0-9]*/N/g' | sort | uniq -c > "$x.rw"
  rm -f "$x.rd"
  llvm-dwarfdump-22 --verify "$x" > "$x.vd" 2>&1
  grep -E '^error:|error:' "$x.vd" | sed -e 's/0x[0-9a-f]*/ADDR/g' | sort | uniq -c > "$x.vf"
  rm -f "$x.vd"
}
cmp_files() { # tag what a b
  if cmp -s "$3" "$4"; then check "$1" "$2" SAME "-"
  else check "$1" "$2" DIFF "$(diff "$3" "$4" | head -6 | tr '\n' '|')"; fi
}
for c in $CONFIGS; do
  cc=$(cc_of "$c"); ld=$(ld_of "$c")
  for dw in ${DWS:-4 5}; do
    tag="$c-dw$dw${TAGX:-}"
    D="$W/$tag"; mkdir -p "$D"; cd "$D" || exit 1
    cp "$HERE"/src/*.cc "$HERE"/src/h.h .
    F="-O0 -g -gdwarf-$dw -ffunction-sections -fdata-sections ${EXTRA:-}"
    ok=1
    for s in a b b2 c; do $cc $F -c $s.cc -o $s.o 2>> build.err || ok=0; done
    if [ $ok = 0 ]; then note "$tag compile FAIL $(head -2 build.err | tr '\n' '|')"; continue; fi
    case $c in *-x64) OC=x86_64-linux-gnu-objcopy; NM=x86_64-linux-gnu-nm;; *) OC=objcopy; NM=nm;; esac
    rn=""
    for sym in $($NM --defined-only b.o | awk '$2 ~ /[A-Z]/ {print $3}'); do rn="$rn --redefine-sym $sym=old_$sym"; done
    # shellcheck disable=SC2086
    $OC $rn b.o bold.o || { note "$tag objcopy FAIL"; continue; }
    L() { local name=$1; shift
      $ld -static -B"$BIN" -Wl,--build-id=none,--gc-sections,--incremental,--incremental-state="$D/$name.state",--mortar-map="$D/$name.map" "$@" -o "$name" 2>> link.err
      local rc=$?; rm -f "$name.state"; return $rc; }
    L ref1 a.o b2.o c.o && L ref3 a.o c.o b2.o && L ref4 a.o b.o c.o \
      && L s2 a.o b2.o bold.o c.o && L s3 a.o bold.o c.o b2.o && L s4 a.o b.o c.o bold.o \
      && L s5 bold.o a.o c.o b2.o || { note "$tag link FAIL $(tail -2 link.err | tr '\n' '|')"; continue; }
    L refb a.o b.o c.o
    # S1: b2's debug contributions against b's allotments
    for sec in .debug_info .debug_abbrev .debug_line .debug_rnglists .debug_ranges .debug_addr .debug_str_offsets .debug_loclists; do
      o=$(awk -v s=$sec '$1=="nplace" && $2==4 && $4==s {print $5" "$6}' refb.map | head -1)
      n=$(awk -v s=$sec '$1=="nplace" && $2==4 && $4==s {print $5" "$6}' ref1.map | head -1)
      [ -n "$o$n" ] && note "$tag S1 $sec old(off size)=$o new=$n"
    done
    rm -f refb refb.map
    nfile() { awk '$1=="file" && $3=="bold.o" {print $2}' "$1"; }
    for ref in ref1 ref3 ref4; do static_checks $ref; done
    QEMU=0; [ "${c##*-}" = x64 ] && QEMU=1
    if [ "${c##*-}" = a64 ] || [ "${DEBUGGERS_X64:-0}" = 1 ]; then
      debuggers ref1 b2.cc 12
      debuggers ref3 b2.cc 12; debuggers ref4 b.cc 9
      dbg=1
    else dbg=0; fi
    "$XT" plumb ref1 --map ref1.map --map-format mortar > ref1.plumb 2>&1
    note "$tag ref1 plumb $(tail -1 ref1.plumb)"
    variants="plain pieces perunit"
    case $c-$dw in clang*-5|kiln*-5) variants="$variants addrbase";; esac
    for s in s2 s3 s4 s5; do
      case $s in s2) ref=ref1;; s3|s5) ref=ref3;; s4) ref=ref4;; esac
      case $s in s4) src=b.cc; line=9;; *) src=b2.cc; line=12;; esac
      fn=$(nfile $s.map)
      # the unpadded emulation (the dead contributions as compiled, IN-53)
      static_checks $s
      cmp_files "$tag $s" "raw-verify" $ref.vf $s.vf
      for v in $variants; do
        x=${s}$v
        case $v in plain) o="";; pieces) o="--pieces";; perunit) o="--per-unit";; addrbase) o="--addr-base";; esac
        # shellcheck disable=SC2086
        python3 -I "$HERE/place.py" $s $s.map $x $x.map --dead=$fn $o > $x.place 2>&1 || { check "$tag $x" place DIFF "$(tail -1 $x.place)"; continue; }
        static_checks $x
        cmp_files "$tag $x" readelf $ref.rw $x.rw
        cmp_files "$tag $x" verify $ref.vf $x.vf
        if [ $dbg = 1 ]; then
          debuggers $x $src $line
          sed -i "s/\b$x\b/X/g" $x.gdb $x.lldb
          cmp_files "$tag $x" gdb $ref.gdb $x.gdb
          cmp_files "$tag $x" lldb $ref.lldb $x.lldb
        fi
        "$XT" plumb $x --map $x.map --map-format mortar > $x.plumb 2>&1
        # AArch64 mapping symbols ($d, Clang) of the old file's debug
        # sections stay in the emulation's .symtab, inside the dead ranges
        # (P-I3); the update's symbol table has none of the old version's
        # symbols (U-5). Those findings are counted apart.
        nd=$(grep -c "verifier rule P-I3: .*symbol '\$d'" $x.plumb)
        other=$(grep 'mortar-plumb: error' $x.plumb | grep -vc "verifier rule P-I3: .*symbol '\$d'")
        if grep -q '^findings: ' $x.plumb && [ "$other" = 0 ]; then check "$tag $x" plumb SAME "mapping-symbol-findings=$nd"
        else check "$tag $x" plumb DIFF "$(grep 'mortar-plumb: error' $x.plumb | grep -v "symbol '\$d'" | head -4 | tr '\n' '|')"; fi
        rm -f $x
      done
      rm -f $s
    done
    rm -f ref1 ref3 ref4
  done
done
echo "EMULATE-DONE runs=$runs same=$same differ=$differ" >> "$LOG"
echo "EMULATE-DONE runs=$runs same=$same differ=$differ"
