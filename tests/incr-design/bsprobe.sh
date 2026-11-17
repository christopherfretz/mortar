#!/bin/bash
# build-system view of a torn output: fresh mtime, epoch mtime, missing.
# The "link" is cp; the input in.o; the output out.
set -u
printf 'out: in.o\n\tcp in.o out\n' > Makefile
printf 'rule link\n  command = cp $in $out\nbuild out: link in.o\n' > build.ninja
for tool in make ninja; do
  for torn in fresh epoch missing; do
    rm -f out .ninja_log .ninja_deps; echo v1 > in.o
    $tool > /dev/null 2>&1
    sleep 0.05; echo v2 > in.o            # the edit
    # the link started: an in-place update was killed in its write window
    case $torn in
      fresh) printf 'TORN' > out ;;              # a write sets mtime = now
      epoch) printf 'TORN' > out; touch -d @0 out ;;
      missing) rm -f out ;;
    esac
    r=$($tool 2>&1 | grep -c 'cp in.o out\|link')
    printf '%-6s %-8s relinks: %s\n' $tool $torn "$([ "$r" -gt 0 ] && echo yes || echo NO)"
  done
done
