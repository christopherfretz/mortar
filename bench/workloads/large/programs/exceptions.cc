// C++: exceptions through frames and destructors, RTTI, thread_local, threads.
#include <cstdio>
#include <stdexcept>
#include <string>
#include <thread>
#include <typeinfo>
#include <vector>

static thread_local int depth_seen = 0;
static int destroyed = 0;

struct Guard { ~Guard() { ++destroyed; } };

struct Oops : std::runtime_error { int code; Oops(int c) : std::runtime_error("oops"), code(c) {} };

[[gnu::noinline]] static void dive(int n) {
  Guard g;
  depth_seen = n;
  if (n == 0) throw Oops(7);
  dive(n - 1);
}

int main() {
  try {
    dive(50);
  } catch (const std::exception &e) {
    std::printf("caught %s code %d type %s destroyed %d\n", e.what(),
                dynamic_cast<const Oops &>(e).code, typeid(e).name(), destroyed);
  }
  std::vector<std::thread> threads;
  std::vector<int> results(4);
  for (int t = 0; t < 4; t++)
    threads.emplace_back([t, &results] {
      try {
        dive(t + 3);
      } catch (const Oops &) {
        results[t] = depth_seen + 10 * t;
      }
    });
  for (auto &t : threads) t.join();
  for (int r : results) std::printf("%d ", r);
  std::printf("\n");
  try {
    std::string s("x");
    (void)s.at(5);
  } catch (const std::out_of_range &) {
    std::printf("out_of_range\n");
  }
  return 0;
}
