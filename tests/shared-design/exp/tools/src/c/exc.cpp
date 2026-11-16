#include "exc.h"
#include <iostream>
#include <sstream>
#include <typeinfo>
namespace libns {
Base::~Base() {}
int Base::id() const { return 1; }
Derived::~Derived() {}
int Derived::id() const { return 2; }
LibError::LibError(const std::string &m, int c) : std::runtime_error(m), code(c) {}
LibError::~LibError() {}
int self_catch() { try { throw 41; } catch (int i) { return i + 1; } return 0; }
void throw_lib(int code) { throw LibError("from library", code); }
int call_cb_catch(void (*cb)())
{
    try { cb(); }
    catch (const Base &b) { return 100 + b.id(); }
    catch (...) { return -1; }
    return 0;
}
Base *make_derived() { return new Derived; }
int is_derived(Base *b) { return dynamic_cast<Derived *>(b) != nullptr; }
const std::type_info &derived_ti() { return typeid(Derived); }
std::string greet(const std::string &who)
{
    std::ostringstream o;
    o << "hello " << who << " " << 42;
    std::cout << "EV lib cout: " << o.str() << std::endl;
    return o.str();
}
}
