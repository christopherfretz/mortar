program fmain
  use fmod
  implicit none
  integer :: a, b, y
  common /blk/ a, b
  call add_twice(5, y)
  print '(A,I0,A,I0)', 'fortran: add_twice(5)=', y, ' mod_counter=', mod_counter
  a = 1; b = 2
  call add_twice(5, y)
  if (y == 13 .and. mod_counter == 2) then
    print '(A)', 'OK fortran main (common block shared with library)'
  else
    print '(A,I0)', 'FAIL fortran main y=', y
  end if
end program
