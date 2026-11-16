int lf(int x){ int s = 0; for (int i = 0; i < x; i++) s += i * x + (s >> 3); return s + x * 3; }
