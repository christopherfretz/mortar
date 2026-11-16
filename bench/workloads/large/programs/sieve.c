/* C: arrays, recursion, varargs, qsort, setjmp, printf formatting. */
#include <setjmp.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static jmp_buf escape;

static int sum(int n, ...) {
  va_list ap;
  va_start(ap, n);
  int s = 0;
  for (int i = 0; i < n; i++) s += va_arg(ap, int);
  va_end(ap);
  return s;
}

static int cmp(const void *a, const void *b) {
  double x = *(const double *)a, y = *(const double *)b;
  return (x > y) - (x < y);
}

static long fib(int n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }

static void deep(int n) {
  if (n == 0) longjmp(escape, 42);
  deep(n - 1);
}

int main(void) {
  enum { N = 100000 };
  static char composite[N];
  int primes = 0;
  for (int i = 2; i < N; i++) {
    if (composite[i]) continue;
    primes++;
    for (long j = (long)i * i; j < N; j += i) composite[j] = 1;
  }
  printf("primes below %d: %d\n", N, primes);
  double v[16];
  for (int i = 0; i < 16; i++) v[i] = (i * 7919 % 16) / 3.0;
  qsort(v, 16, sizeof v[0], cmp);
  for (int i = 0; i < 16; i++) printf("%.3f%c", v[i], i == 15 ? '\n' : ' ');
  printf("sum %d fib(25) %ld\n", sum(5, 1, 2, 3, 4, 5), fib(25));
  int r = setjmp(escape);
  if (r == 0) deep(100);
  printf("longjmp %d, %s\n", r, strchr("linker", 'k'));
  return 0;
}
