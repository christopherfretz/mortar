// Instantiated in both files: COMDAT groups, main.o's copies kept.
template <typename T> T twice(T v) { return v + v; }
inline int scale(int v) { return v * 3; }
