#include <cstdio>
#include <cstring>
#include <typeinfo>
struct Base { virtual ~Base(); virtual int id() const { return 1; } };
struct Left : virtual Base { int id() const override { return 2; } virtual int left() const { return 20; } };
struct Right : virtual Base { virtual int right() const { return 30; } };
struct Diamond : Left, Right { int id() const override { return 4; } int right() const override { return 31; } };
Base *make(int k);
const std::type_info &diamond_type();
int main() {
    int sum = 0;
    for (int k = 0; k < 4; ++k) {
        Base *b = make(k);
        Left *l = dynamic_cast<Left *>(b);
        Right *r = dynamic_cast<Right *>(b);
        Right *cross = l ? dynamic_cast<Right *>(l) : nullptr;
        void *whole = dynamic_cast<void *>(b);
        std::printf("k %d id %d left %d right %d cross %d whole %d name %s\n", k, b->id(),
                    l ? l->left() : -1, r ? r->right() : -1, cross ? cross->right() : -1,
                    whole != nullptr, typeid(*b).name());
        sum += b->id();
        delete b;
    }
    std::printf("same type_info %d\n", &diamond_type() == &typeid(Diamond));
    std::printf("type_info eq %d\n", diamond_type() == typeid(Diamond));
    try {
        Base &b = *make(0);
        (void)dynamic_cast<Diamond &>(b);
    } catch (const std::bad_cast &e) {
        std::printf("bad_cast caught\n");
    }
    return sum;
}
