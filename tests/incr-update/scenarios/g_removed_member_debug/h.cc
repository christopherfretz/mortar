// lib.a's member: in the link only while x.o calls it.
int h_extra(int x) {
  int doubled = x * 2;
  return doubled + 3;
}
