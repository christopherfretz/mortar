/* libw.a(w.o): foo and its link warning (glibc's link_warning form: the
   assembler comment leaves the section unallocated). */
int foo(int x) { return x + 1; }
static const char warn_foo[] __attribute__((used, section(".gnu.warning.foo\n\t#")))
    = "foo is deprecated";
