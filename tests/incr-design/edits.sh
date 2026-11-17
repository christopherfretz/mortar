#!/bin/sh
# WP-202: a handful of real edits to one protoc source (stubs/strutil.cc),
# compiled with GCC and Clang, objects only (no link), each classified by
# elfdiff.py against WP-064a §3's refused-edit list.
#
#   edits.sh SRCROOT BUILDDIR SCRATCH
#
# SRCROOT is protobuf-3.21.12/src, BUILDDIR the protoc build directory
# (its config header), SCRATCH an empty directory this script fills.
set -eu
src=$1/google/protobuf/stubs/strutil.cc
inc="-I$2 -I$1 -DGOOGLE_PROTOBUF_CMAKE_BUILD -std=c++11"
out=$3
here=$(cd "$(dirname "$0")" && pwd)

mk() { # name, then a sed program applied to the base source
	name=$1
	shift
	mkdir -p "$out/$name"
	sed "$@" "$src" >"$out/$name/strutil.cc"
}
mk base -e ''
# e2's class: two statements added at the top of an early function.
mk grow -e '/^void StripWhitespace(std::string \*str) {$/a\
  if (str->empty()) return;\
  if (str->size() == 1 && (*str)[0] == 0) str->clear();'
# a merged string changed, same length (s4/W6's class).
mk string -e 's/"Error reporting not implemented."/"Error reporting not supported.."/'
# a static function made global (W10).
mk l2g -e 's/^static int memcasecmp(/int memcasecmp(/'
# a static helper added and called (a local appears: .symtab's local block).
mk local -e '/^void StripWhitespace(std::string \*str) {$/i\
static int wp202_helper(int x) { return x * 3 + 1; }' \
	-e '/^void StripWhitespace(std::string \*str) {$/a\
  if (wp202_helper(int(str->size())) == 0) return;'
# a global function added at the end (a key appears).
mk global -e '$a\
namespace google { namespace protobuf { int Wp202Added(int x) { return x + 1; } } }'
# a comment line at the top (e3's class: __LINE__ in GOOGLE_CHECK).
mk comment -e '1i\
// WP-202'

for cfg in "g++ -O0 -g" "clang++ -O0 -g" "g++ -O2 -g -ffunction-sections -fdata-sections" "clang++ -O2 -g -ffunction-sections -fdata-sections"; do
	tag=$(echo "$cfg" | tr -d ' +' | tr -c 'a-zA-Z0-9\n' '_')
	for e in base grow string l2g local global comment; do
		# One file name and one directory name in every variant, so that
		# __FILE__ and the DWARF names do not differ by the variant's name.
		(cd "$out/$e" && $cfg $inc -fdebug-prefix-map="$out/$e=/src" -c strutil.cc -o "../$e.$tag.o")
	done
	for e in grow string l2g local global comment; do
		printf '%s %s: %s\n' "$cfg" "$e" "$(python3 "$here/elfdiff.py" diff "$out/base.$tag.o" "$out/$e.$tag.o")"
	done
done
