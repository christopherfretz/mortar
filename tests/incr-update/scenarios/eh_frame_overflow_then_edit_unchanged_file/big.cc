// The changed file. BIG: a function with 64 try blocks (a large LSDA).
  int may_throw(int x);
  #ifdef BIG
  int big(int x) {
    int s = 0;
    try { s += may_throw(x + 0); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 1); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 2); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 3); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 4); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 5); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 6); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 7); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 8); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 9); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 10); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 11); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 12); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 13); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 14); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 15); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 16); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 17); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 18); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 19); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 20); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 21); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 22); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 23); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 24); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 25); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 26); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 27); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 28); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 29); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 30); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 31); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 32); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 33); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 34); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 35); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 36); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 37); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 38); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 39); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 40); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 41); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 42); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 43); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 44); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 45); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 46); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 47); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 48); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 49); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 50); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 51); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 52); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 53); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 54); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 55); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 56); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 57); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 58); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 59); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 60); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 61); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 62); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
try { s += may_throw(x + 63); } catch (int e) { s += e; } catch (long e) { s -= (int)e; }
    return s;
  }
  #else
  int big(int x) { return may_throw(x + 2) * 0; }  // x + 2: no throw (x = 0)
  #endif
