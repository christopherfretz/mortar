/* libtool-shaped C library */
int foo_hidden_by_script(void) { return 1; }
int foo(void) { return 41 + foo_hidden_by_script(); }
