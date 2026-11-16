/* an undefined reference for -z defs / --no-undefined */
extern int not_defined_anywhere(void);
int undef_user(void) { return not_defined_anywhere(); }
