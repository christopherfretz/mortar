/* Shared object under test (always linked by GNU ld so every exe sees the same input). */
#define _GNU_SOURCE
#include <stdio.h>
#include <string.h>
#include <unistd.h>
int dvar = 42;
extern int dalias __attribute__((weak, alias("dvar")));   /* environ/__environ-style alias */
int darr[4] = {1, 2, 3, 4};
const int ro_obj = 7;                                     /* .rodata in the .so */
const char *const ro_rel = "ro_rel_str";                  /* .data.rel.ro in the .so */
__attribute__((aligned(64))) int al64 = 64;
#ifdef BIG
long long dvbig = 0x1122334455667788LL;                  /* v2: size 8 */
#else
int dvbig = 0x55667788;                                  /* v1: size 4 */
#endif
__thread int tlsx = 5;
__thread int tlsy = 6;
/* oddsym: st_value = 16k+1 in a 16-aligned region; a4sym: 4-aligned (not 8);
   zsym: no .size (st_size 0) */
__asm__(".data\n.balign 16\n.byte 0\n.globl oddsym\n.type oddsym,%object\n.size oddsym,3\n"
        "oddsym: .byte 0x11,0x22,0x33\n.balign 8\n.byte 0,0,0,0\n.globl a4sym\n.type a4sym,%object\n"
        ".size a4sym,4\na4sym: .byte 0x44,0x55,0x66,0x77\n.balign 8\n.globl zsym\n.type zsym,%object\n"
        "zsym: .byte 0x5a,0,0,0,0,0,0,0\n.text\n");
extern char oddsym[3], a4sym[4], zsym[8];
int fa(void) { return 1; }
int fb(void) { return 2; }
int fc(void) { return 3; }
int wdef(void) { return 4; }
int wdef_data = 44;
int plt_z(void) { return 26; }
int plt_a(void) { return 1; }
int plt_m(void) { return 13; }
int plt_q(void) { return 17; }
int lib_get_tlsx(void) { return tlsx; }
void lib_set_tlsx(int v) { tlsx = v; }
int fr(void) { return 1; }   /* called only from ifunc resolvers (no GOT entry in the exe) */
int lib_get_dvar(void) { return dvar; }
int lib_get_oddsym(void) { return oddsym[0] | oddsym[1] << 8 | oddsym[2] << 16; }
int lib_get_zsym(void) { return zsym[0]; }
long long lib_get_dvbig(void) { return dvbig; }
void *lib_addr(const char *w) {
#define A(n) if (!strcmp(w, #n)) return (void *)&n;
  A(fa) A(fb) A(fc) A(wdef) A(wdef_data) A(plt_z) A(plt_a) A(plt_m) A(plt_q)
  A(printf) A(puts) A(dvar) A(dalias) A(darr) A(ro_obj) A(ro_rel) A(al64) A(dvbig)
  A(oddsym) A(a4sym) A(zsym) A(environ) A(__environ) A(stdout) A(tlsx) A(tlsy)
  return (void *)-1;
}
