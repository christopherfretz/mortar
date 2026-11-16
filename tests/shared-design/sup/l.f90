module ml
  implicit none
  real(8) :: gvar(4) = (/1d0,2d0,3d0,4d0/)
  integer :: gcount = 7
contains
  function s(n) result(r)
    integer, intent(in) :: n
    real(8) :: r
    real(8), save :: tab(8) = (/1d0,2d0,3d0,4d0,5d0,6d0,7d0,8d0/)
    integer :: i
    r = 0
    do i = 1, n
      tab(i) = tab(i) * 2 + gvar(1)
      r = r + tab(i)
    end do
  end function
  function t(i, j) result(r)
    integer, intent(in) :: i, j
    real(8) :: r
    real(8), save :: mat(3,3) = reshape((/1d0,2d0,3d0,4d0,5d0,6d0,7d0,8d0,9d0/),(/3,3/))
    mat(i,j) = mat(i,j) + gcount
    r = mat(i,j)
  end function
end module
