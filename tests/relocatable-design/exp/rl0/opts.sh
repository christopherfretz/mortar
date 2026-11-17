#!/bin/bash
# rl0/opts.sh WORK: the accepted rows of docs/status/options.md that
# spec/cli.md C-R2/C-R2a/C-R3/C-R4 do not name, run at -r by GNU ld 2.44
# (and lld 22 for the lld-only ones) over the case-1 and case-8 objects of
# run.sh (WORK must hold them), each output compared byte for byte with the
# plain -r. Log: tests/relocatable-design/work/rl0-opts.log.
set -u; ulimit -c 0
WORK=$(realpath -m "$1"); cd "$WORK" || exit 1
mkdir -p opts; IN="m1.o m2.o d1.o d2.o"
ld.bfd -r -o opts/base.bfd.o $IN || exit 1
ld.lld-22 -r -o opts/base.lld.o $IN || exit 1
one() { # one LINKER TAG ARGS...
  local l=$1 t=$2; shift 2; local o=opts/$t.$l.o msg rc
  case $l in bfd) msg=$(ld.bfd -r -o $o "$@" $IN 2>&1);; lld) msg=$(ld.lld-22 -r -o $o "$@" $IN 2>&1);; esac; rc=$?
  if [ $rc != 0 ]; then echo "$l	$*	rc=$rc	$(echo "$msg" | head -1 | cut -c1-110)"; return; fi
  if cmp -s $o opts/base.$l.o; then r=identical; else r="DIFFERS"; fi
  echo "$l	$*	$r${msg:+	[$(echo "$msg" | head -1 | cut -c1-90)]}"
}
for a in --strip-discarded --discard-none --no-gc-sections --print-gc-sections --no-print-gc-sections \
  --gc-keep-exported --no-copy-dt-needed-entries --no-add-needed --sysroot=/ -rpath-link=/nonexistent \
  --require-defined=g1 -init=g1 -fini=g2 --no-export-dynamic -Bno-symbolic --error-limit=5 \
  --fatal-warnings --no-fatal-warnings -w --no-warnings --color-diagnostics --no-color-diagnostics \
  --allow-multiple-definition --no-allow-multiple-definition --no-warn-common --warn-execstack \
  --detect-odr-violations --dependency-file=opts/dep.d -plugin-save-temps -flto -flto-partition=one \
  -fuse-ld=bfd -g -Qy -qmagic "-assert definitions" --map-whole-files --no-map-whole-files \
  --no-keep-memory --reduce-memory-overheads --hash-size=31 --max-cache-size=1000 --no-eh-frame-hdr \
  --no-gdb-index --no-debug-names --undefined-version --no-undefined-version --rosegment \
  --start-lib --end-lib --warn-backrefs --no-warn-backrefs --no-dependent-libraries --time-trace \
  -M --verbose --no-incremental --gdb-index --no-relax --emit-relocs ; do
  for l in bfd lld; do one $l "$(echo $a | tr -c 'a-zA-Z0-9\n' _)" $a; done
done
# --build-id at -r: which references write the note (P-P10, A98)
for l in bfd lld mold; do
  case $l in bfd) ld.bfd -r --build-id -o opts/bid.$l.o $IN;; lld) ld.lld-22 -r --build-id -o opts/bid.$l.o $IN;; mold) mold -r --build-id -o opts/bid.$l.o $IN;; esac
  echo "$l	--build-id	rc=$?	$(readelf -SW opts/bid.$l.o | grep -c 'note.gnu.build-id') build-id note(s)"
done
echo "opts done"
