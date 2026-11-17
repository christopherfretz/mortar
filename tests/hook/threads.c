/* WP-077 case J (after prototype 2's hook2.sh): 8 threads x 300 dlopen/dlclose,
 * alternating RTLD_LOCAL/RTLD_GLOBAL, of 4 runtime libraries with shared keys;
 * thread 0 forks every 10th iteration. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/wait.h>
static const char* dir;
static void* run(void* a) {
  long t = (long)a; char p[512];
  for (int i = 0; i < 300; i++) {
    snprintf(p, sizeof p, "%s/libj%ld.so", dir, (t + i) % 4 + 1);
    void* h = dlopen(p, RTLD_NOW | (i & 1 ? RTLD_GLOBAL : RTLD_LOCAL));
    if (!h) { fprintf(stderr, "dlopen: %s\n", dlerror()); exit(2); }
    if (t == 0 && i % 10 == 0) { pid_t c = fork(); if (c == 0) _exit(0); waitpid(c, 0, 0); }
    dlclose(h);
  }
  return 0;
}
int main(int argc, char** argv) {
  dir = argv[1]; pthread_t th[8];
  for (long t = 0; t < 8; t++) pthread_create(&th[t], 0, run, (void*)t);
  for (int t = 0; t < 8; t++) pthread_join(th[t], 0);
  puts("done"); return 0;
}
