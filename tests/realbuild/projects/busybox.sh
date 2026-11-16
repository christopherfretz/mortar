# BusyBox: Kbuild (the Linux kernel's build system of its time): every
# directory's objects are combined by `ld -r` into built-in.o, and the
# final link is scripts/trylink's (--gc-sections, --sort-common,
# --start-group, retried without each library). `make defconfig`, then
# CONFIG_TC off (networking/tc.c does not build against the kernel
# headers of Debian 13: a configuration choice, not a patch). The tests
# are `make check` (testsuite/runtest: each applet's shell tests).
#
# Two test files are removed before `make check`, a configuration choice
# (WP-111): the lane runs as root, and these two are the suite's tests
# that run only as root and then change the machine.
#
# - testsuite/mount.tests: mounts (`mount -o loop`, `--bind`,
#   `--make-rshared`, `-t tmpfs`, `remount`) and loads a module
#   (`modprobe minix`). Its `mount -o loop` reaches libbb/loop.c's
#   set_loop, which, with no /dev/loop-control and
#   CONFIG_FEATURE_MOUNT_LOOP_CREATE=y (defconfig), mknods every missing
#   /dev/loopN for N up to 0xfffff while no node opens: on 2026-11-16
#   that put 1,048,576 block nodes in this container's /dev.
# - testsuite/makedevs.tests: mknods a device table (block and character
#   nodes, /dev/kmem's among them) in the build tree.
#
# Their tests are recorded SKIP with that reason. testsuite/mdev.tests
# also mknods as root, inside a chroot of its own directory, and only
# with CONFIG_STATIC (off in defconfig: it reports SKIPPED itself); the
# *.testroot files (mount.testroot) are run only by umlwrapper.sh, never
# by runtest.
rb_build() {
  cd "$RB_SRC"
  rb_configure make defconfig HOSTCC="$CC"
  sed -i 's/^CONFIG_TC=y$/# CONFIG_TC is not set/' .config
  rb_configure make oldconfig HOSTCC="$CC" < /dev/null
  make CC="$CC" HOSTCC="$CC"
}

rb_test() {
  cd "$RB_SRC"
  rm -f testsuite/mount.tests testsuite/makedevs.tests
  local t
  for t in "mount -o remount,mand" "mount bind+rshared" "mount RO loop" \
    "makedevs -d ../makedevs.device_table.txt ."; do
    rb_result "$t" SKIP
  done
  make check CC="$CC" HOSTCC="$CC" > rb-check.log 2>&1
  sed -n 's/^\(PASS\|FAIL\|SKIPPED\|UNTESTED\): \(.*\)$/\2\t\1/p' rb-check.log |
    sed 's/\tSKIPPED$/\tSKIP/; s/\tUNTESTED$/\tSKIP/' >> "$RB_RESULTS"
}
