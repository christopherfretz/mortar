// census: exceptions across the libgtest_dll.so boundary.
// The library throws (throw_on_failure makes its failure reporter throw
// GoogleTestFailureException, a std::runtime_error); this executable catches.
#include <gtest/gtest.h>
#include <cstdio>
#include <stdexcept>
#include <typeinfo>
static int thrower(int x) { if (x) throw std::logic_error("exe-thrown"); return 0; }
TEST(Cross, ExeThrowCaughtByLibMacro) { EXPECT_THROW(thrower(1), std::logic_error); }
TEST(Cross, Equality) { EXPECT_EQ(42, 6 * 7); }
int main(int argc, char **argv) {
  testing::InitGoogleTest(&argc, argv);
  int rc = RUN_ALL_TESTS();
  std::printf("%s RUN_ALL_TESTS=%d\n", rc == 0 ? "OK" : "FAIL", rc);
  GTEST_FLAG_SET(throw_on_failure, true);
  try {
    ADD_FAILURE() << "deliberate failure outside a test";
    std::printf("FAIL no exception from the library\n");
    return 1;
  } catch (const std::runtime_error &e) {
    std::printf("OK caught library-thrown %s\n", typeid(e).name());
  }
  return rc;
}
