# Reference LAPACK: CMake, Fortran with C interfaces (CBLAS, LAPACKE), a
# slice: the double-precision routines, real and complex (BUILD_DOUBLE,
# BUILD_COMPLEX16; the single-precision ones off: CBLAS's double tests
# call cblas_dzasum and cblas_dznrm2, which only the double complex set
# builds), static libraries. The
# tests are its ctest suite for that slice: BLAS levels 1 to 3, CBLAS,
# the LAPACK linear-equation and eigenvalue tests, LAPACKE's examples. The
# Fortran is gfortran's in both rows; the clang row compiles the C with
# Clang.
rb_build() {
  rb_configure cmake -S "$RB_SRC" -B . -G "Unix Makefiles" -DBUILD_TESTING=ON \
    -DCBLAS=ON -DLAPACKE=ON -DBUILD_SINGLE=OFF -DBUILD_COMPLEX=OFF -DBUILD_COMPLEX16=ON \
    -DBUILD_DOUBLE=ON -DLAPACKE_BUILD_SINGLE=OFF -DLAPACKE_BUILD_COMPLEX=OFF \
    -DLAPACKE_BUILD_COMPLEX16=ON -DLAPACKE_BUILD_DOUBLE=ON -DCMAKE_Fortran_COMPILER="$FC"
  make
}

rb_test() {
  rb_ctest .
}
