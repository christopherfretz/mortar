module m1
  integer :: counter = 3
contains
  integer function twice(x) bind(c, name="twice")
    integer, value :: x
    twice = 2 * x + counter
  end function
end module
