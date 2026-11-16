# BusyBox: Kbuild (the Linux kernel's build system of its time): every
# directory's objects are combined by `ld -r` into built-in.o, and the
# final link is scripts/trylink's (--gc-sections, --sort-common,
# --start-group, retried without each library). `make defconfig`, then
# CONFIG_TC off (networking/tc.c does not build against the kernel
# headers of Debian 13: a configuration choice, not a patch). The tests
# are `make check` (testsuite/runtest: each applet's shell tests).
rb_build() {
  cd "$RB_SRC"
  rb_configure make defconfig HOSTCC="$CC"
  sed -i 's/^CONFIG_TC=y$/# CONFIG_TC is not set/' .config
  rb_configure make oldconfig HOSTCC="$CC" < /dev/null
  make CC="$CC" HOSTCC="$CC"
}

rb_test() {
  cd "$RB_SRC"
  make check CC="$CC" HOSTCC="$CC" > rb-check.log 2>&1
  sed -n 's/^\(PASS\|FAIL\|SKIPPED\|UNTESTED\): \(.*\)$/\2\t\1/p' rb-check.log |
    sed 's/\tSKIPPED$/\tSKIP/; s/\tUNTESTED$/\tSKIP/' >> "$RB_RESULTS"
}
