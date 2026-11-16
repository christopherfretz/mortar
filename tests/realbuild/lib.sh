# The real-builds lane's project library (spec/harness.md "realbuild",
# "A project's script"). `cargo xtask realbuild` runs
#
#   bash tests/realbuild/run.sh <project script> build|test
#
# in the project's build directory with:
#
#   RB_SRC      the unpacked sources (a fresh copy for this build)
#   RB_EXTRA    the second archive of the project, unpacked, if it has one
#   RB_BUILD    the build directory (empty at `build`; out of tree where
#               the project's build system allows it)
#   RB_ROW      gcc | clang (the compilers of the row)
#   CC CXX FC   the row's compilers; CFLAGS CXXFLAGS FFLAGS FCFLAGS "-g -O2"
#   RB_TEST_JOBS  the job count of a test runner that is not a jobserver
#               client (ctest), at most 6
#   RB_RESULTS  the file the tests' results go to, one `<name> TAB
#               PASS|FAIL|SKIP` per line
#   MAKEFLAGS   the machine's jobserver: make takes its parallelism from it
#               (never pass -j to make)
#
# A project script defines `rb_build` and `rb_test`. Nothing in it passes
# a linker: the wrapper (tests/realbuild/ld) is found as `ld` through
# COMPILER_PATH and PATH. No project is patched.

set -o pipefail

# rb_configure <command>...: the project's configuration step. The links
# it makes (autoconf's and CMake's checks, a hand-written configure's
# probes) are recorded as the `configure` phase: probes, counted apart.
rb_configure() {
  local was=$RB_PHASE st
  export RB_PHASE=configure
  "$@"; st=$?
  export RB_PHASE=$was
  return $st
}

# rb_result <name> <PASS|FAIL|SKIP>
rb_result() {
  printf '%s\t%s\n' "$1" "$2" >> "${RB_RESULTS:?}"
}

# rb_run <name> <command>...: one test, passed when the command exits 0
# (a time limit of 10 minutes).
rb_run() {
  local name=$1; shift
  if timeout 600 "$@"; then rb_result "$name" PASS; else rb_result "$name" FAIL; fi
}

# rb_ctest <build dir> [ctest arguments]: ctest at RB_TEST_JOBS, each
# test's result from its JUnit file.
rb_ctest() {
  local dir=$1; shift
  (cd "$dir" && ctest -j "${RB_TEST_JOBS:-4}" --timeout 900 --output-junit rb-ctest.xml "$@") || true
  python3 -I - "$dir/rb-ctest.xml" >> "${RB_RESULTS:?}" <<'EOF'
import sys, xml.etree.ElementTree as ET
try:
    root = ET.parse(sys.argv[1]).getroot()
except Exception as e:
    print(f"ctest\tFAIL")
    sys.exit(0)
for case in root.iter("testcase"):
    status = case.get("status", "")
    name = case.get("name", "?").replace("\t", " ")
    if status == "run" and case.find("failure") is None:
        word = "PASS"
    elif status in ("notrun", "disabled"):
        word = "SKIP"
    else:
        word = "FAIL"
    print(f"{name}\t{word}")
EOF
}

# rb_automake <dir>: the results automake's test driver left (`*.trs`)
# after `make check` (run with -k by the script).
rb_automake() {
  local dir=$1 trs result name
  while IFS= read -r -d '' trs; do
    result=$(sed -n 's/^:test-result: //p' "$trs" | head -n 1)
    name=${trs#"$dir"/}; name=${name%.trs}
    case $result in
      PASS|XFAIL) rb_result "$name" PASS ;;
      SKIP) rb_result "$name" SKIP ;;
      *) rb_result "$name" FAIL ;;
    esac
  done < <(find "$dir" -name '*.trs' -print0 | sort -z)
}
