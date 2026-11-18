// The changed file. EDIT: grow() is about 4096 statements long.

#define S1 x = x * 3 + 1;
#define S8 S1 S1 S1 S1 S1 S1 S1 S1
#define S64 S8 S8 S8 S8 S8 S8 S8 S8
#define S512 S64 S64 S64 S64 S64 S64 S64 S64

    int grow(int x) {
    #ifdef EDIT
      S512 S512 S512 S512 S512 S512 S512 S512
    #endif
      return x + 1;
    }
