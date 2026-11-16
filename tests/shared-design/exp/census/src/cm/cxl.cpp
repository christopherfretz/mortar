#include <stdexcept>
extern "C" int cl_value(void);
int cxl_value(int x) { if (x < 0) throw std::invalid_argument("negative"); return cl_value() + x; }
