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
exit 0
