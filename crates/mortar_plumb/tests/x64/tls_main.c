__thread int t_ext = 35;
int get_def(void); long get_loc(int); int get_ext(void);
int main(void) { return get_def() + (int)get_loc(0) + get_ext() - 43; }
