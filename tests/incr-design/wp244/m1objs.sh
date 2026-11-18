#!/bin/bash
# WP-244 (WP-239's m1objs.sh, unchanged): the M.1 series' objects (review-in72's m/pb/build.sh recipes:
# protobuf 3.21.12 stubs, strutil.cc and its four edits, main.cc) for
# another compiler, e.g. the x86-64 cross compiler.
#
#   m1objs.sh CXX "FLAGS" OUTDIR
set -eu
cxx=$1; fl=$2; out=$3
S=/Volumes/AgentWork/mortar-helper/.build/bench-work/src/4eab9b524aa5913c/protobuf-3.21.12/src
mkdir -p "$out"; cd "$out"
for f in bytestream common int128 status statusor stringpiece stringprintf structurally_valid substitute; do
	[ -f $f.o ] || $cxx $fl -std=c++14 -I$S -DHAVE_PTHREAD -c $S/google/protobuf/stubs/$f.cc -o $f.o
done
base=$S/google/protobuf/stubs/strutil.cc
mk() { n=$1; shift; sed "$@" $base > strutil_$n.cc; $cxx $fl -std=c++14 -I$S -I$S/google/protobuf/stubs -DHAVE_PTHREAD -c strutil_$n.cc -o strutil_$n.o; }
mk base -e ''
mk grow1 -e '/^void StripWhitespace(std::string \*str) {$/a\
  if (str->empty()) return;\
  if (str->size() == 1 \&\& (*str)[0] == 0) str->clear();'
mk grow2 -e '/^void StripWhitespace(std::string \*str) {$/a\
  if (str->empty()) return;\
  if (str->size() == 1 \&\& (*str)[0] == 0) str->clear();\
  if (str->size() == 2 \&\& (*str)[1] == 0) str->resize(1);\
  if (str->size() == 3 \&\& (*str)[2] == 0) str->resize(2);'
mk addfn -e '$a\
namespace google { namespace protobuf { int Rv72Added(int x) { static const char *m[] = {"rv72-a","rv72-b"}; return x + m[x\&1][0]; } } }'
mk rmfn -e 's/^bool safe_strtof(const char\* str, float\* value) {$/static bool rv72_removed(const char* str, float* value) {/'
printf '#include <string>\nnamespace google { namespace protobuf { std::string SimpleItoa(int); } }\nint main(int argc, char**) { return google::protobuf::SimpleItoa(argc).size() == 1 ? 0 : 1; }\n' > main.cc
$cxx $fl -std=c++14 -c main.cc -o main.o
echo "m1objs done"
