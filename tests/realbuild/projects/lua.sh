# Lua: a hand-written Makefile (`make linux`): liblua.a, and lua and luac
# linked against it with -Wl,-E (the interpreter exports its API to C
# modules). The tests are Lua's own suite for the release (a separate
# archive, RB_EXTRA): `all.lua` in its portable mode (`_U`), with the
# suite's C modules (libs/, `-shared`) built first, as its README says.
rb_build() {
  cd "$RB_SRC"
  make linux CC="$CC" MYCFLAGS="$CFLAGS"
  make -C "$RB_EXTRA/libs" CC="$CC" LUA_DIR="$RB_SRC/src"
}

rb_test() {
  cd "$RB_EXTRA"
  rb_run all.lua "$RB_SRC/src/lua" -e "_U=true" all.lua
  rb_run luac "$RB_SRC/src/luac" -v
}
