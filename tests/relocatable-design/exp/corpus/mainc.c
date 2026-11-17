/* mainc.c: the final link's own object; prints what the component did. */
#include <stdio.h>
#include <string.h>
static int order[16], n;
void rec(int v) { if (n < 16) order[n++] = v; }
extern int g_data, common_x[];
extern __thread int tv;
const char *msg1(void); const char *msg2(void);
int wk(void); int wk_only(void); int bump(void); int call_undef_weak(void); int use_hid(void);
int ifn(void) __attribute__((weak));   /* c4.c, when linked */
int main(void) {
  printf("ctor order:"); for (int i = 0; i < n; i++) printf(" %d", order[i]); printf("\n");
  printf("g_data=%d wk=%d wk_only=%d bump=%d,%d undef_weak=%d ifn=%d use_hid=%d tv=%d\n",
         g_data, wk(), wk_only(), bump(), bump(), call_undef_weak(), ifn ? ifn() : -1, use_hid(), tv);
  printf("msg same=%d %s common_x=%d\n", msg1() == msg2() ? 1 : 0, msg1(), common_x[0]);
  return 0;
}
