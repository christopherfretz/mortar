! Fortran routines with C bindings, called from kiln-built C++, and a
! Fortran routine calling back into C++.
module mason_f
  use iso_c_binding
  implicit none
  interface
    function cxx_square(x) bind(C, name="cxx_square") result(r)
      import :: c_int
      integer(c_int), value :: x
      integer(c_int) :: r
    end function
  end interface
contains
  function f_dot(n, a, b) bind(C, name="f_dot") result(r)
    integer(c_int), value :: n
    real(c_double), intent(in) :: a(n), b(n)
    real(c_double) :: r
    r = dot_product(a, b)
  end function
  function f_sum_squares(n) bind(C, name="f_sum_squares") result(r)
    integer(c_int), value :: n
    integer(c_int) :: r, i
    r = 0
    do i = 1, n
      r = r + cxx_square(i)
    end do
  end function
  subroutine f_print(n) bind(C, name="f_print")
    integer(c_int), value :: n
    write(*, '(A,I0)') 'fortran says ', n
    flush(6)
  end subroutine
end module
