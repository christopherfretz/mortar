# zlib: its own configure script and Makefile, the default configuration
# (libz.a and libz.so, the shared library with a version script); the
# tests are the Makefile's: example and minigzip linked against libz.a,
# examplesh and minigzipsh against libz.so, and the 64-bit offset pair.
# Built in the sources' copy (zlib's configure builds in place).
rb_build() {
  cd "$RB_SRC"
  rb_configure ./configure
  make
}

rb_test() {
  cd "$RB_SRC"
  rb_run teststatic make teststatic
  rb_run testshared make testshared
  rb_run test64 make test64
}
