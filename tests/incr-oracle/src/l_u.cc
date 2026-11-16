#include <stdexcept>
int g(int);
int keep(int x){ try { return g(x); } catch (const std::exception&) { return -1; } }
int dropme(int x){ try { return g(x)+1; } catch (const std::exception&) { return -2; } }
