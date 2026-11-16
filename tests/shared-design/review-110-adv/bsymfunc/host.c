#include <stdio.h>
int get_dat(void),get_tv(void),call_fn(void),call_nt(void),call_ifn(void),get_wd(void),call_wf(void);
int main(void){printf("dat=%s tv=%s fn=%s nt=%s ifn=%s wd=%s wf=%s\n",
 get_dat()==10?"P":"L", get_tv()==20?"P":"L", call_fn()==30?"P":"L", call_nt()==40?"P":"L", call_ifn()==50?"P":"L", get_wd()==60?"P":"L", call_wf()==70?"P":"L");return 0;}
