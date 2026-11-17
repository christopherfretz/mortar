/* WP-077: what libc offers in an executable's .preinit_array (glibc): getenv,
 * the envp argument, stdio to stderr; and at the end of the process (a
 * destructor after stdio's exit handlers). */
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
static void pre(int argc, char** argv, char** envp) {
  (void)argc; (void)argv;
  const char* g = getenv("HOME");
  int n = 0; while (envp && envp[n]) n++;
  fprintf(stderr, "preinit: getenv(HOME)=%s envp has %d entries; fprintf(stderr) works\n", g ? g : "(null)", n);
}
__attribute__((section(".preinit_array"), used)) static void (*p)(int, char**, char**) = pre;
__attribute__((destructor(101))) static void late(void) { fprintf(stderr, "destructor(101): fprintf(stderr) after main\n"); }
int main(void) { printf("main\n"); return 0; }
