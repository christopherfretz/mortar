/* The C half of the Fortran workload (fortran.f90). */
int c_sum(int n) {
  int total = 0;
  for (int i = 1; i <= n; i++)
    total += i;
  return total;
}
