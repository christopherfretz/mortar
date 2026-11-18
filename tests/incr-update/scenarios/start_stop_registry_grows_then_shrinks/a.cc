// The changed file. EDIT: one more entry of the registry section `reg`.
#ifdef EDIT
__attribute__((section("reg"), used)) const int a_entry = 30;
#endif
int a_fn() { return 1; }
