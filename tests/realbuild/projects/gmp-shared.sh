# GMP configured shared (its default, WP-126) with the C++ interface: libtool
# links libgmp.so (C and hand-written assembly) and libgmpxx.so (C++, needing
# libgmp.so), and the tests against them. The tests are `make check`, as the
# gmp row.
rb_build() {
  rb_configure "$RB_SRC/configure" --enable-cxx
  make
}

rb_test() {
  make -k check > rb-check.log 2>&1
  rb_automake "$RB_BUILD"
}
