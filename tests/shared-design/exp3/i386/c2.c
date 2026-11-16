static int counter; int g_exported_counter;
static int arr[10]; int after;
static const char name[] = "abc"; const char exported_name[] = "xyz";
int bump(void) { counter++; g_exported_counter++; return counter; }
int *arr_end(void) { return arr + 10; }
int loop(void) { int s = 0; for (int *p = arr; p < arr + 10; p++) s += *p; return s; }
const char *name_end(void) { return name + sizeof name; }
void set(void) { counter = 123456; arr[9] = 7; }
__attribute__((visibility("default"))) int vis_default = 4;
