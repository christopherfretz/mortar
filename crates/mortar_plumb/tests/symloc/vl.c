/* WP-169: version-local keys (own DEFAULT, own PROTECTED), a common, a
   __start_/__stop_ section, _end and __bss_start; no .data. */
int foo(void) { return 1; }
__attribute__((visibility("protected"))) int bar(void) { return 2; }
int cm;
__attribute__((section("mysec"), used)) static const int a = 1;
__attribute__((section("mysec"), used)) static const int b = 2;
extern const int __start_mysec[], __stop_mysec[];
extern char _end[], __bss_start[];
int api(void) { return foo() + bar() + cm + (int)(__stop_mysec - __start_mysec); }
const char *api_end(void) { return _end; }
const char *api_bss(void) { return __bss_start; }
int api__start_mysec(void) { return 0; }
