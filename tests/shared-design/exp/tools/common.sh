# Sourced by run.sh and the part scripts. Needs W (the work dir) and S (the
# script dir) set.
set -u
ulimit -c 0   # no core files (qemu writes qemu_*.core into the cwd otherwise)
LINKERS="bfd lld22 lld19 mold"
ARCHS="aa x64"
MORTAR=/Volumes/AgentWork/mortar-helper/.build/target-wp-110/release/mortar
X64ROOT=/usr/x86_64-linux-gnu

# cc ARCH LANG -> compiler driver
cc() {
    case "$1:$2" in
        aa:c) echo gcc ;; aa:c++) echo g++ ;; aa:f) echo gfortran ;;
        x64:c) echo x86_64-linux-gnu-gcc ;; x64:c++) echo x86_64-linux-gnu-g++ ;;
        x64:f) echo x86_64-linux-gnu-gfortran ;;
    esac
}
# ldflag LINKER -> driver flags that select it (gcc has no --ld-path: a -B
# directory holding an `ld.lld` symlink picks the lld version)
ldflag() {
    case "$1" in
        bfd) echo "-fuse-ld=bfd" ;;
        lld22) echo "-B$W/bin/l22 -fuse-ld=lld" ;;
        lld19) echo "-B$W/bin/l19 -fuse-ld=lld" ;;
        mold) echo "-fuse-ld=mold" ;;
        mortar) echo "-B$W/bin/mortar" ;;
    esac
}
# run ARCH LIBDIR PROG ARGS... : run natively or under qemu
runp() {
    local a=$1 d=$2; shift 2
    if [ "$a" = aa ]; then LD_LIBRARY_PATH=$d "$@"
    else qemu-x86_64 -L $X64ROOT -E LD_LIBRARY_PATH=$d "$@"; fi
}
# run with an extra environment variable (LD_PRELOAD etc.)
runpe() {
    local a=$1 d=$2 e=$3; shift 3
    if [ "$a" = aa ]; then env LD_LIBRARY_PATH=$d "$e" "$@"
    else qemu-x86_64 -L $X64ROOT -E LD_LIBRARY_PATH=$d -E "$e" "$@"; fi
}
ldso() {
    if [ "$1" = aa ]; then echo /lib/ld-linux-aarch64.so.1
    else echo "qemu-x86_64 -L $X64ROOT $X64ROOT/lib/ld-linux-x86-64.so.2"; fi
}
# normalise addresses for cross-linker comparisons
norm() { sed -E 's/0x[0-9a-f]+/ADDR/g; s/[Pp]rocess [0-9]+/process PID/g; s/pid = [0-9]+/pid = PID/g; s/\[New Thread [^]]*\]/[New Thread]/; s/LWP [0-9]+/LWP N/g; s/Thread [0-9]+\.[0-9]+/Thread N.N/g; s#'"$W"'#W#g; s#/(bfd|lld22|lld19|mold)\b#/LK#g'; }
