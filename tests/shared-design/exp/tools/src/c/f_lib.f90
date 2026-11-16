! Fortran shared library: a module procedure and a common block
module fmod
  use iso_c_binding
  implicit none
  integer :: mod_counter = 0
contains
  subroutine add_twice(x, y)
    integer, intent(in) :: x
    integer, intent(out) :: y
    integer :: a, b
    common /blk/ a, b
    mod_counter = mod_counter + 1
    y = 2 * x + a + b
  end subroutine
  function c_add(x) bind(c, name="f_add") result(y)
    integer(c_int), value :: x
    integer(c_int) :: y
    integer(c_int) :: ca, cb
    common /cblk/ ca, cb
    bind(c, name="cblk") :: /cblk/
    y = x + ca * 10 + cb
  end function
end module
block data blkinit
  integer :: a, b
  common /blk/ a, b
  data a, b /100, 1000/
end block data
