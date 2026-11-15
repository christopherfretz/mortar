/* function addresses of shared-object functions taken from code, rw data, ro data */
#include <stdio.h>
extern int fa(void), fb(void), fc(void);
void *const ro_fp_fb = (void *)&fb;
void *rw_fp_fc = (void *)&fc;
void *canon_ro_fb(void) { void *const *volatile p = &ro_fp_fb; return *p; }
void *canon_rw_fc(void) { void **volatile p = &rw_fp_fc; return *p; }
void *canon_code_fa(void) { return (void *)&fa; }
void *canon_code_printf(void) { return (void *)&printf; }
extern int darr[4];
int *rw_darr2 = &darr[2];            /* R_*_64 / R_386_32 against a .so symbol with addend 8 */
int *canon_rw_darr2(void) { int **volatile p = &rw_darr2; return *p; }
