#!/bin/sh
# How tests/scriptorder/ was made (WP-193; AArch64, GNU ld 2.44, mortar of
# this branch). The outputs, maps and lines are the lane's own, kept by
#   cargo xtask linktest --filter needed-order-script --keep
#   cargo xtask linktest --filter needed-order-script-group --linker mortar --keep
# from tests/link/dyn/shlib/needed-order-script-group.test (line 20
# g-anx-obj, line 42 nested-group) and needed-order-script.test (line 405
# s-twice-off, line 544 input-then-group), each copied as
#   <out> -> <cell>.<linker>, .linktest-plumb-N.map -> <cell>.<linker>.map,
#   .linktest-ld-status-N.line -> <cell>.<linker>.line
# with the inputs (main0.o, use_x.o, the needed-libs fixture's libn*.so and
# the scripts), which are byte-identical between the runs. GNU ld stops at
# its first failing line, so nested-group.bfd was linked here by GNU ld from
# g-anx-obj.bfd.line with the inputs of line 42:
#   main0.o --no-as-needed libna.so --as-needed libnx.so --no-as-needed
#   libnb.so --as-needed g_sx_use.so --no-as-needed libnc.so
#   -o nested-group -Map=nested-group.bfd.map
# (ld @response-file of those words, the -L of the lane's bin dir removed).
# use_x.c is the source of use_x.o (cc -O1 -fPIE).
#
# sys/ (WP-241) holds the system files the lines name by absolute path,
# at those paths, so that no test reads the host's own (an x86-64 host
# has none of them). Copied from the AArch64 host (GCC 14.2, glibc 2.41):
#   /usr/lib/aarch64-linux-gnu/{Scrt1.o,crti.o,crtn.o,libc.so,libc_nonshared.a}
#   /usr/lib/gcc/aarch64-linux-gnu/14/{crtbeginS.o,crtendS.o,libgcc_s.so}
# Built from stub/ in place of the real files (5 MB), each with the
# soname, DT_NEEDED edges, version definitions and symbols the outputs
# and libn*.so bind (L="-nostdlib -shared -Wl,--build-id=none
# -Wl,-z,max-page-size=4096 -Wl,--hash-style=gnu -s"):
#   gcc $L -Wl,-soname,ld-linux-aarch64.so.1 -Wl,--version-script=ld.ver \
#     empty.s -o sys/lib/ld-linux-aarch64.so.1
#   gcc $L -Wl,-soname,libc.so.6 -Wl,--version-script=libc.ver libc.s \
#     -Wl,--no-as-needed <ld-linux-aarch64.so.1 above> \
#     -o sys/lib/aarch64-linux-gnu/libc.so.6
#   gcc $L -Wl,-soname,libgcc_s.so.1 -Wl,--version-script=gcc_s.ver \
#     empty.s -Wl,--no-as-needed <libc.so.6 above> \
#     -o sys/usr/lib/aarch64-linux-gnu/libgcc_s.so.1
#   printf '!<arch>\n' > sys/usr/lib/gcc/aarch64-linux-gnu/14/libgcc.a
# (no member of libgcc.a is in any of the links). On a host with the real
# files, s12_1a_the_sysroot_stands_for_the_aarch64_host checks that every
# verdict is the same with them.
exit 0
