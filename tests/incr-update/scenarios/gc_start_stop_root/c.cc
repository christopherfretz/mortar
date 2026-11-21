extern "C" const int __start_myreg[] __attribute__((weak));
extern "C" const int __stop_myreg[] __attribute__((weak));
int c_count() {
#ifdef EDIT
  return (int)(__stop_myreg - __start_myreg);
#else
  return 7;
#endif
}
