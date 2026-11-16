# {fmt}: CMake, template-heavy C++ (formatting at compile time, chrono,
# ranges, locale), exceptions; its test suite with the GoogleTest it
# bundles (test/gtest), one static library per test support.
rb_build() {
  rb_configure cmake -S "$RB_SRC" -B . -G "Unix Makefiles" -DFMT_TEST=ON
  make
}

rb_test() {
  rb_ctest .
}
