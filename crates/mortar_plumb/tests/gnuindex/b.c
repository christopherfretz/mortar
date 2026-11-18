/* WP-251: a second -gsplit-dwarf object (DWARF 4). */
typedef unsigned long word;
word counter;
int helper(int x) { counter += (word)x; return x * 3 - 2; }
