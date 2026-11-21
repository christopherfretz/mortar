// The changed file (WP-230 G-5). GROW: c_value grows by two statements
// (its .text, .debug_line and .debug_info contributions change size).
// RENAME: c_fill7's local is renamed (a new .debug_str string appended,
// the old one dead: its name is used once and longer than a string
// offset, so GCC puts it in .debug_str too, not inline). DECL: u_dead is declared and called (GCC orders the
// file's .debug_str afresh: front 1's merge moves stored strings).
// Forty-odd small functions give the debug contributions room in their
// allotments (in a tiny file every growth is debug-compaction).
#ifdef GROW
#define C_GROW scaled = scaled * 3 + 1; scaled = scaled * 3 + 1;
#else
#define C_GROW
#endif
int u_helper(int);
#ifdef DECL
int u_dead(int);
#endif
int c_value(int x) {
  int scaled = x * 9;
  C_GROW
  return scaled + 1;
}
int c_fill0(int x) { int t = x * 2; return t - x; }
int c_fill1(int x) { int t = x * 3; return t - x; }
int c_fill2(int x) { int t = x * 4; return t - x; }
int c_fill3(int x) { int t = x * 5; return t - x; }
int c_fill4(int x) { int t = x * 6; return t - x; }
int c_fill5(int x) { int t = x * 7; return t - x; }
int c_fill6(int x) { int t = x * 8; return t - x; }
#ifdef RENAME
int c_fill7(int x) { int renamed_fill_wp230 = x * 9; return renamed_fill_wp230 - x; }
#else
int c_fill7(int x) { int original_fill_wp230 = x * 9; return original_fill_wp230 - x; }
#endif
int c_fill8(int x) { int t = x * 10; return t - x; }
int c_fill9(int x) { int t = x * 11; return t - x; }
int c_fill10(int x) { int t = x * 12; return t - x; }
int c_fill11(int x) { int t = x * 13; return t - x; }
int c_fill12(int x) { int t = x * 14; return t - x; }
int c_fill13(int x) { int t = x * 15; return t - x; }
int c_fill14(int x) { int t = x * 16; return t - x; }
int c_fill15(int x) { int t = x * 17; return t - x; }
int c_fill16(int x) { int t = x * 18; return t - x; }
int c_fill17(int x) { int t = x * 19; return t - x; }
int c_fill18(int x) { int t = x * 20; return t - x; }
int c_fill19(int x) { int t = x * 21; return t - x; }
int c_fill20(int x) { int t = x * 22; return t - x; }
int c_fill21(int x) { int t = x * 23; return t - x; }
int c_fill22(int x) { int t = x * 24; return t - x; }
int c_fill23(int x) { int t = x * 25; return t - x; }
int c_fill24(int x) { int t = x * 26; return t - x; }
int c_fill25(int x) { int t = x * 27; return t - x; }
int c_fill26(int x) { int t = x * 28; return t - x; }
int c_fill27(int x) { int t = x * 29; return t - x; }
int c_fill28(int x) { int t = x * 30; return t - x; }
int c_fill29(int x) { int t = x * 31; return t - x; }
int c_fill30(int x) { int t = x * 32; return t - x; }
int c_fill31(int x) { int t = x * 33; return t - x; }
int c_fill32(int x) { int t = x * 34; return t - x; }
int c_fill33(int x) { int t = x * 35; return t - x; }
int c_fill34(int x) { int t = x * 36; return t - x; }
int c_fill35(int x) { int t = x * 37; return t - x; }
int c_fill36(int x) { int t = x * 38; return t - x; }
int c_fill37(int x) { int t = x * 39; return t - x; }
int c_fill38(int x) { int t = x * 40; return t - x; }
int c_fill39(int x) { int t = x * 41; return t - x; }
int c_fill40(int x) { int t = x * 42; return t - x; }
int c_fill41(int x) { int t = x * 43; return t - x; }
#ifdef DECL
int c_fill42(int x) { int t = x * 44; return t - x; }
int c_fill_call(int x) { return u_dead(x) + u_helper(x); }
#else
int c_fill42(int x) { int t = x * 44; int w1 = t; int w2 = w1; int w3 = w2; return w3 - x; }
int c_fill_call(int x) { return u_helper(x); }
#endif
