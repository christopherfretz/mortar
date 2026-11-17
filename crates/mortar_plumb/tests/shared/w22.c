/* W22: -init names a hidden function nothing else references, under
   --gc-sections (compiled -ffunction-sections): mold collects it and
   writes DT_INIT = 0. */
int w22_ran;
__attribute__((visibility("hidden"))) void w22_init(void) { w22_ran = 1; }
int w22_get(void) { return w22_ran; }
