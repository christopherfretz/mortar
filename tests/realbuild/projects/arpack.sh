# ARPACK-NG: CMake, Fortran with its ISO_C_BINDING interface (-DICB=ON)
# and the C and C++ examples and tests that use it: one link line of C++,
# C and Fortran objects against BLAS and LAPACK (Debian's static
# libblas.a, liblapack.a: libblas-dev, liblapack-dev). Static libraries,
# its ctest suite.
rb_build() {
  rb_configure cmake -S "$RB_SRC" -B . -G "Unix Makefiles" -DICB=ON -DEXAMPLES=ON \
    -DBUILD_SHARED_LIBS=OFF -DMPI=OFF -DCMAKE_Fortran_COMPILER="$FC"
  make
}

rb_test() {
  rb_ctest .
}
