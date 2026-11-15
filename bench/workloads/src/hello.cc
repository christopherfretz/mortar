// bench workload `hello-cxx` (spec/harness.md "bench", "Workloads"): a
// hello world that pulls in iostreams, strings, a container and an
// exception, as a small C++ program does.
#include <iostream>
#include <map>
#include <stdexcept>
#include <string>

int main(int argc, char **) {
    std::map<std::string, int> counts{{"hello", 1}, {"world", 2}};
    try {
        if (argc > 5) throw std::runtime_error("too many arguments");
        std::cout << "hello, world " << counts.at("world") << '\n';
    } catch (const std::exception &e) {
        std::cerr << e.what() << '\n';
        return 1;
    }
    return 0;
}
