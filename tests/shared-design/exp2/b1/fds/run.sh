#!/bin/bash
# b1/fds/run.sh: the one-past-the-end boundary case of the B-1 rule under
# -fdata-sections (the remedy the diagnostic names), gcc and clang, a64/x64.
ulimit -c 0; cd "$(dirname "$0")"; S=../src
for cc in gcc clang x86_64-linux-gnu-gcc "clang --target=x86_64-linux-gnu"; do
 for f in boundary.c vague.cc; do
  case $f in *.cc) c=$(echo $cc | sed 's/gcc$/g++/; s/^clang/clang++/');; *) c=$cc;; esac
  for fl in "-fPIC" "-fPIC -fdata-sections"; do
   o=out.$(echo "$cc $f $fl" | tr -c 'a-zA-Z0-9.\n' '_').o
   $c -O2 $fl -c $S/$f -o $o 2>/dev/null || { echo "compile fail $cc $f"; continue; }
   echo "$cc $f $fl: $(python3 -I ../relscan.py --label x $o | grep RULE-objects)"
  done
 done
done
find . -maxdepth 1 -name 'out.*.o' -delete
