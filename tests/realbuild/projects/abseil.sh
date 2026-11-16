# Abseil: CMake, about 140 static libraries, and its tests (each test
# binary links tens of the libraries and GoogleTest, which is the
# release's own archive, RB_EXTRA, as ABSL_LOCAL_GOOGLETEST_DIR names it:
# nothing is downloaded at configure time). C++17, threads, thread-local
# storage, exceptions, symbolization of its own stack traces.
rb_build() {
  rb_configure cmake -S "$RB_SRC" -B . -G "Unix Makefiles" -DABSL_BUILD_TESTING=ON \
    -DABSL_USE_EXTERNAL_GOOGLETEST=OFF -DABSL_LOCAL_GOOGLETEST_DIR="$RB_EXTRA" \
    -DCMAKE_CXX_STANDARD=17 -DBUILD_TESTING=ON
  make
}

rb_test() {
  rb_ctest .
}
