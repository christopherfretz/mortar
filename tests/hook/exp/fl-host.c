#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdio.h>
int main(int argc, char** argv) {
  void* h = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
  printf("dlopen(%s) = %p%s%s\n", argv[1], h, h ? "" : ": ", h ? "" : dlerror());
  printf("the process goes on\n");
  return 0;
}
