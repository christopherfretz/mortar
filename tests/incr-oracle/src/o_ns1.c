__attribute__((section("o_newsec"), used)) int o_new_var = 3;
int lf(int x){ return x + o_new_var; }
