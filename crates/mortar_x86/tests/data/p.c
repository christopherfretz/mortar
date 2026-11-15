#include <stdio.h>
#include <string.h>
#include <locale.h>
#include <pthread.h>
#include <stdlib.h>
__thread int tv = 5;
__thread char tb[64] __attribute__((aligned(64)));
static __thread int ltv = 9;
extern __thread int gtv;
__thread int gtv = 11;
static void *th(void *a){ tv += (int)(long)a; ltv++; gtv++; return (void*)(long)(tv+ltv+gtv); }
#ifndef NOIFUNC
static int sel_a(void){return 1;} static int sel_b(void){return 2;}
static int (*resolve_sel(void))(void){ extern int selflag; return selflag ? sel_b : sel_a; }
int selflag = 0;
int sel(void) __attribute__((ifunc("resolve_sel")));
#else
static int sel(void){return 1;}
#endif
int main(void){
  char buf[200], src[200]; memset(src,'x',199); src[199]=0; memcpy(buf,src,200); 
  setlocale(LC_ALL,"C"); 
  pthread_t t; void *r; pthread_create(&t,0,th,(void*)3); pthread_join(t,&r);
  int (*fp)(void)=sel;
  printf("C ok len=%zu tv=%d ltv=%d gtv=%d thr=%ld sel=%d fp=%d tbmod=%lu\n", strlen(buf), tv, ltv, gtv,(long)r, sel(), fp(), (unsigned long)tb%64);
  return 0; }
