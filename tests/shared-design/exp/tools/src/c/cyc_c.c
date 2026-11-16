/* C lifecycle library: DT_INIT/DT_FINI (via -init/-fini), constructor and
   destructor attributes, explicit .init_array/.fini_array entries. */
#include <stdio.h>
static int calls;
static void ev(const char *s) { printf("EV %s\n", s); fflush(stdout); }
void lib_init(void) { ev("DT_INIT lib_init"); }
void lib_fini(void) { ev("DT_FINI lib_fini"); }
__attribute__((constructor)) static void ctor_attr(void) { ev("ctor_attr"); }
__attribute__((destructor)) static void dtor_attr(void) { ev("dtor_attr"); }
__attribute__((constructor(200))) static void ctor_prio200(void) { ev("ctor_prio200"); }
__attribute__((destructor(200))) static void dtor_prio200(void) { ev("dtor_prio200"); }
static void ia1(void) { ev("init_array ia1"); }
static void ia2(void) { ev("init_array ia2"); }
static void fa1(void) { ev("fini_array fa1"); }
static void fa2(void) { ev("fini_array fa2"); }
/* one pointer per variable: a 16-byte array is 16-aligned on x86-64 and the
   padding would put a null entry into .init_array */
__attribute__((section(".init_array"), used)) static void (*p_ia1)(void) = ia1;
__attribute__((section(".init_array"), used)) static void (*p_ia2)(void) = ia2;
__attribute__((section(".fini_array"), used)) static void (*p_fa1)(void) = fa1;
__attribute__((section(".fini_array"), used)) static void (*p_fa2)(void) = fa2;
int cyc_call(void) { ev("cyc_call"); return ++calls; }
