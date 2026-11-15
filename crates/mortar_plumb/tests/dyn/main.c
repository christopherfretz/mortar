#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <dlfcn.h>
#include <unistd.h>
extern void *lib_addr(const char *);
extern int lib_get_tlsx(void), lib_get_exe_tls(void), lib_get_dvar(void), lib_get_oddsym(void), lib_get_zsym(void);
extern long long lib_get_dvbig(void);
extern void lib_set_tlsx(int);
extern int fa(void), fb(void), fc(void), plt_z(void), plt_a(void), plt_m(void);
extern int dvar, dalias, darr[4], al64, dvbig;
extern const int ro_obj;
extern const char *const ro_rel;
extern char oddsym[3], a4sym[4];
extern __thread int tlsx;
__thread int exe_tls = 77;
extern int wnone(void) __attribute__((weak));
extern int wnone_data __attribute__((weak));
extern int wdef(void) __attribute__((weak));
extern int wdef_data __attribute__((weak));
extern void *canon_ro_fb(void), *canon_rw_fc(void), *canon_code_fa(void), *canon_code_printf(void);
extern int tls_gd_read(void), *tls_gd_addr(void), tls_gd_exe(void);
extern int tls_desc_read(void), *tls_desc_addr(void), tls_desc_exe(void);
extern int ifn(void);
extern void *ifn_addr_other_tu(void);
extern int tu2(void);
static int fails;
static void chk(const char *n, const void *exe) {
  void *l = lib_addr(n), *d = dlsym(RTLD_DEFAULT, n);
  int ok = exe == l && l == d;
  printf("ADDR %-10s exe=%p lib=%p dlsym=%p %s\n", n, exe, l, d, ok ? "OK" : "MISMATCH");
  if (!ok) fails++;
}
#define VAL(n, got, want) do { long long g_ = (long long)(got), w_ = (long long)(want); \
  printf("VAL  %-16s got=%#llx want=%#llx %s\n", n, g_, w_, g_ == w_ ? "OK" : "BAD"); if (g_ != w_) fails++; } while (0)
int main(void) {
  setvbuf(stdout, NULL, _IONBF, 0);
  /* call order deliberately z, a, m (tu2 calls m then q) */
  VAL("plt_z()", plt_z(), 26); VAL("plt_a()", plt_a(), 1); VAL("plt_m()", plt_m(), 13); VAL("tu2()", tu2(), 30);
  chk("fa", (void *)&fa); chk("printf", (void *)&printf); chk("puts", (void *)&puts);
  chk("dvar", &dvar); chk("dalias", &dalias); chk("darr", darr); chk("ro_obj", &ro_obj);
  chk("ro_rel", &ro_rel); chk("al64", &al64); chk("oddsym", oddsym); chk("a4sym", a4sym);
  chk("environ", &environ); chk("__environ", &__environ); chk("stdout", &stdout);
  chk("tlsx", &tlsx);
  { extern void *libt_exe_tls_addr(void); void *l = libt_exe_tls_addr(), *d = dlsym(RTLD_DEFAULT, "exe_tls"); int ok = l == (void *)&exe_tls && d == l;
    printf("ADDR exe_tls    exe=%p lib=%p dlsym=%p %s\n", (void *)&exe_tls, l, d, ok ? "OK" : "MISMATCH"); if (!ok) fails++; }
  printf("INFO &environ=%p &__environ=%p same=%d dlsym(environ)=%p dlsym(__environ)=%p\n", (void *)&environ,
         (void *)&__environ, (void *)&environ == (void *)&__environ, dlsym(RTLD_DEFAULT, "environ"), dlsym(RTLD_DEFAULT, "__environ"));
  printf("INFO align: dvar%%64=%lu al64%%64=%lu oddsym%%16=%lu a4sym%%16=%lu ro_obj%%16=%lu\n",
         (unsigned long)&dvar % 64, (unsigned long)&al64 % 64, (unsigned long)oddsym % 16, (unsigned long)a4sym % 16,
         (unsigned long)&ro_obj % 16);
  /* function pointers through other routes */
  VAL("canon_code_fa", canon_code_fa() == lib_addr("fa"), 1);
  VAL("canon_code_printf", canon_code_printf() == lib_addr("printf"), 1);
  VAL("canon_ro_fb", canon_ro_fb() == lib_addr("fb"), 1);
  VAL("canon_rw_fc", canon_rw_fc() == lib_addr("fc"), 1);
  { extern int *canon_rw_darr2(void); VAL("rw &darr[2]", canon_rw_darr2() == &darr[2], 1); VAL("lib &darr[2]", canon_rw_darr2() == (int *)lib_addr("darr") + 2, 1); }
  VAL("call via ro_fb", ((int (*)(void))canon_ro_fb())(), 2);
  /* data values / writes visible both ways */
  VAL("dvar", dvar, 42); dvar = 43; VAL("lib sees dvar", lib_get_dvar(), 43);
  VAL("dalias", dalias, 43); VAL("ro_obj", ro_obj, 7); VAL("ro_rel", strcmp(ro_rel, "ro_rel_str"), 0);
  VAL("oddsym", oddsym[0] | oddsym[1] << 8 | oddsym[2] << 16, 0x332211); oddsym[1] = 0x23;
  VAL("lib sees oddsym", lib_get_oddsym(), 0x332311);
  VAL("a4sym[0]", a4sym[0], 0x44);
  VAL("darr[3]", darr[3], 4); VAL("al64", al64, 64);
  printf("INFO dvbig exe=%#x lib=%#llx\n", dvbig, lib_get_dvbig());
  /* TLS */
  VAL("tlsx IE", tlsx, 5); tlsx = 50; VAL("lib tlsx", lib_get_tlsx(), 50); lib_set_tlsx(51);
  VAL("tlsx after lib set", tlsx, 51); VAL("tls_gd_read", tls_gd_read(), 51); VAL("tls_desc_read", tls_desc_read(), 51);
  VAL("tls_gd_addr", tls_gd_addr() == &tlsx, 1); VAL("tls_desc_addr", tls_desc_addr() == &tlsx, 1);
  exe_tls = 78; VAL("lib exe_tls", lib_get_exe_tls(), 78); VAL("tls_gd_exe", tls_gd_exe(), 78); VAL("tls_desc_exe", tls_desc_exe(), 78);
  /* undefined weak */
  printf("INFO &wnone=%p &wnone_data=%p &wdef=%p &wdef_data=%p\n", (void *)&wnone, (void *)&wnone_data, (void *)&wdef, (void *)&wdef_data);
  VAL("&wnone==0", &wnone == 0, 1); VAL("&wnone_data==0", &wnone_data == 0, 1);
  VAL("&wdef==lib", (void *)&wdef == lib_addr("wdef"), 1); VAL("&wdef_data==lib", (void *)&wdef_data == lib_addr("wdef_data"), 1);
  VAL("wdef()", &wdef ? wdef() : -1, 4); VAL("wdef_data", &wdef_data ? wdef_data : -1, 44);
  /* ifunc defined in the exe */
  VAL("ifn()", ifn(), 11); VAL("&ifn same both TUs", (void *)&ifn == ifn_addr_other_tu(), 1);
  VAL("call &ifn", ((int (*)(void))ifn_addr_other_tu())(), 11);
  printf("RESULT %s fails=%d\n", fails ? "FAIL" : "PASS", fails);
  return fails != 0;
}
