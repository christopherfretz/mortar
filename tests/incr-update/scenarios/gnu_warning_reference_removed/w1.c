/* lib.a(w1.o): w_once and its link warning (glibc's link_warning form:
   the assembler comment leaves the section unallocated). */
int w_once(int x) { return x + 1; }
static const char warn_w_once[] __attribute__((used, section(".gnu.warning.w_once\n\t#")))
    = "w_once is deprecated; use w_both";
