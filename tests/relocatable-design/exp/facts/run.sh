#!/bin/bash
# facts/run.sh WORK: item B of WP-154, cell by cell: small inputs, each partially
# linked by GNU ld 2.44, lld 22 and mold 2.37.1, the output described in one line
# per reference. Sections are listed as name(type,flags,align,size).
set -u; ulimit -c 0
WORK=$(realpath -m "$1"); HERE=$(cd "$(dirname "$0")" && pwd); T=$HERE/../tools
mkdir -p "$WORK"; cd "$WORK" || exit 1
R() { # R ARCH REF OUT ARGS... : one -r link
  local a=$1 k=$2 o=$3; shift 3
  case $k in
    bfd) case $a in aarch64) ld.bfd -r -o "$o" "$@";; *) $a-linux-gnu-ld.bfd -r -o "$o" "$@";; esac;;
    lld) ld.lld-22 -r -o "$o" "$@";;
    mold) mold -r -o "$o" "$@";;
  esac
}
secs() { python3 -I "$T/secs.py" "$1" "${2:-.}"; }
syms() { readelf -sW "$1" | awk -v pat="$2" 'NR>3 && $8 ~ pat {printf "%s:%s/%s/%s/%s/%s ", $8,$4,$5,$6,$7,$3}'; }
secs_p() { local p=$1; shift; secs "$1" "$p"; }
syms_p() { local p=$1; shift; syms "$1" "$p"; }
each() { # each CASE ARCH OUTPUT-DESCRIBER INPUTS...
  local c=$1 a=$2 d=$3; shift 3
  for k in bfd lld mold; do
    local o=$c.$k.o msg
    msg=$(R $a $k $o "$@" 2>&1); local rc=$?
    if [ $rc != 0 ] || [ ! -f $o ]; then echo "$c | $k | rc=$rc $(echo "$msg" | head -2 | tr '\n' ' ' | cut -c1-220)"; continue; fi
    echo "$c | $k | $(eval "$d $o") $( [ -n "$msg" ] && echo "[diag: $(echo "$msg" | head -2 | tr '\n' ' ' | cut -c1-200)]")"
  done
}
CC=gcc
# ---- 1. sections: same name different flags, alignment, order of pieces
cat > s1.s <<'X'
  .section .foo,"a",%progbits
  .balign 4
  .word 1
  .section .bar,"aw",%progbits
  .balign 16
  .word 0x11
X
cat > s2.s <<'X'
  .section .foo,"aw",%progbits
  .balign 8
  .word 2
  .section .bar,"aw",%progbits
  .balign 4
  .word 0x22
X
as s1.s -o s1.o; as s2.s -o s2.o
each 1-flags-conflict aarch64 'secs_p "foo|bar"' s1.o s2.o
d1() { secs $1 "foo|bar"; echo -n "| .bar bytes: $(objcopy -O binary --only-section=.bar $1 t.$1.bin 2>/dev/null; od -An -tx4 t.$1.bin | tr -s ' ')"; }
each 1-order-align aarch64 d1 s1.o s2.o
# ---- 2. -Ur: init_array priorities
printf '__attribute__((constructor(200))) static void a(void){}\n__attribute__((constructor)) static void b(void){}\n' > p1.c
printf '__attribute__((constructor(101))) static void c(void){}\n' > p2.c
$CC -c p1.c -o p1.o; $CC -c p2.c -o p2.o
each 2-init-array aarch64 'secs_p init_array' p1.o p2.o
for k in bfd lld mold; do case $k in bfd) ld.bfd -Ur -o 2u.$k.o p1.o p2.o;; lld) ld.lld-22 -Ur -o 2u.$k.o p1.o p2.o 2>&1 | head -1;; mold) mold -Ur -o 2u.$k.o p1.o p2.o 2>&1 | head -1;; esac; echo "2-Ur | $k | $( [ -f 2u.$k.o ] && secs 2u.$k.o init_array)"; done
# ---- 3. COMDAT groups
printf 'inline int f(int x){static int k; return k += x;}\nint g1(int x){return f(x);}\n' > g1.cc
printf 'inline int f(int x){static int k; return k += x;}\nint g2(int x){return f(x)+1;}\n' > g2.cc
g++ -O0 -c g1.cc -o g1.o; g++ -O0 -c g2.cc -o g2.o
dg() { echo -n "groups: $(readelf -gW $1 | awk '/COMDAT group section/{gsub(/[\[\]`]/,""); printf "%s ", $NF}') sigsym: $(syms $1 '^_Z1fi$|_ZZ1fiE1k$')"; }
each 3-comdat aarch64 dg g1.o g2.o
# a group whose signature is a section symbol? and non-COMDAT group (flag 0)
cat > g3.s <<'X'
  .section .text.gx,"axG",%progbits,gx_sig
  .globl gx_sig
gx_sig: ret
  .section .data.ng,"awG",%progbits,ng_sig
  .word 7
X
sed 's/^gx_sig:/gx_sig2:/; s/globl gx_sig/globl gx_sig2/' g3.s > g4.s
as g3.s -o g3.o; as g4.s -o g4.o
dg2() { readelf -gW $1 | grep -E "group section|^ +\[" | tr -s ' ' | tr '\n' ' '; }
each 3-nocomdat-group aarch64 dg2 g3.o g4.o
# ---- 4. notes, retain, link-order, exclude, addrsig, comment, warning, linkonce
cat > n1.s <<'X'
  .section .rt,"axR",%progbits
  .word 1
  .section .lo,"ao",%progbits,.rt
  .word 2
  .section .ex,"e",%progbits
  .word 3
  .section .gnu.warning.wsym,"",%progbits
  .string "wsym is deprecated"
  .text
  .globl wsym
wsym: ret
  .section .gnu.linkonce.t.lk,"ax",%progbits
  .globl lk
lk: ret
  .section .note.GNU-stack,"",%progbits
X
sed -e 's/wsym/wsym2/g' -e 's/lk$/lk2/; s/lk:/lk2:/; s/\.t\.lk/.t.lk2/' n1.s > n2.s; as n1.s -o n1.o; as n2.s -o n2.o
each 4-retain-linkorder-exclude-warning-linkonce aarch64 'secs_p "rt|lo|ex|warning|linkonce|GNU-stack"' n1.o n2.o
printf 'int bti1(void){return 1;}\n' > b1.c; printf 'int bti2(void){return 2;}\n' > b2.c
$CC -mbranch-protection=standard -c b1.c -o b1.o; $CC -mbranch-protection=standard -c b2.c -o b2.o; $CC -mbranch-protection=none -c b2.c -o b0.o
prop() { echo -n "$(secs $1 property) $(readelf -nW $1 2>/dev/null | grep -i 'feature' | tr -s ' ' | tr '\n' ' ')"; }
each 4-gnu-property-both-bti aarch64 prop b1.o b2.o
each 4-gnu-property-one-bti aarch64 prop b1.o b0.o
x86_64-linux-gnu-gcc -fcf-protection=full -c b1.c -o xb1.o; x86_64-linux-gnu-gcc -fcf-protection=none -c b2.c -o xb0.o; x86_64-linux-gnu-gcc -fcf-protection=full -c b2.c -o xb2.o
each 4-gnu-property-x86-both x86_64 prop xb1.o xb2.o
each 4-gnu-property-x86-one x86_64 prop xb1.o xb0.o
clang-22 -c b1.c -o cb1.o; clang-22 -c b2.c -o cb2.o
dac() { secs $1 "addrsig|comment"; echo -n " comment: $(readelf -p .comment $1 2>/dev/null | grep "\]" | tr -s " " | tr "\n" "|")"; }
each 4-addrsig-comment aarch64 dac cb1.o b0.o cb2.o
# ---- 5. symbols
cat > y1.c <<'X'
static int loc1(void) { return 1; }
int glob1(void) { return loc1(); }
__attribute__((visibility("hidden"))) int hidden_def(void) { return 2; }
__attribute__((visibility("protected"))) int prot_def(void) { return 3; }
extern int hidden_ref(void) __attribute__((visibility("hidden")));
int use_hidden_ref(void) { return hidden_ref(); }
int ext_ref(void); int use_ext(void) { return ext_ref(); }
extern int uw(void) __attribute__((weak)); int use_uw(void) { return uw ? 1 : 0; }
__attribute__((weak)) int wdef(void) { return 4; }
int dup_strong(void) { return 5; }
__thread int tls1;
int comm1; 
__asm__(".symver vfoo1, vfoo@V1\n.symver vfoo2, vfoo@@V2");
int vfoo1(void) { return 1; } int vfoo2(void) { return 2; }
extern char __start_mysec[], __stop_mysec[], _end[], __ehdr_start[], _GLOBAL_OFFSET_TABLE_[];
void *ld_defs[] = { __start_mysec, __stop_mysec, _end, __ehdr_start };
__attribute__((section("mysec"), used)) int in_mysec = 1;
X
cat > y2.c <<'X'
int hidden_ref(void) { return 6; }          /* default visibility here, hidden by y1's mention */
int ext_ref2(void);
__attribute__((weak)) int wdef(void) { return 40; }
int wstrong(void);
int comm1 __attribute__((aligned(16)));
long comm2[4];
static int loc1(void) { return 10; } int glob2(void) { return loc1(); }
X
$CC -fcommon -O0 -c y1.c -o y1.o; $CC -fcommon -O0 -c y2.c -o y2.o
dsym() { syms $1 '^(loc1|glob1|hidden_def|prot_def|hidden_ref|ext_ref|uw|wdef|tls1|comm1|comm2|vfoo.*|__start_mysec|__stop_mysec|_end|__ehdr_start|y1.c|y2.c)$'; echo -n " | locals-first sh_info=$(readelf -SW $1 | awk '$2==".symtab"||$3==".symtab"{print $(NF-1)}') nsyms=$(readelf -sW $1 | awk 'NR>3' | wc -l) nsect=$(readelf -sW $1 | awk '$4=="SECTION"' | wc -l) nfile=$(readelf -sW $1 | awk '$4=="FILE"' | wc -l) firstglobal=$(readelf -sW $1 | awk 'NR>3 && $5!="LOCAL"{sub(":","",$1); print $1; exit}')"; }
each 5-symbols aarch64 dsym y1.o y2.o
printf 'int dup_strong(void){return 50;}\n' > y3.c; $CC -c y3.c -o y3.o
each 5-duplicate-strong aarch64 'syms_p dup_strong' y1.o y3.o
printf 'int ifn_impl(void){return 1;}\nstatic void *r(void){return ifn_impl;}\nint ifn(void) __attribute__((ifunc("r")));\n' > yi.c; $CC -c yi.c -o yi.o
printf 'struct S{ static int &get(){ static int v; return v; } }; int use(){ return S::get(); }\n' > yu.cc; g++ -O0 -c yu.cc -o yu.o
each 5-ifunc-unique aarch64 'syms_p "ifn|_ZZN1S3getEvE1v"' yi.o yu.o
# -u / -e at -r
for k in bfd lld mold; do case $k in bfd) ld.bfd -r -u forced_undef -e glob1 -o 5ue.$k.o y1.o 2>&1|head -1;; lld) ld.lld-22 -r -u forced_undef -e glob1 -o 5ue.$k.o y1.o 2>&1|head -1;; mold) mold -r -u forced_undef -e glob1 -o 5ue.$k.o y1.o 2>&1|head -1;; esac; echo "5-u-e | $k | $(syms 5ue.$k.o forced_undef) entry=$(readelf -hW 5ue.$k.o | awk '/Entry/{print $NF}')"; done
# ---- 6. relocations: relaxable forms kept
cat > r1.c <<'X'
extern int ext_var; extern __thread int ext_tls; int ext_fn(void);
int r1(void) { return ext_var + ext_tls + ext_fn(); }
X
x86_64-linux-gnu-gcc -O1 -fPIE -c r1.c -o xr1.o; $CC -O1 -fPIC -c r1.c -o ar1.o; i686-linux-gnu-gcc -O1 -fPIE -c r1.c -o ir1.o
rtypes() { readelf -rW $1 | awk '/R_/{print $3}' | sort | uniq -c | awk '{printf "%s:%s ", $2, $1}'; }
for a in x86_64 aarch64 i686; do case $a in x86_64) i=xr1.o;; aarch64) i=ar1.o;; i686) i=ir1.o;; esac; echo "6-reloc-types-input-$a | - | $(rtypes $i)"; each 6-reloc-types-$a $a rtypes $i; done
clang-22 -O1 -c -Wa,--crel r1.c -o crel.o 2>/dev/null && { echo "6-crel-input | - | $(secs crel.o crel) $(readelf -SW crel.o | grep -c CREL)"; each 6-crel aarch64 'secs_p "rel"' crel.o; }
# ---- 7. file header, mode, determinism
dh() { echo -n "type=$(readelf -hW $1 | awk '/Type:/{print $2}') phnum=$(readelf -hW $1 | awk '/Number of program headers/{print $NF}') entry=$(readelf -hW $1 | awk '/Entry/{print $NF}') flags=$(readelf -hW $1 | awk '/^  Flags/{print $2}') mode=$(stat -c %a $1) shstrndx=$(readelf -hW $1 | awk '/string table index/{print $NF}')/$(readelf -hW $1 | awk '/Number of section headers/{print $NF}')"; }
each 7-header aarch64 dh y1.o y2.o
for k in bfd lld mold; do R aarch64 $k det1.$k.o y1.o y2.o g1.o g2.o; R aarch64 $k det2.$k.o y1.o y2.o g1.o g2.o; echo "7-determinism | $k | $(cmp -s det1.$k.o det2.$k.o && echo identical || echo DIFFERENT)"; done
# ---- 8. REL addend rewrite on i386 (section symbol of a piece that moved)
printf 'static int s1 = 11; int *p1 = &s1;\n' > a1.c; printf 'static int s2 = 22; int *p2 = &s2;\n' > a2.c
i686-linux-gnu-gcc -O1 -c a1.c -o ia1.o; i686-linux-gnu-gcc -O1 -c a2.c -o ia2.o
dadd() { readelf -rW $1 | awk '/R_386/{printf "%s %s %s; ", $1, $3, $5}'; echo -n " .data: $(i686-linux-gnu-objcopy -O binary --only-section=.data $1 t.$1.bin; od -An -tx4 t.$1.bin | tr -s ' ')"; }
echo "8-rel-addend-input | - | ia2: $(dadd ia2.o)"
each 8-rel-addend i686 dadd ia1.o ia2.o
for k in bfd lld mold; do [ -f 8-rel-addend.$k.o ] && echo "8-rcheck | $k | $(python3 -I $T/rcheck.py 8-rel-addend.$k.o ia1.o ia2.o | tr -s ' ' | tr '\n' ';')"; done
