#!/bin/bash
# forms.sh OUTDIR [BATCH...]: WP-228 step 2, part A. Every neutral form of
# crates/mortar_image/src/neutral.rs placed into really linked outputs
# (emulate.sh's scenarios S2-S5, place.py --rust: the bytes are
# neutral.rs's, through its test binary) and judged by readelf,
# llvm-dwarfdump --verify, gdb, lldb and `cargo xtask plumb`, on AArch64
# (native) and x86-64 (qemu-x86_64's gdb stub, gdb-multiarch, lldb
# gdb-remote). Builds mortar (release), xtask and the test binary first.
# Batches (default: all):
#   base   -O0 (info, abbrev, line, rnglists/ranges, str_offsets, addr)
#   o2     -O2 (loc in DWARF 4, loclists in DWARF 5)
#   frame  -O2 without unwind tables (.debug_frame)
#   types  -fdebug-types-section (.debug_types in DWARF 4)
#   macro  -g3, Clang -g3 -fdebug-macro (macro; macinfo in Clang DWARF 4)
#   split  -gsplit-dwarf, the .dwo files present, the GNU name index
#          removed (IN-55 (A)): skeleton units (WP-229) and the address
#          base kept (DW_AT_addr_base, DW_AT_GNU_addr_base)
# each as <batch>-a64 and <batch>-x64. FINAL line "FORMS-DONE ...".
set -u
ulimit -c 0
HERE="$(cd "$(dirname "$0")" && pwd)"
WT="$(cd "$HERE/../../.." && pwd)"
OUT="$1"; shift
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
BATCHES="${*:-base-a64 o2-a64 frame-a64 types-a64 macro-a64 split-a64 base-x64 o2-x64 frame-x64 types-x64 macro-x64 split-x64}"
wait_load() { while [ "$(cut -d' ' -f1 /proc/loadavg | cut -d. -f1)" -ge 12 ]; do sleep 60; done; }
wait_load
(
  cd "$WT" || exit 1
  unset CARGO_TARGET_DIR
  # shellcheck disable=SC1091
  source scripts/env.sh > /dev/null
  cargo build --release -p mortar 2>&1 | tail -1
  cargo build -p xtask 2>&1 | tail -1
  cargo test -p mortar_image --lib --no-run 2>&1 | sed -n 's/.*Executable unittests src\/lib.rs (\(.*\))$/\1/p' > "$OUT/rust-bin"
) > "$OUT/build.log" 2>&1
RUST_BIN="$(cat "$OUT/rust-bin")"
[ -x "$RUST_BIN" ] || { echo "FORMS-DONE build failed (see $OUT/build.log)"; exit 1; }
T="${CARGO_TARGET_DIR_WP:-/Volumes/AgentWork/mortar-helper/.build/target-wp-154}"
mkdir -p "$OUT/bin"
ln -sf "$T/release/mortar" "$OUT/bin/ld"
export RUST_BIN MORTAR_BIN_DIR="$OUT/bin" XTASK="$T/debug/xtask"
export VARIANTS="${VARIANTS:-plain pieces addrbase v4}"
FRAME="-O2 -fno-exceptions -fno-asynchronous-unwind-tables -fno-unwind-tables"
summary=""
for b in $BATCHES; do
  wait_load
  arch=${b##*-}
  case $b in
    base-a64) cfg="gcc14-a64 clang19-a64 clang22-a64 kiln-a64"; ex="";;
    base-x64) cfg="gcc14-x64 clang19-x64 clang22-x64 kiln-x64"; ex="";;
    o2-a64) cfg="gcc14-a64 clang19-a64 clang22-a64 kiln-a64"; ex="-O2";;
    o2-x64) cfg="gcc14-x64 clang22-x64 kiln-x64"; ex="-O2";;
    frame-a64) cfg="gcc14-a64 clang19-a64 clang22-a64"; ex="$FRAME";;
    frame-x64) cfg="gcc14-x64 clang22-x64"; ex="$FRAME";;
    types-a64) cfg="gcc14-a64 clang19-a64 clang22-a64"; ex="-fdebug-types-section";;
    types-x64) cfg="gcc14-x64 clang22-x64"; ex="-fdebug-types-section";;
    macro-a64|macro-x64) cfg="gcc14-$arch"; ex="-g3";;
    split-a64|split-x64) cfg="gcc14-$arch clang22-$arch"; ex="-gsplit-dwarf";;
    *) echo "unknown batch $b" >> "$OUT/forms.log"; continue;;
  esac
  dx=0; [ "$arch" = x64 ] && dx=1
  np=0; [ "${b%%-*}" = split ] && np=1
  NO_GNU_PUB=$np EXTRA="$ex" TAGX="-$b" DEBUGGERS_X64=$dx "$HERE/emulate.sh" "$OUT/$b" $cfg > "$OUT/$b.out" 2>&1
  if [ "${b%%-*}" = macro ]; then
    # Clang's macros: -fdebug-macro (DWARF 4: .debug_macinfo)
    EXTRA="-g3 -fdebug-macro" TAGX="-$b-clang" DEBUGGERS_X64=$dx "$HERE/emulate.sh" "$OUT/$b-clang" "clang19-$arch" "clang22-$arch" > "$OUT/$b-clang.out" 2>&1
    summary="$summary $b-clang: $(tail -1 "$OUT/$b-clang.out");"
  fi
  summary="$summary $b: $(tail -1 "$OUT/$b.out");"
  echo "$b done $(date +%T)" >> "$OUT/forms.log"
done
echo "FORMS-DONE$summary"
