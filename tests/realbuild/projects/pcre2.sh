# PCRE2: autoconf, automake and libtool, configured static with the 16-
# and 32-bit libraries and the JIT compiler. The tests are `make check`
# (RunTest, RunGrepTest, the JIT test, the unit tests) through automake's
# test driver.
rb_build() {
  rb_configure "$RB_SRC/configure" --disable-shared --enable-pcre2-16 --enable-pcre2-32 --enable-jit
  make
}

rb_test() {
  make -k check > rb-check.log 2>&1
  rb_automake "$RB_BUILD"
}
