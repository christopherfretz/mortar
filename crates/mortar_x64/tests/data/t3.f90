module m
  real(8) :: bigarr(200000)
  real(8), allocatable :: a(:)
contains
  subroutine s(n)
    integer :: n
    bigarr(n) = bigarr(n) + 1
    !$omp parallel
    bigarr(1) = 2
    !$omp end parallel
  end subroutine
end module
