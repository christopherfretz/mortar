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
MOLD_VERSION=2.37.1     # mold --version

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
# The user-mode emulator that runs the architecture's programs off its
# own host.
qemu() {
  case "$1" in
    i686-linux-gnu) echo qemu-i386 ;;
    *) echo "qemu-${1%%-*}" ;;
  esac
}
# No i686 gfortran: `lang/fortran-c` and the Fortran objects of
# mortar_elf's tests run on AArch64 and x86-64 only.
fortran() { [ "$1" != i686-linux-gnu ]; }

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
  echo bash ca-certificates curl git make procps python3 time xz-utils
  # The host's own toolchain: gcc, g++, gfortran, the binutils (ld.bfd).
  echo build-essential binutils gfortran
  # The pinned LLVM: clang, clang++, ld.lld, yaml2obj, llvm-readelf,
  # llvm-readobj, llvm-ar, llvm-nm (/usr/lib/llvm-22/bin).
  echo "clang-$LLVM_MAJOR" "llvm-$LLVM_MAJOR" "lld-$LLVM_MAJOR"
  echo mold qemu-user
  local t d
  for t in "${TRIPLES[@]}"; do
    d="$(dashed "$t")"
    echo "gcc-$d" "g++-$d" "binutils-$d"
    if fortran "$t"; then echo "gfortran-$d"; fi
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
  # The host's tools, unprefixed, as the link tests name them.
  need gcc g++ gfortran as ar nm objdump readelf ld.bfd
  # Every architecture's, prefixed (the host's own too: mortar_elf's tests
  # ask for `<triple>-gcc` first), its emulator, and a cross system root.
  for t in "${TRIPLES[@]}"; do
    need "$t-gcc" "$t-g++" "$t-as" "$t-ar" "$t-nm" "$t-objdump" "$t-readelf" "$t-ld.bfd"
    if fortran "$t"; then need "$t-gfortran"; fi
    if [ "$t" != "$HOST" ]; then
      need "$(qemu "$t")"
      [ -d "/usr/$t" ] || fail "missing: the system root /usr/$t"
    fi
  done
  local llvm="/usr/lib/llvm-$LLVM_MAJOR/bin"
  need "$llvm/clang" "$llvm/clang++" "$llvm/ld.lld" "$llvm/yaml2obj" "$llvm/llvm-readelf" \
    "$llvm/llvm-readobj" "$llvm/llvm-ar" "$llvm/llvm-nm"
  need mold /usr/bin/time python3 bash ps pgrep mkfifo nohup realpath git
  if [ "$FAILED" = 0 ]; then
    echo "ci-tools.sh: versions on $HOST:"
    pinned "GNU ld" "$BINUTILS_VERSION" "$(ld.bfd --version | head -1 | awk '{print $NF}')"
    pinned "GCC" "$GCC_VERSION" "$(gcc -dumpfullversion)"
    pinned "LLVM" "$LLVM_VERSION" "$("$llvm/clang" -dumpversion)"
    pinned "mold" "$MOLD_VERSION" "$(mold --version | awk '{print $2}')"
    echo "  qemu $(qemu-i386 --version | head -1 | awk '{print $3}')"
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
