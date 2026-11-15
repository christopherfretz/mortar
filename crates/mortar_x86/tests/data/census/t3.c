__thread int a1 = 1; __thread int a2 = 2; static __thread int a3 = 3; __thread int a4;
extern __thread int ex;
int f1(void){ return a1 + a2; }
int f2(void){ return a3 + a4; }
int f3(void){ return ex; }
int *f4(void){ return &a3; }
int *f5(void){ return &ex; }
int f6(int i){ static __thread int arr[16]; arr[i] = i; return arr[(i+1)&15] + a3; }
