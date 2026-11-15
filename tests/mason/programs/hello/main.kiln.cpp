// The smallest iostream program.
#include <iostream>
#include <string>
int main() {
    std::string s = "hello";
    std::cout << s << " from kiln+hod " << 42 << ' ' << 2.5 << std::endl;
    std::cerr << "to stderr\n";
    return 0;
}
