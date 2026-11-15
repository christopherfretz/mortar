module m
  implicit none
  real(8) :: arr(1000)
contains
  subroutine fill(n)
    integer, intent(in) :: n
    integer :: i
    do i=1,n; arr(i)=dble(i)*0.5d0; end do
  end subroutine
end module
program p
  use m
  implicit none
  character(len=32) :: s
  call fill(1000)
  write(s,'(F12.3)') sum(arr)
  print *, 'Fortran ok ', trim(adjustl(s))
end program
