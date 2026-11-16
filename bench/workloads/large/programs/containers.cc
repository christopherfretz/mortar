// C++: the standard library's containers, algorithms, strings, streams.
#include <algorithm>
#include <iostream>
#include <map>
#include <numeric>
#include <set>
#include <sstream>
#include <string>
#include <unordered_map>
#include <vector>

int main() {
  std::vector<int> v(1000);
  std::iota(v.begin(), v.end(), 0);
  std::reverse(v.begin(), v.end());
  std::stable_sort(v.begin(), v.end(), [](int a, int b) { return a % 7 < b % 7; });
  std::cout << "first " << v[0] << " last " << v.back() << '\n';
  std::map<std::string, int> words;
  std::istringstream in("the linker links the objects the archives give the linker");
  for (std::string w; in >> w;) ++words[w];
  for (auto &[w, n] : words) std::cout << w << '=' << n << ' ';
  std::cout << '\n';
  std::unordered_map<int, std::set<int>> divisors;
  for (int n = 1; n <= 60; n++)
    for (int d = 1; d <= n; d++)
      if (n % d == 0) divisors[n].insert(d);
  std::cout << "divisors of 60: " << divisors[60].size() << '\n';
  std::ostringstream out;
  out.precision(5);
  out << std::accumulate(v.begin(), v.end(), 0.0) / v.size();
  std::cout << "mean " << out.str() << '\n';
  return 0;
}
