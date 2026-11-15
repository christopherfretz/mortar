// Virtual dispatch, multiple and virtual inheritance, dynamic_cast (down,
// cross, to void*), typeid across translation units, a key function in
// this TU.
#include <typeinfo>
struct Base { virtual ~Base(); virtual int id() const { return 1; } };
struct Left : virtual Base { int id() const override { return 2; } virtual int left() const { return 20; } };
struct Right : virtual Base { virtual int right() const { return 30; } };
struct Diamond : Left, Right { int id() const override { return 4; } int right() const override { return 31; } };
Base::~Base() {}
Base *make(int k) {
    switch (k) {
    case 0: return new Base;
    case 1: return new Left;
    case 2: return new Right;
    default: return new Diamond;
    }
}
const std::type_info &diamond_type() { return typeid(Diamond); }
