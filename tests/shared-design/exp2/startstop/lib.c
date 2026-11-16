__attribute__((section("cid"), used)) int a = 1;
extern int __start_cid[], __stop_cid[];
int count(void){ return __stop_cid - __start_cid; }
