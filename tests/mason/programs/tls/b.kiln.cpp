#include <cstdio>
#include <mutex>
#include <thread>
#include <vector>
extern thread_local int counter;
extern thread_local int dyn;
int bump();
const char *tracked_name();
static std::mutex m;
static thread_local int local_tl = 7;
static void work(int i) {
    for (int j = 0; j < i; ++j) bump();
    local_tl += i;
    std::lock_guard<std::mutex> g(m);
    std::printf("thread %d: counter %d dyn>0 %d local %d name %s\n", i, counter, dyn > 0, local_tl,
                tracked_name());
}
int main() {
    work(0);
    for (int i = 1; i <= 3; ++i) {
        std::thread t(work, i);
        t.join();
    }
    std::printf("main: counter %d local %d\n", counter, local_tl);
    return 0;
}
