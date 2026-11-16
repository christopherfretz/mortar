#!/bin/bash
# bash tests/realbuild/run.sh <project script> build|test
# (spec/harness.md "realbuild"): sources the lane's library and the
# project's script, then runs its `rb_build` or `rb_test` in $RB_BUILD.
set -u
here=${BASH_SOURCE%/*}
. "$here/lib.sh"
. "${1:?a project script}"
cd "${RB_BUILD:?}" || exit 2
export RB_PHASE=${2:-}
case ${2:-} in
  build) set -e; rb_build ;;
  test) rb_test ;;
  *) echo "run.sh: build or test" >&2; exit 2 ;;
esac
