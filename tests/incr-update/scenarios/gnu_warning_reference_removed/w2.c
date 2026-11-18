/* lib.a(w2.o): w_both and its link warning. */
int w_both(int x) { return x + 2; }
static const char warn_w_both[] __attribute__((used, section(".gnu.warning.w_both\n\t#")))
    = "w_both is referenced";
