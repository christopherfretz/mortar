module m
  implicit none
  real(8) :: gvar(4) = (/1d0,2d0,3d0,4d0/)
  integer :: gcount = 7
contains
  function f(i) result(r)
    integer, intent(in) :: i
    real(8) :: r
    real(8), save :: tab(8) = (/1d0,2d0,3d0,4d0,5d0,6d0,7d0,8d0/)
    integer, save :: calls = 0
    calls = calls + 1
    tab(i) = tab(i) + gvar(mod(i,4)+1)
    r = tab(i) + calls
  end function
  subroutine g(i, x)
    integer, intent(in) :: i
    real(8), intent(in) :: x
    real(8), save :: acc(16) = 0d0
    acc(i) = acc(i) + x
    gcount = gcount + 1
  end subroutine
end module
