# Expat: autoconf, automake and libtool, configured static
# (--disable-shared): every program is linked by libtool against
# libexpat.a. The tests are `make check`: the C and C++ unit tests
# (runtests, runtests_cxx) and the example programs, through automake's
# test driver.
rb_build() {
  rb_configure "$RB_SRC/configure" --disable-shared --without-docbook
  make
}

rb_test() {
  make -k check > rb-check.log 2>&1
  rb_automake "$RB_BUILD"
}
