# GoogleTest as shared libraries (WP-126): the googletest project's
# configuration with -DBUILD_SHARED_LIBS=ON, so libgtest.so, libgtest_main.so,
# libgmock.so and libgmock_main.so are -shared -soname lines and every test
# binary links against them (CMake's build-tree RPATH finds them at run
# time). The tests are the same: death tests, threads, exceptions, typed
# tests; a few are Python scripts driving a test binary.
rb_build() {
  rb_configure cmake -S "$RB_SRC" -B . -G "Unix Makefiles" -DBUILD_SHARED_LIBS=ON -Dgtest_build_tests=ON -Dgmock_build_tests=ON
  make
}

rb_test() {
  rb_ctest .
}
