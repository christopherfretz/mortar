#!/bin/bash
# Bounded, deterministic fuzz of the fields the positional replay trusts
# (state_edit fuzz SEED), seeds FIRST..LAST, by position; one line per
# seed: SEED <mutation> => ok-same | SILENT-DIFF | named: <reason> | PANIC.
# usage: h-fuzz.sh ROOT FIRST LAST
set -u
here=$(cd "$(dirname "$0")" && pwd)
root=$1; first=$2; last=$3
d="$root/fz"
for seed in $(seq "$first" "$last"); do
  mkdir -p "$d"; find "$d" -mindepth 1 -delete
  out=$(timeout 120 "$here/h-hostile.sh" "$d" fuzz "$seed" 2>&1)
  what=$(echo "$out" | grep '^fuzz ' | head -1)
  upd=$(echo "$out" | grep '^update' | head -1)
  after=$(echo "$out" | grep '^after' | head -1)
  if echo "$upd" | grep -q 'panicked'; then r="PANIC: $upd"
  elif echo "$upd" | grep -q 'failed:'; then r="named: ${upd#*failed: }"
  elif echo "$after" | grep -q 'exit=21 out=\[foo\]'; then r="ok-same"
  else r="SILENT-DIFF: $after"; fi
  echo "$seed $what => $r"
done
echo "H-FUZZ-DONE $first..$last"
