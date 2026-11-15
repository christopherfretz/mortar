#include <cstdio>
#include <string>
#include <stdexcept>
#include <thread>
#include <vector>
#include <memory>
thread_local std::string ts = "tls";
struct B { virtual ~B(){} virtual int f() const = 0; };
struct D : B { int f() const override { return 42; } };
static int thrower(int x){ if (x>1) throw std::runtime_error("boom"); return x; }
int main(){
  int caught=0;
  try { thrower(3); } catch (const std::exception &e) { caught = (std::string(e.what())=="boom"); }
  std::vector<std::thread> v; int sum[4]={0};
  for(int i=0;i<4;i++) v.emplace_back([i,&sum]{ ts += std::to_string(i); sum[i]=ts.size(); });
  for(auto&t:v) t.join();
  std::unique_ptr<B> b(new D);
  std::printf("C++ ok caught=%d ts=%s sizes=%d%d%d%d virt=%d\n", caught, ts.c_str(), sum[0],sum[1],sum[2],sum[3], b->f());
}
