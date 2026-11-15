! The Fortran workload of the bench (bench/workloads/manifest,
! `fortran-static`): the program of tests/link/lang/fortran-c.test, a
! Fortran main that calls C through BIND(C).
program main
  use iso_c_binding
  implicit none
  interface
    integer(c_int) function c_sum(n) bind(c, name="c_sum")
      import :: c_int
      integer(c_int), value :: n
    end function
  end interface
  print '(A)', 'hello from fortran'
  print '(A,I0)', 'sum ', c_sum(10_c_int)
end program
