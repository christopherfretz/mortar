#!/bin/bash
# options/run.sh WORK: the option tail of the census with -r, per reference
# (GNU ld, lld 22, mold): accepted or refused, and the effect on the output
# (section list, symbol table, a few counts) against the plain -r output.
set -u; ulimit -c 0
WORK=$(realpath -m "$1"); HERE=$(cd "$(dirname "$0")" && pwd); T=$HERE/../tools
mkdir -p "$WORK"; cd "$WORK" || exit 1
cat > o1.c <<'X'
#include <stdio.h>
static int lstat1(int x) { return x + 1; }
int keep_me(int x) { return lstat1(x); }
int drop_me(int x) { return x * 3; }
__attribute__((visibility("hidden"))) int hid1(void) { return 1; }
int wrapped(void) { return 10; }
int call_wrapped(void) { return wrapped(); }
int ucommon;
const char *s1(void) { return "abc"; }
X
cat > o2.c <<'X'
int keep_me(int);
int hid1(void);
int o2_entry(void) { return keep_me(1) + hid1(); }
const char *s2(void) { return "abc"; }
X
gcc -g -O1 -fcommon -ffunction-sections -fdata-sections -c o1.c -o o1.o
gcc -g -O1 -fcommon -ffunction-sections -fdata-sections -c o2.c -o o2.o
printf 'V1 { global: keep_me; local: *; };\n' > vs.map
printf 'SECTIONS { .text : { *(.text .text.*) } .mydata : { *(.data .data.*) } }\n' > merge.ld
printf 'hello blob\n' > blob.txt
ar rcs libo.a o1.o o2.o; ar rcsT libthin.a o1.o o2.o
gcc -shared -fPIC -o libdso.so o2.c
desc() { # a compact description of an -r output
  local f=$1
  echo -n "secs=$(readelf -SW $f | grep -c '^ *\[') rel=$(readelf -rW $f | grep -c 'R_') syms=$(readelf -sW $f | awk 'NR>3' | wc -l) dbg=$(readelf -SW $f | grep -c '\.debug_') "
  echo -n "local:$(readelf -sW $f | awk 'NR>3 && $5=="LOCAL" && $4!="SECTION" && $4!="FILE"' | wc -l) "
  echo -n "drop_me:$(readelf -sW $f | awk '$8=="drop_me"{print $5"/"$7}') hid1:$(readelf -sW $f | awk '$8=="hid1"{print $5"/"$6"/"$7}') keep_me:$(readelf -sW $f | awk '$8=="keep_me"{print $5"/"$6"/"$7}') ucommon:$(readelf -sW $f | awk '$8=="ucommon"{print $7}') "
  echo -n "wrap:$(readelf -sW $f | awk '$8 ~ /wrap/{printf "%s/%s,", $8, $7}') "
  echo -n "extra:$(readelf -SW $f | awk '/build-id|gnu.hash|dynsym|interp|eh_frame_hdr|COMMON|\.zdebug|mydata/{for(i=1;i<=NF;i++) if ($i ~ /^\./) {printf "%s,", $i; break}}') "
  echo -n "compressed:$(readelf -SW $f | awk '$0 ~ / C / || $9 ~ /C/' | wc -l) "
}
LINES=(
 "plain|"
 "-s|-s" "-S|-S" "--strip-debug|--strip-debug" "-x|-x" "-X|-X" "--discard-locals|--discard-locals"
 "-d|-d" "-dc|-dc" "-dp|-dp" "--sort-common|--sort-common"
 "--gc-sections|--gc-sections" "--gc-sections -e o2_entry|--gc-sections -e o2_entry" "--gc-sections -u keep_me|--gc-sections -u keep_me"
 "--unique|--unique" "--unique=.text.*|--unique=.text.*" "--build-id|--build-id" "--build-id=sha1|--build-id=sha1"
 "--compress-debug-sections=zlib|--compress-debug-sections=zlib" "--emit-relocs|--emit-relocs" "-q|-q"
 "-z noexecstack|-z noexecstack" "-z relro|-z relro" "-z now|-z now" "-z separate-code|-z separate-code" "-z max-page-size=65536|-z max-page-size=65536"
 "--version-script|--version-script=vs.map" "--wrap=wrapped|--wrap=wrapped" "--defsym=newsym=keep_me|--defsym=newsym=keep_me"
 "--exclude-libs=ALL|--exclude-libs=ALL" "--eh-frame-hdr|--eh-frame-hdr" "--hash-style=gnu|--hash-style=gnu" "--as-needed|--as-needed"
 "-dynamic-linker|-dynamic-linker /lib/ld-linux-aarch64.so.1" "--fix-cortex-a53-843419|--fix-cortex-a53-843419" "-EL|-EL" "-m aarch64linux|-m aarch64linux" "-maarch64linux|-maarch64linux"
 "-pie|-pie" "-no-pie|-no-pie" "-shared|-shared" "-static|-static" "-Bstatic|-Bstatic" "-nostdlib|-nostdlib" "-O1|-O1" "--icf=all|--icf=all"
 "-T merge.ld|-T merge.ld" "--relax|--relax" "--no-relax|--no-relax" "-Map|-Map=MAPFILE" "--threads|--threads=2"
 "--start-group|--start-group" "--whole-archive|--whole-archive" "--no-undefined|--no-undefined" "-z defs|-z defs" "--warn-common|--warn-common"
)
INPUTS="o1.o o2.o"
for ent in "${LINES[@]}"; do
  name=${ent%%|*}; opt=${ent#*|}
  for k in bfd lld mold; do
    o=$WORK/out.$(echo "$name" | tr -c 'A-Za-z0-9' _).$k.o
    o2=${opt//MAPFILE/$o.map}
    case $name in --start-group) args="--start-group $INPUTS --end-group";; --whole-archive) args="--whole-archive libthin.a --no-whole-archive";; *) args="$o2 $INPUTS";; esac
    case $k in bfd) L=ld.bfd;; lld) L=ld.lld-22;; mold) L=mold;; esac
    msg=$($L -r -o $o $args 2>&1); rc=$?
    if [ $rc = 0 ] && [ -f $o ]; then d=$(desc $o); else d="REFUSED"; fi
    echo "$name | $k | rc=$rc | $d | $(echo "$msg" | grep -v '^$' | head -2 | tr '\n' ' ' | cut -c1-200)"
  done
done
# archives, shared objects, binary
for k in bfd lld mold; do
  case $k in bfd) L=ld.bfd;; lld) L=ld.lld-22;; mold) L=mold;; esac
  for c in "archive-only|libo.a" "archive-u|-u keep_me libo.a" "archive-after-ref|o2.o libo.a" "thin-whole|--whole-archive libthin.a" "group|--start-group o2.o libo.a --end-group" "dso|o1.o libdso.so" "dso-l|o1.o -L. -ldso" "empty|/dev/null" "binary|-b binary blob.txt" "format-binary|--format=binary blob.txt -b default o1.o" "binary-m|-m aarch64linux -b binary blob.txt"; do
    name=${c%%|*}; args=${c#*|}
    o=$WORK/in.$name.$k.o
    msg=$($L -r -o $o $args 2>&1); rc=$?
    if [ $rc = 0 ] && [ -f $o ]; then d="secs:$(python3 -I $T/secs.py $o '^[^.]|^\.[a-z]' | tr ' ' '\n' | grep -v 'symtab\|strtab' | tr '\n' ' ') syms:$(readelf -sW $o | awk 'NR>3 && $4!="SECTION" && $4!="FILE" {printf "%s/%s/%s ", $8, $5, $7}' | cut -c1-300) machine=$(readelf -hW $o | awk '/Machine/{print $2}') flags=$(readelf -hW $o | awk '/^  Flags/{print $2}')"; else d=REFUSED; fi
    echo "input:$name | $k | rc=$rc | $d | $(echo "$msg" | grep -v '^$' | head -2 | tr '\n' ' ' | cut -c1-200)"
  done
done
