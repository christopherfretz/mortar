# GoogleTest: CMake (Unix Makefiles, make through the jobserver), static
# libraries gtest, gtest_main, gmock, gmock_main, and its own tests
# (-Dgtest_build_tests=ON -Dgmock_build_tests=ON): death tests, threads,
# exceptions, typed tests; a few are Python scripts driving a test binary.
rb_build() {
  rb_configure cmake -S "$RB_SRC" -B . -G "Unix Makefiles" -Dgtest_build_tests=ON -Dgmock_build_tests=ON
  make
}

rb_test() {
  rb_ctest .
}
