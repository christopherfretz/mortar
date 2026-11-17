# Expat configured as it is by default (WP-126): shared and static, so
# libtool links libexpat.so (-shared -soname, a C library) and the
# programs against it. The tests are `make check`, as the expat row.
rb_build() {
  rb_configure "$RB_SRC/configure" --without-docbook
  make
}

rb_test() {
  make -k check > rb-check.log 2>&1
  rb_automake "$RB_BUILD"
}
