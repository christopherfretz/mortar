#!/bin/bash
# WP-249 step 1, item 1: which compilers emit a reference from a non-alloc
# section into a non-alloc member of a COMDAT group, and which of those
# name a copy the link drops. Compiles src/{a,b,c}.{c,cc} for each
# (compiler, target, language, flag set, DWARF version) and runs
# classes.py link on a b c in that order.
#   survey.sh OUT        writes OUT/survey.tsv, OUT/refused.txt; last line SURVEY DONE
ulimit -c 0
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$1"; [ -n "$OUT" ] || { echo "usage: survey.sh OUT"; exit 2; }
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
KILN="${MASON_KILN:-/Volumes/AgentWork/mortar-helper/.build/target-kiln-next/release/kiln++}"
HOD="${MASON_HOD:-/Volumes/AgentWork/mortar-helper/.build/target-hod-a68/release/hod}"
: > "$OUT/survey.tsv"; : > "$OUT/refused.txt"; : > "$OUT/groups.tsv"
cc_of() { # compiler target lang
  local x= tr=aarch64; case $2 in x64) x=x86_64-linux-gnu-; tr=x86_64;; x86) x=i686-linux-gnu-; tr=i686;; esac
  # i386: GCC only (free; not a priority target)
  [ $2 = x86 ] && [ $1 != gcc14 ] && return
  case $1-$3 in
    gcc14-c) echo "${x}gcc-14";;
    gcc14-cxx) echo "${x}g++-14";;
    clang19-c) echo "clang-19 --target=$tr-linux-gnu";;
    clang19-cxx) echo "clang++-19 --target=$tr-linux-gnu";;
    clang22-c) echo "clang-22 --target=$tr-linux-gnu";;
    clang22-cxx) echo "clang++-22 --target=$tr-linux-gnu";;
    kiln-cxx) echo "env KILN_HOD=$HOD $KILN -fkiln-backend=hod --target=$tr-linux-gnu";;
  esac
}
# flag sets; Clang emits macros only with -fdebug-macro (kiln refuses it: -g3 alone)
flags_of() { # compiler set
  local m=; case $1 in clang*) m=" -fdebug-macro";; esac
  case $2 in
    g) echo "-O0 -g";;
    g3) echo "-O0 -g3$m";;
    types) echo "-O0 -g -fdebug-types-section";;
    g3types) echo "-O0 -g3$m -fdebug-types-section";;
    split) echo "-O0 -g -gsplit-dwarf";;
    g3split) echo "-O0 -g3$m -gsplit-dwarf";;
    g3gz) echo "-O0 -g3$m -gz";;
    g3O2) echo "-O2 -g3$m -ffunction-sections -fdata-sections";;
  esac
}
for comp in gcc14 clang19 clang22 kiln; do
 for tgt in a64 x64 x86; do
  for lang in c cxx; do
   cc="$(cc_of $comp $tgt $lang)"; [ -n "$cc" ] || continue
   ext=c; [ $lang = cxx ] && ext=cc
   for set in g g3 types g3types split g3split g3gz g3O2; do
    for v in 4 5; do
      cfg="$comp-$tgt-$lang-$set-dw$v"
      d="$OUT/obj/$cfg"; mkdir -p "$d"
      ok=1
      for f in a b c; do
        (cd "$d" && $cc $(flags_of $comp $set) -gdwarf-$v -c "$HERE/src/$f.$ext" -o $f.o) > "$d/$f.err" 2>&1 || ok=0
      done
      if [ $ok = 0 ]; then
        echo "$cfg $(head -1 "$d/a.err" "$d/b.err" | grep -v '^==' | head -1)" >> "$OUT/refused.txt"
        continue
      fi
      python3 -I "$HERE/classes.py" link "$cfg" "$d/a.o" "$d/b.o" "$d/c.o" >> "$OUT/survey.tsv"
      # the non-alloc members of COMDAT groups, by name, per configuration
      python3 -I "$HERE/classes.py" groups "$d/a.o" "$d/b.o" "$d/c.o" | awk -F'\t' -v c="$cfg" \
        '$3 % 2 == 1 { n = split($4, m, " "); for (i = 1; i <= n; i++) { sub(/:[0-9]*$/, "", m[i]); if (m[i] ~ /^\.debug|^\.zdebug/) k[m[i]]++ } }
         END { s = ""; for (x in k) s = s " " x "=" k[x]; print c "\t" (s == "" ? " -" : s) }' >> "$OUT/groups.tsv"
    done
   done
  done
 done
done
echo "configs refused: $(wc -l < "$OUT/refused.txt"); rows: $(wc -l < "$OUT/survey.tsv")"
echo "SURVEY DONE"
