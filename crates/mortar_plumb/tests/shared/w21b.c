/* W21's second translation unit. */
extern int hif(void) __attribute__((visibility("hidden")));
int (*w21_pb)(void) = hif;
void *w21_ab(void) { return (void *)hif; }
int w21_cb(void) { return hif(); }
