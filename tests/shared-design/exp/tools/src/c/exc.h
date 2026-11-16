#pragma once
#include <stdexcept>
#include <string>
namespace libns {
struct Base { virtual ~Base(); virtual int id() const; };
struct Derived : Base { ~Derived() override; int id() const override; };
struct LibError : std::runtime_error {
    int code;
    LibError(const std::string &m, int c);
    ~LibError() override;            // key function: typeinfo lives in the library
};
int self_catch();                    // throw and catch inside the library
void throw_lib(int code);            // throws LibError
int call_cb_catch(void (*cb)());     // calls cb, catches libns::Base& thrown by it
Base *make_derived();
int is_derived(Base *);              // dynamic_cast<Derived*> inside the library
const std::type_info &derived_ti();
std::string greet(const std::string &who);   // ostringstream + std::cout
}
