! census library, Fortran
module censuslib
  use iso_c_binding
  implicit none
  integer(c_int), bind(C, name='lib_inited') :: lib_inited = 0
  integer(c_int), bind(C, name='lib_data') :: lib_data = 41
contains
  integer(c_int) function lib_func() bind(C, name='lib_func')
    character(len=16) :: s
    write (s, '(I0)') lib_data + 1
    read (s, *) lib_func
  end function
  subroutine my_init() bind(C, name='my_init')
    lib_inited = 1
  end subroutine
  subroutine my_fini() bind(C, name='my_fini')
  end subroutine
  integer(c_int) function lib_check() bind(C, name='lib_check')
    print '(A,I0)', 'lib Fortran data=', lib_data
    lib_check = lib_func()
  end function
end module
