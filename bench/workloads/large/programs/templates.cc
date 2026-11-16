// C++: templates, variadics, constexpr, CRTP, virtual dispatch, lambdas.
#include <array>
#include <cstdio>
#include <functional>
#include <memory>
#include <string>
#include <tuple>
#include <type_traits>
#include <vector>

template <class... T> constexpr auto total(T... v) { return (v + ... + 0); }

template <unsigned N> struct Fact { static constexpr unsigned long value = N * Fact<N - 1>::value; };
template <> struct Fact<0> { static constexpr unsigned long value = 1; };

template <class D> struct Shape {
  double area() const { return static_cast<const D *>(this)->area_impl(); }
};
struct Square : Shape<Square> { double s; double area_impl() const { return s * s; } };

struct Animal { virtual ~Animal() = default; virtual std::string say() const = 0; };
struct Cat : Animal { std::string say() const override { return "meow"; } };
struct Dog : Animal { std::string say() const override { return "woof"; } };

template <class F, class... A> auto apply_all(F f, A... a) { return std::make_tuple(f(a)...); }

int main() {
  static_assert(Fact<10>::value == 3628800);
  std::printf("total %d fact %lu\n", total(1, 2, 3, 4), Fact<12>::value);
  Square sq{{}, 3.0};
  std::printf("area %.1f\n", sq.area());
  std::vector<std::unique_ptr<Animal>> zoo;
  for (int i = 0; i < 5; i++)
    zoo.push_back(i % 2 ? std::unique_ptr<Animal>(new Cat) : std::unique_ptr<Animal>(new Dog));
  std::string all;
  for (auto &a : zoo) all += a->say() + " ";
  std::printf("%s\n", all.c_str());
  auto t = apply_all([](auto x) { return x * 2; }, 1, 2.5, 3L);
  std::printf("%d %.1f %ld\n", std::get<0>(t), std::get<1>(t), std::get<2>(t));
  std::function<int(int)> rec = [&](int n) { return n <= 1 ? 1 : n * rec(n - 1); };
  std::array<int, 4> arr{4, 3, 2, 1};
  int acc = 0;
  for (int x : arr) acc = acc * 10 + rec(x) % 10;
  std::printf("acc %d\n", acc);
  return 0;
}
