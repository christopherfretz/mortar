#!/usr/bin/env bash
# The reference tools mortar's gate needs, in one place (spec/harness.md
# "Hosted CI"): the Debian 13 (trixie) packages that give the reference
# linkers (GNU ld, lld, mold), the compilers that make their inputs (GCC
# with g++ and gfortran, Clang), the binutils and LLVM tools that check
# their outputs, the GNU toolchain and system root of the two other
# architectures, qemu-user to run their programs, and what the harness
# itself runs (GNU time, python3 and procps for scripts/jobserver.sh).
#
#   scripts/ci-tools.sh            install what is missing (root, apt), then --check
#   scripts/ci-tools.sh --check    install nothing: every package installed, every
#                                  tool found, the pinned versions; exit 1 otherwise
#   scripts/ci-tools.sh --list     the packages for this host, one per line
#
# The same list on an AArch64 and an x86-64 host: the `<tool>-<triple>`
# packages are the native compiler under its triplet name for the host's
# own architecture, and the cross compiler for the others. Idempotent; run
# by .github/actions/mortar-ci and usable in any Debian 13 container. With
# every package of this list, the gate passes under MORTAR_REQUIRE_TOOLS=1,
# which makes a missing tool a failure, never a skip (docs/VALIDATION.md
# "Oracles"); a test that needs a tool this list does not give fails the
# hosted lanes, so the list cannot fall behind the tests unnoticed.
#
# The versions are pinned because tests and specs record the behavior of
# these releases (spec/harness.md "Where the references differ",
# spec/script.md, spec/elf.md, spec/archive.md): a different release is a
# different oracle, and is re-measured before the pin moves. Debian 13's own
# packages are these releases, so no third-party archive is used.
set -euo pipefail

BINUTILS_VERSION=2.44   # ld.bfd --version
GCC_VERSION=14.2        # gcc -dumpfullversion, major.minor
LLVM_MAJOR=22           # mortar_testutil::LLVM_MAJORS[0]: the LLVM tools the tests run
LLVM_VERSION=22.1.8     # clang-22 -dumpversion
DEBIAN_LLVM_MAJOR=19    # Debian 13's own LLVM: lldb (find_tool tries 22, then 19), the
                        # clang-19 of the compiler censuses, the ld.lld of gcc -fuse-ld=lld
DEBIAN_LLVM_VERSION=19.1.7  # lldb-19 --version, clang-19 -dumpversion, ld.lld --version
MOLD_VERSION=2.37.1     # mold --version
GDB_VERSION=16.3        # gdb --version
ELFUTILS_VERSION=0.192  # eu-elflint --version
QEMU_VERSION=10.0.13    # qemu-<arch> --version

# The GNU triples of the three architectures (xtask::linktest::target::Arch),
# Debian's dashed package spelling of each, and its Debian architecture.
TRIPLES=(aarch64-linux-gnu x86_64-linux-gnu i686-linux-gnu)
dashed() { echo "${1//_/-}"; }
debian_arch() {
  case "$1" in
    aarch64-linux-gnu) echo arm64 ;;
    x86_64-linux-gnu) echo amd64 ;;
    i686-linux-gnu) echo i386 ;;
  esac
}

# The tool guard (spec/harness.md "Hosted CI", "The tool guard"): every
# name a test or the link-test harness looks a tool up by
# (mortar_testutil::tool, mortar_testutil::find_listed_tool) must be
# covered by one of the four lists below, or that lookup fails, on every
# machine and whatever MORTAR_REQUIRE_TOOLS says;
# mortar_testutil::ci_tools reads them from this file. A name is covered
# when it is in TOOLS; or is `<triple>-<t>` with <triple> one of TRIPLES
# and <t> in TRIPLE_TOOLS; or is in LLVM_TOOLS, bare or with a `-<major>`
# suffix; or is in SETUP_TOOLS. A new tool goes into a list here (and its
# package into packages() below) in the change that first asks for it.
# Each list is one line, `NAME=(...)`, of plain words.
#
# On PATH, by these names, on both hosts:
TOOLS=(gcc g++ gfortran as ar nm objdump objcopy strip readelf ld ld.bfd mold gdb gdb-multiarch eu-elflint eu-strip file ldd qemu-aarch64 qemu-x86_64 qemu-i386 python3 bash git dpkg-query sha1sum sha256sum mkfifo nohup realpath ps pgrep)
# `<triple>-<tool>` for each of the three TRIPLES (the host's own too):
TRIPLE_TOOLS=(gcc g++ gfortran as ar nm objdump objcopy readelf ld ld.bfd)
# The pinned LLVM's, /usr/lib/llvm-$LLVM_MAJOR/bin/<tool> (lldb: Debian's own,
# DEBIAN_LLVM_MAJOR's); mortar_testutil::find_tool finds them as
# <tool>-<major> or there. clang-19 and clang++-19 are Debian's (the
# censuses), and so is the bare ld.lld on PATH, which gcc -fuse-ld=lld runs:
LLVM_TOOLS=(clang clang++ ld.lld yaml2obj llvm-readelf llvm-readobj llvm-ar llvm-nm llvm-objcopy llvm-strip llvm-dwarfdump llvm-mc lldb llvm-dwp)
# Given by the job's Rust set-up (.github/actions/mortar-ci), not by apt:
SETUP_TOOLS=(rustc)
# Looked up only when a listed name is missing (mold as ld.mold, a
# static-only qemu): covered, never checked here.
FALLBACK_TOOLS=(ld.mold qemu-aarch64-static qemu-x86_64-static qemu-i386-static)

# The user-mode emulator that runs the architecture's programs off its
# own host.
qemu() {
  case "$1" in
    i686-linux-gnu) echo qemu-i386 ;;
    *) echo "qemu-${1%%-*}" ;;
  esac
}

host_arch() {
  case "$(uname -m)" in
    aarch64) echo aarch64-linux-gnu ;;
    x86_64) echo x86_64-linux-gnu ;;
    *) echo "ci-tools.sh: unsupported host architecture $(uname -m) (AArch64 or x86-64)" >&2
       exit 2 ;;
  esac
}
HOST="$(host_arch)"

packages() {
  # What the job and the harness run, beyond the tools under test.
  echo bash ca-certificates coreutils curl git make procps python3 time xz-utils
  # The host's own toolchain: gcc, g++, gfortran, the binutils (ld.bfd).
  echo build-essential binutils gfortran
  # The pinned LLVM (/usr/lib/llvm-22/bin: clang, ld.lld, yaml2obj, the
  # llvm- tools).
  echo "clang-$LLVM_MAJOR" "llvm-$LLVM_MAJOR" "lld-$LLVM_MAJOR"
  # Debian's own LLVM: its lldb, its clang (the compiler censuses), and its
  # lld as the ld.lld on PATH (gcc -fuse-ld=lld).
  echo "clang-$DEBIAN_LLVM_MAJOR" "lldb-$DEBIAN_LLVM_MAJOR" lld
  echo mold qemu-user
  # The debuggers of the debugger tests and the link tests' `check gdb`
  # (gdb-multiarch reads the other architectures' programs), elfutils'
  # eu-elflint, file, and ldd (libc-bin, in every Debian).
  echo gdb gdb-multiarch elfutils file libc-bin
  local t d
  for t in "${TRIPLES[@]}"; do
    d="$(dashed "$t")"
    echo "gcc-$d" "g++-$d" "gfortran-$d" "binutils-$d"
    # The system root of a cross target, /usr/<triple> (the cross
    # compilers only recommend the C library).
    if [ "$t" != "$HOST" ]; then
      echo "libc6-dev-$(debian_arch "$t")-cross" "libstdc++-14-dev-$(debian_arch "$t")-cross"
    fi
  done
}

mapfile -t PACKAGES < <(packages | tr ' ' '\n')

installed() {
  dpkg-query -W -f='${Status}' "$1" 2>/dev/null | grep -q "install ok installed"
}

FAILED=0
fail() { echo "ci-tools.sh: $*" >&2; FAILED=1; }

# A command on PATH, or an absolute path that is an executable file.
need() {
  local tool
  for tool in "$@"; do
    case "$tool" in
      /*) [ -x "$tool" ] || fail "missing: $tool" ;;
      *) command -v "$tool" >/dev/null || fail "missing: $tool" ;;
    esac
  done
}

# `want` is the pinned version, `got` what the tool says.
pinned() {
  local what="$1" want="$2" got="$3"
  case "$got" in
    "$want" | "$want".*) echo "  $what $got" ;;
    *) fail "$what is $got, pinned to $want (re-measure before moving the pin)" ;;
  esac
}

check() {
  local p t
  for p in "${PACKAGES[@]}"; do
    installed "$p" || fail "package not installed: $p"
  done
  need "${TOOLS[@]}"
  for t in "${TRIPLES[@]}"; do
    need "${TRIPLE_TOOLS[@]/#/$t-}"
    if [ "$t" != "$HOST" ]; then
      [ -d "/usr/$t" ] || fail "missing: the system root /usr/$t"
    fi
  done
  local llvm="/usr/lib/llvm-$LLVM_MAJOR/bin" lldb="/usr/lib/llvm-$DEBIAN_LLVM_MAJOR/bin/lldb" l
  for l in "${LLVM_TOOLS[@]}"; do
    if [ "$l" = lldb ]; then need "$lldb"; else need "$llvm/$l"; fi
  done
  # What the harness itself runs.
  need /usr/bin/time "clang-$DEBIAN_LLVM_MAJOR" "clang++-$DEBIAN_LLVM_MAJOR" ld.lld
  if [ "$FAILED" = 0 ]; then
    echo "ci-tools.sh: versions on $HOST:"
    pinned "GNU ld" "$BINUTILS_VERSION" "$(ld.bfd --version | head -1 | awk '{print $NF}')"
    pinned "GCC" "$GCC_VERSION" "$(gcc -dumpfullversion)"
    pinned "LLVM" "$LLVM_VERSION" "$("$llvm/clang" -dumpversion)"
    pinned "mold" "$MOLD_VERSION" "$(mold --version | awk '{print $2}')"
    pinned "LLDB" "$DEBIAN_LLVM_VERSION" "$("$lldb" --version | head -1 | awk '{print $3}')"
    pinned "clang-$DEBIAN_LLVM_MAJOR" "$DEBIAN_LLVM_VERSION" "$("clang-$DEBIAN_LLVM_MAJOR" -dumpversion)"
    pinned "ld.lld" "$DEBIAN_LLVM_VERSION" "$(ld.lld --version | awk '{print $3}')"
    pinned "gdb" "$GDB_VERSION" "$(gdb --version | head -1 | awk '{print $NF}')"
    pinned "gdb-multiarch" "$GDB_VERSION" "$(gdb-multiarch --version | head -1 | awk '{print $NF}')"
    pinned "elfutils" "$ELFUTILS_VERSION" "$(eu-elflint --version | head -1 | awk '{print $NF}')"
    for t in "${TRIPLES[@]}"; do
      pinned "$(qemu "$t")" "$QEMU_VERSION" "$("$(qemu "$t")" --version | head -1 | awk '{print $3}')"
    done
  fi
  if [ "$FAILED" != 0 ]; then
    echo "ci-tools.sh: the reference tools are incomplete (scripts/ci-tools.sh installs them)" >&2
    return 1
  fi
  echo "ci-tools.sh: every reference tool is here (${#PACKAGES[@]} packages)"
}

case "${1:-install}" in
  --list) printf '%s\n' "${PACKAGES[@]}" ;;
  --check) check ;;
  install)
    missing=()
    for p in "${PACKAGES[@]}"; do installed "$p" || missing+=("$p"); done
    if [ ${#missing[@]} -gt 0 ]; then
      echo "ci-tools.sh: installing ${missing[*]}"
      export DEBIAN_FRONTEND=noninteractive
      apt-get update -qq
      apt-get install -y -qq --no-install-recommends "${missing[@]}" >/dev/null
    fi
    check
    ;;
  *) echo "usage: $0 [--check | --list]" >&2; exit 2 ;;
esac
