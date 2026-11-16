# GMP: autoconf, automake and libtool, assembly for the host's CPU, the C++
# class interface (--enable-cxx), configured static. The tests are `make
# check` (the library's, the C++ interface's, mpz, mpq, mpf, mpn and the
# rest) through automake's test driver.
rb_build() {
  rb_configure "$RB_SRC/configure" --disable-shared --enable-cxx
  make
}

rb_test() {
  make -k check > rb-check.log 2>&1
  rb_automake "$RB_BUILD"
}
