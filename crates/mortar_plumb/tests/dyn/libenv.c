extern char **_environ;
extern char **__environ;
extern char **environ;
/* report which storage each alias resolves to, as seen from a shared object */
void *addr_u_environ(void) { return &_environ; }
void *addr_uu_environ(void) { return &__environ; }
void *addr_environ(void) { return &environ; }
char **val_u_environ(void) { return _environ; }
