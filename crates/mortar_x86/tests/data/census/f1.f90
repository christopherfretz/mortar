module m
  real(8) :: bigarr(20000)
  integer, bind(c) :: shared_counter
  real(8), allocatable :: a(:)
  common /blk/ cb(1000)
  real(8) :: cb
contains
  subroutine s(n) bind(c, name="fsub")
    integer, value :: n
    integer :: i
    bigarr(n) = bigarr(n) + 1
    cb(n) = cb(n) + 2
    !$omp parallel do
    do i = 1, 4
      bigarr(i) = 2
    end do
    !$omp end parallel do
    shared_counter = shared_counter + n
  end subroutine
end module
