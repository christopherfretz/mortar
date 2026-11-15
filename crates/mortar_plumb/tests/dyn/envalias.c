#include <stdio.h>
#include <stdlib.h>
extern char **environ;
void *addr_u_environ(void); void *addr_uu_environ(void); void *addr_environ(void); char **val_u_environ(void);
int main(void) {
  setenv("MORTAR_NEW_VAR", "1", 1); /* may reallocate the array: updates __environ */
  printf("exe&environ=%d lib&environ=%d lib&__environ=%d lib&_environ=%d  _environ-value-current=%d\n",
         1, addr_environ() == (void *)&environ, addr_uu_environ() == (void *)&environ,
         addr_u_environ() == (void *)&environ, val_u_environ() == environ);
  return 0;
}
