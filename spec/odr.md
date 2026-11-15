# spec: one-definition-rule checks (`mortar_odr`)

**Status: draft (WP-070, revision 1 after the adversarial review
`briefs/reviews/review-070-meta-odr.md` and the coordinator's rulings;
awaiting the operator's decisions of §15).** Rules are `O-n`. The
container they read is `spec/meta.md` (`MT-n`). Every statement about
what a compiler, linker or loader does names its experiment (Appendix A;
scripts in the WP-070 review directory, record in
`spec/landed/meta/WP-070.md`); statements about C++ cite the standard
from knowledge and are marked `[std, unverified clause]` where the clause
number was not checked against the text. `[unverified]` marks what no
experiment showed and `[estimate]` every number no script printed. Tags
`[M6]` (default) and `[M4]` (shared-object output).

**Numbering.** Rules are numbered by section; the gaps are reserved for
later rules of the same section and are never reused: O-6..9 (§3-§4),
O-26..29 (§7), O-36..39 (§8), O-48..49 (§9), O-60..69 (§10). Catalogue
numbers K3, K10 and K15 were retired while the first draft was written
(their cases merged into K2, K9 and K14) and are not reused. Every rule
has a test, listed in §12.6.

## 1. Scope and guarantees

`mortar_odr` (depends on `mortar_core` and `mortar_meta`) finds programs
that break the one-definition rule across translation units
([basic.def.odr], "ill-formed, no diagnostic required") and the
neighbouring hazards a linker resolves silently today: two definitions of
one entity that differ, one declaration that does not match its
definition, translation units compiled for incompatible ABIs. It reports
them as diagnostics of phase 40 (`spec/model.md` §8). It never changes
the output: a link that succeeds with the checks on produces the same
bytes as with them off (`--odr=off`, O-41), except the loadable table and
the runtime object (§10) when the table is on (the runtime is an input,
so it shifts every later `FileId` and address).

Evidence comes in two strengths:

- **records** (`meta.md` kinds 1-5) written by kiln for each TU: a
  definition hash per entity, layout per type, the declared type of
  every external reference. They make a check a proof;
- **foreign facts** that any object has: symbol sizes, types, bindings
  and COMDAT shape (§8). They catch a few kinds and prove only what they
  see.

**O-1 (the bar).** A check is **on by default only if it has been run**
on the legal corpus (§12.2) and the false-positive oracle (§12.3) **and
found zero findings** there. A check that cannot yet be run (it needs
kiln's hasher, K-3) or whose false-positive rate is unmeasured runs only
under `--odr=strict` until its corpus exists. A check that is on by
default never reports on a legal program. A **missed violation is a
documented exclusion** (X1-X12 below); a **false positive is a defect**:
the check is fixed or moved back to `--odr=strict`, and the program joins
the legal corpus (§3, §12.2). O-47 is the register of which checks are on
today and why; O-46 is how a check moves.

**The guarantee.** Assume

- **A1** every TU that defines or declares entity `E` was compiled by
  kiln with `-fmason-metadata` at the default level or above, with one
  `odr.scheme` (O-11), and not with `-flto` (kiln's bitcode objects carry
  no unit, X10);
- **A2** the objects were changed after compilation only by the tools and
  options that `meta.md` MT-5 lists as keeping both the container and the
  symbols (`ar`, `ld -r` by any of the three references, `strip -g`/`-x`/
  `--strip-unneeded`, the listed `objcopy` options); in particular not by
  `llvm-strip`, `llvm-objcopy --strip-non-alloc`, `eu-strip` (they remove
  the container) or a default `strip` (it removes the symbols the rows
  anchor to);
- **A3** kiln's hasher meets O-13 (its must-equal and must-differ tests);
- **A4** the link runs with `--odr=strict`, or with the default once O-47
  lists the check as on.

Then every violation of kinds K1, K2, K4-K9, K11, K13-K14b, K16a, K16b,
K19 of §2 **between definitions emitted as symbols in at least two live
files** of a static link, and between a dynamic output and a shared-object
input that carries a table (O-24), is reported, by the check named in §2.
A violation is missed only if two different definitions have equal
128-bit hashes (O-12; about 2^-128 per pair, no adversary assumed), or if
it is one of these **exclusions**:

- **X1** a definition inlined in every TU that uses it (no symbol): its
  row exists only at `-fmason-metadata=all` (`INLINED_ONLY`) and is
  compared only under `--odr=strict` (O-4). K1, K2, K8 and K11 at `-O2`
  are often of this shape.
- **X2** a variable whose every use was constant-folded, so that no TU
  emits it (K9's folded uses): no symbol, no row.
- **X3** K16c in the catalogue's shape, where only one TU emits the
  vtable (the other uses the type's `type_info` through `dynamic_cast`):
  O-25 compares two COMDAT copies of one `_ZTV` only. The model shows the
  miss and an extension that would catch it (Q16).
- **X4** a type reached in some TU only through a pointer or reference to
  it while it is incomplete there: that TU has no definition to hash, so
  only the TUs that define it are compared (O-17).
- **X5** K12 (`USES_INTERNAL`) by default: `odr-internal-reference` is
  `--odr=strict` only (O-16; libstdc++'s own `std::mutex` sets it).
- **X6** differences in `SYSTEM` rows between units of different `-std`:
  counted by `--odr-stats`, never reported (O-32).
- **X7** C translation units: no type rows at all, and C-linkage
  declaration mismatches only under `--odr=strict` (O-33).
- **X8** a TU whose container or symbols were lost (A2's tools): its rows
  are absent or unanchored; nothing reports the loss (MT-5).
- **X9** objects without records (GCC, Clang, foreign archives): only the
  W checks of §8 apply (best effort, below).
- **X10** `-flto` objects (bitcode) carry no unit.
- **X11** K16d, K16e (ISA and struct-return conventions): kind 1 notes
  (O-30, O-31), `--odr=strict` only until their corpus exists.
- **X12** K17 (a definition in an archive member that is not linked) is
  reported only on request (O-23); K18 between shared objects at run time
  only by the hook (§10.3), which is off by default (O-58).

**Best effort (no proof).** Objects without records: W1 and W2 of §8 are
on by default and catch K5 and K6 when two copies' sizes differ; nothing
else measured. Mixed links (one side with records): O-34.

**What is on by default today.** Following O-1, only checks that have run
on a corpus: W1, W2, `meta-malformed`, `meta-version` (O-47). Every
record-based check is `--odr=strict` until kiln emits records and the
oracle of §12.3 runs it at zero. The guarantee above is therefore, today,
the guarantee of `--odr=strict`; it becomes the default check by check.

## 2. The catalogue (measured)

Each case is two TUs `a.cpp` (defines `fa`) and `b.cpp` (defines `fb`)
sharing a header, compiled with g++ 14.2 and clang++ 19.1.7, linked with
GNU ld 2.44, lld 19.1.7 and mold 2.37.1 in the orders `a,b` and `b,a`,
and run (`catalogue`, `catalogue2`, `k19`; AArch64 host, x86 cases under
qemu). "Weak" is what the foreign checks of §8 see on the same objects
(`weak-catalogue`). "Today" holds for all three linkers and both
compilers unless stated: no linker printed anything for any case. The
last column is the check that proves it and its record; the records of
K4 (pointer-only use), K7 and K16c were constructed by hand and checked
by the model (`model`, §12.7).

| K | kind | today (every linker, both orders) | weak checks | check that proves it |
|---|---|---|---|---|
| K1 | inline function, body differs by `-D` | silently the first object's copy for all callers: `fa=fb=1` (`a,b`), `fa=fb=2` (`b,a`) | no (code bytes differ legitimately, B1) | kind 2 hash (O-20) |
| K2 | inline function with `assert`, `NDEBUG` in one TU | `a,b` runs; `b,a`: the assertion fires in the TU that disabled it, abort (134) | no | kind 2 hash |
| K4 | class with an extra member in one TU | wrong values: `fb=` garbage (`a,b`), `fa=0` (`b,a`) | no: no symbol carries a size | kind 3 layout (O-18); also when the class is used only through `S*` and is complete in both TUs (model `K4-pointer-only`; O-17's reach); not when one TU sees it incomplete (X4) |
| K5 | class with a different set of virtual functions | ran correctly by luck; `_ZTV1B` 48 vs 40 bytes | **W1** (size), W4 | kind 3 layout (vtable shape) |
| K6 | class with different base classes | clang `a,b`: SIGBUS (135) under all three; g++ ran; `_ZTI1D` 56 vs 24, `_ZTV1D` 72 vs 40 | **W1**, W4 | kind 3 layout |
| K7 | enum with different enumerators, used only as constants in `inline int isB(int e){ return e == B; }` | wrong results, order-dependent (`fa=1 fb=0` / `fa=0 fb=1`) | no | kind 2 hash (an enumerator hashes as type, name and **value**, O-10) and kind 3 (O-17 reaches the types of expressions) — model `K7-revised`; the first draft's rules missed it (`K7-old-rules`) |
| K8 | inline function, default argument differs | `fa=1 fb=2` in every order: each call site uses its TU's default; no visible failure | no | kind 2 hash (O-10: default arguments) |
| K9 | inline variable, initializer differs | the variable takes the first object's value (`11`/`21`) while constant-folded uses keep each TU's value | W4 only (strict, §8) | kind 2 hash (the folded uses: X2) |
| K11 | lambda in an inline function, body differs | the first object's closure for all callers | no | kind 2 (the enclosing function and the closure's `operator()`) |
| K12 | unnamed namespace used by an inline function in a header | one counter for both TUs (`fa=11 fb=12`): the kept copy's | no (identical bytes) | kind 2 flag `USES_INTERNAL`, `--odr=strict` only (O-16, X5) |
| K13 | explicit specialization in one TU, implicit instantiation in another (IF-NDR) | the strong specialization wins for all callers (`fa=fb=2`) | W3 (strict only: false positives, N20-N22) | kind 2 flags (O-15), compared by O-20 although the group has one copy (O-21) |
| K14 | `extern` C++ variable declared with another type | wrong values (`fa=0`) | no: an undefined symbol has no size | kind 5 (O-35) |
| K14b | `extern "C"` function declared with another signature, both TUs C++ | wrong results (`fa=2`, expected 6) | no | kind 5; C-linkage rows are compared under C's rules and `--odr=strict` only until a C corpus exists (O-33) |
| K16a | `-fshort-enums` in one TU | garbage values | no | kind 3 (enum size) + kind 1 `abi.short-enums` note |
| K16b | `_GLIBCXX_DEBUG` in one TU (g++ only) | SIGSEGV in every order | no (10 symbols checked, none flagged) | kind 3 (user types containing containers) + kind 1 `abi.stdlib-debug` |
| K16c | `-fno-rtti` in one TU, polymorphic class | SIGSEGV in every order | no | O-25: two COMDAT copies of one `_ZTV` from units whose `abi.rtti` differ (model `K16c-two-vtable-copies`); the catalogue's own shape, with one copy, is X3 |
| K16d | inline function compiled with `-mavx2` in one TU (x86-64) | `a,b`: every linker keeps the AVX2 copy (5 `ymm` instructions in the linked `sum`) for every caller, the baseline TU's included; `b,a`: 0. A CPU without AVX2 would fault [unverified: qemu-user executes AVX2 whatever `-cpu` says] | no | kind 1 `isa` (O-31), strict until its corpus |
| K16e | `-freg-struct-return` in one TU (i386) | SIGSEGV under all three | no | kind 1 `abi.struct-return` (O-30), strict until its corpus |
| K17 | the same symbol in two archives / an object and an archive | link order picks silently: `liba,libb` 1, `libb,liba` 2, `x.o libb.a` 1 | yes, if unextracted members are read | kind 2, opt-in (O-23); legal by archive semantics |
| K18 | inline function in two shared objects, different bodies | default visibility: the first in lookup order wins for both libraries (`-lA -lB`: 1,1; `-lB -lA`: 2,2); with `-fvisibility-inlines-hidden` each keeps its own (1,2) | no | at static link time when both are inputs of one link (O-24 b); otherwise the hook (§10.3) |
| K18b | copy-relocated array whose size grows in the shared object | glibc at run time: ``Symbol `arr' has different size in shared object, consider re-linking``, then wrong values (`last=0`) | the sizes, at static link time only | kind 5 size vs `st_size` (O-24 c) |
| K18c | template static data member in a library loaded with `dlopen(RTLD_LOCAL)` | g++ binds it `STB_GNU_UNIQUE`, clang `STB_WEAK`; the library gets its own instance (sees 1) unless the executable is linked `-rdynamic` (sees 2) | — | not a source violation: one entity, two objects (`spec/dyn.md` §14; Q8) |
| K19 | modules: one header included in the global module fragments of two named modules under different `-D` | **built** (`k19`): clang 19 and 22 compile the importer without a diagnostic; both module objects emit a `WEAK` COMDAT copy of `val()`; `ma,mb`: `fa=fb=1`, `mb,ma`: `fa=fb=2` under all three linkers | no | kind 2 hash, as K1 (rows carry `GMF`); kiln's own compile-time check is K-6 |

Not built separately, same mechanism as a built case: `#pragma pack`
(K4's: layout), point-of-instantiation differences and `-std`
differences in a template body (K1's: kind 2 hash of the instantiated
definition), struct-return conventions on x86-64 and AArch64 (K16e's,
where the ABI has no such switch).

**Existing tools** (`catalogue2`):

| tool | catches here | misses here |
|---|---|---|
| GCC LTO `-Wodr` (`-flto`) | K4, K5 (vtable), K7, K16a | K1, K11 (bodies are not compared) |
| GCC LTO `-Wlto-type-mismatch` | K14, K14b | |
| GCC ASan (`detect_odr_violation`) | a strong global defined in two shared objects, also with equal sizes | the inline variable of K9 |
| Clang ASan | not verified: clang 19's runtime is not installed here | |
| gold `--detect-odr-violations` | not verified: gold is not installed (it compares DWARF line info of same-named symbols) | |
| clang modules' ODR hash | does not compare global-module-fragment definitions (K19, measured with clang 19 and 22) | |
| MSVC `/ODR`-style checks, `abidiff` | not verified: not available | |

## 3. What must not be flagged

The legal corpus. Each program links and runs under GNU ld, lld and
mold; "measured" names the experiment that ran the checks on it.

| N | legal program | why it looks like a violation | what keeps it unflagged |
|---|---|---|---|
| N1 | one inline function at `-O0` and `-O2` | different bytes (g++: 136 vs 36; clang: 140 vs 152) | the hash is of the definition before code generation (O-10); code bytes are never compared |
| N2 | `-g` and no `-g` | (g++ `-O0`: identical bytes, B1) | O-10 excludes debug information |
| N3 | GCC and Clang objects of one source | different bytes; `_ZTS*` and `_ZSt19piecewise_construct` aligned 8 by GCC, 1 by Clang; a libstdc++ vtable whose contents differ (D1/D2 destructor aliases) | alignment (W5) is not checked; contents (W4) only under strict; hashes of different schemes are never compared (O-11). Measured: protobuf's 84 TUs rebuilt round-robin with four compiler/`-O`/`-std` settings, 823 files, W1-W3 zero (`mixed-protobuf`) |
| N4 | `-fexceptions` and `-fno-exceptions` TUs mixed | flags differ | `abi.exceptions` is a note (`meta.md` MT-26) |
| N5 | a weak default overridden by a strong definition; `malloc`/`free` replaced | two definitions of one name | rows are compared only for the definitions O-14 lists; interposing strong definitions are not vague-linkage entities (measured: not flagged) |
| N6 | symbol versioning: `foo@V1` and `foo@@V2`, two definitions in one shared object | one name, two bodies | the key is the full name with its version (O-4). Built (`n6`): the object's symbols are `foo@V1`, `foo@@V2`; W1-W3 zero; a program binds `foo@@V2` |
| N7 | `gnu_inline` / `extern inline` with an out-of-line definition | two definitions (measured: not flagged) | `gnu_inline` definitions are not vague linkage; kiln writes no row for them (K-3) |
| N8 | C tentative definitions with `-fcommon`; Fortran blank COMMON of different sizes | several definitions | commons are resolution's (`spec/resolve.md` §3.3); W1-W3 zero (the review's corpus) |
| N9 | explicit instantiation declaration + definition, with an implicit copy elsewhere | a strong-looking definition and a COMDAT copy | g++ and clang emit the explicit instantiation `STB_WEAK` in a COMDAT group (`legal-fp`: "WEAK in COMDAT groups: 1" for both; the first draft's `legal.sh` printed `LOCAL` because its grep matched the section symbol); kind 2 flags `EXPLICIT_INST` and `IMPLICIT_INST` are compatible (O-15) |
| N10 | implicitly defined special members under different `-O` | different bytes | the hash is of the synthesized definition (O-10) |
| N11 | `libgcc.a` and `libgcc_s.so` in one program | 122 names defined by both (`__divti3`, …); a `-static-libgcc` library with its own `__divti3` and an executable using `libgcc_s`'s | built (`n11`): runs (`1 2`); no records; W1-W3 zero over `libgcc.a`'s 398 members; `libgcc_s.so` has no table, so O-24 reads nothing |
| N12 | a vendored copy renamed with `objcopy --redefine-sym` or a prefix macro | the records still name the old symbol | anchoring (O-4): a row is compared only if its object defines the symbol it names |
| N13 | `static` functions or unnamed-namespace entities with one name in many TUs | one name | internal linkage is never a key (O-2) |
| N14 | the same symbol in an unextracted archive member (K17) | two definitions in the inputs | not part of the program; checked only on request (O-23) |
| N15 | `extern int a[];` in one TU, `int a[4];` defining it | types differ | [basic.link] allows an array of unknown bound (`[std, unverified clause]`): kind 5 types are bound-erased and size 0 means unknown (O-35) |
| N16 | definitions that differ in comments, whitespace, macro names that expand alike, `[[likely]]`, `[[deprecated]]`, `[[nodiscard]]`, `[[maybe_unused]]` | different source text | O-10 hashes the AST after expansion and lists the attributes it ignores |
| N17 | an inline function in a header that reads a namespace-scope `const int` (internal linkage) without odr-using it | refers to an internal entity | [basic.def.odr] allows it (`[std, unverified clause]`); `USES_INTERNAL` is set only on an odr-use (O-16) |
| N18 | `assert` in a header's inline function, the header reached as `h.h` and `./h.h` | `__FILE__` differs (clang's message said `./h.h`, g++'s `h.h`: `catalogue` K2) | O-10: builtin-macro expansions hash as placeholders (K-3) |
| N19 | `-fPIC` and not, `-ffunction-sections` and not | different code and sections | O-10 |
| N20 | `clang++ -std=c++17`, static against Debian's `libstdc++.a`, binding `steady_clock::is_steady` to a `const&` | `chrono.o` (built as C++11) defines `is_steady` strongly; the C++17 TU emits it as a `WEAK` COMDAT copy: W3 fires (`legal-fp` F1) | W3 is `--odr=strict` only (O-47). With g++ the copy is `STB_GNU_UNIQUE` and every linker refuses the link as a duplicate, as mortar does (`spec/resolve.md` §3.4, §3.8) |
| N21 | the C99 `inline` + one `extern inline` idiom in a header shared by C and C++ | C strong `sq`, C++ COMDAT `sq`: W3 fires (F2) | W3 strict only; C units carry no kind 2 rows (O-14) |
| N22 | `struct A { static constexpr int n = 3; }; constexpr int A::n;` in a C++14 TU, odr-used in a C++17 TU (protobuf 3.21's pattern) | strong definition + COMDAT copy: W3 fires (F3) | W3 strict only; the two kind 2 rows (one strong, one `VAGUE`) are compared by hash only and are equal (O-15; model `F3-constexpr-member`) |
| N23 | a weak C default `hook` (`STT_FUNC`) overridden by an assembler-written strong `hook:` without `.type` (`STT_NOTYPE`) | function in one file, "something else" in another (F4; the first draft's prototype flagged it) | W2 compares `STT_FUNC`/`STT_GNU_IFUNC` against `STT_OBJECT`/`STT_TLS`/common only; `STT_NOTYPE` never participates (§8.1) |
| N24 | a polymorphic `Base` with a key function in a `-fno-rtti` library, a client compiled with RTTI calling `make()->f()` (the LLVM-library + tool shape) | layouts "differ" in the RTTI slot; `abi.rtti` differs (F5: runs, `7 4Mine`, both compilers, three linkers) | the layout hash has no RTTI bit (O-18); `abi.rtti` is a note (MT-26); the vtable is emitted once (key function), so O-25 has one copy (model `F5-nortti-lib-rtti-client`) |
| N25 | a user `struct X { std::string s; }` and a lambda in an inline function, in TUs of different `-std` | the implicit destructor is `constexpr` in C++20 only; a lambda's `operator()` is implicitly `constexpr` from C++17 | O-10 hashes only explicitly written specifiers (models `mixed-std-implicit-dtor`, `mixed-std-lambda`) |
| N26 | library headers declaring a function `constexpr` from C++20 (`_LIBCPP_CONSTEXPR_SINCE_CXX20`), used from TUs of different `-std` | the written declarator differs: at least 210 of the 541 vague definitions of the heavy TU at `-std=c++17` and `c++20` (`std-mix`) | the rows are `SYSTEM` and the units' `std` differ: an `--odr-stats` count, not a finding (O-32; model `mixed-std-system-row`) |
| N27 | two unrelated C libraries, each with its own `struct node` | the same tag, different members | C units write no kind 3 rows (O-33; model `C-struct-node-two-libs`) |
| N28 | `enum mode get(void)` defined in one C TU, declared `unsigned get(void)` in another | the Itanium type encodings differ | GCC documents such an enum as compatible with `unsigned int`; C-linkage kind 5 rows are compared under C's rules and only under `--odr=strict` (O-33; model `C-enum-compatible-type`: a strict-only finding today, until O-33's compatibility table covers it) |
| N29 | two `dlopen(RTLD_LOCAL)` plugins, each with its own version of a header-only library's inline function at default visibility | one key, two hashes in one process (the first draft's hook aborted, exit 134) | each binds its own copy; the hook compares a key only where a module binds another's definition (O-54; `hook2` A2, A3; three targets) |
| N30 | the same plugin loaded in the base namespace and in a `dlmopen(LM_ID_NEWLM)` namespace | one key, two hashes | namespaces never share an index (O-54; `hook2` E) |

**O-2 (keys never compared).** Local symbols, internal-linkage entities,
and names of a file's `STB_LOCAL` symbols are never keys.

**O-3 (the oracle).** Before a check is on by default, and on every
release after: the false-positive oracle of §12.3 runs the check and
must report **zero** findings, or each finding is a real violation
recorded in the corpus with its reason. "Zero" is measured, not argued;
the measurement is recorded where O-46 says.

## 4. The entity key

**O-4 (key, anchoring).** An entity's key is the pair (name, class):

- the **name** is the mangled name as it appears in the symbol table,
  including ABI tags (`B5cxx11`), module attachment (`W`, Itanium modules
  mangling as kiln writes it) and a symbol version if the name has one
  (`foo@V1` and `foo@@V2` are two keys, N6); for a type, the Itanium
  `<type>` encoding of the class or enum (what follows `_ZTS`);
- the **class** is function, variable or type: two rows with one name
  and different classes are W2's finding (§8), not a definition
  comparison.

Rows are grouped by the row's `key` (FNV-1a 64 of the name, MT-13) and
grouped again by name bytes, so that a key collision cannot pair
different names. An **anchored** row (kind 2, 5) has no name of its own:
its name is that of a symbol its file defines (or, for kind 5, mentions)
whose FNV-1a 64 is the key; a row whose file has no such symbol is
**unanchored** and is not compared unless `--odr=strict` (it describes a
renamed symbol, N12, or a definition the compiler inlined away, X1). Two
symbols of one file with the same FNV-1a 64 and different names make
the rows ambiguous: they are not compared, and `--odr-stats` counts
them [estimate: never seen; FNV-1a 64 over a file's few thousand names].

Cost: the anchor check needs FNV-1a 64 of every global name of a file
with a unit, once per file, only when a record-based check is on
[estimate: about 1 ns a byte, 78 KB of names in the heavy TU, so ~80 µs
per such file, parallel per file]. With `--odr=off`, and while no
record-based check is on, nothing is hashed (O-42).

O-5 is reserved (it held a pointer in an earlier numbering).

## 5. The definition hash

**O-10 (what it covers) [kiln].** The hash of a definition is computed
by kiln over the definition after semantic analysis and template
instantiation, serialized canonically:

- node kinds, operators, literal values as written (after macro
  expansion);
- the declarator **as written**: return and parameter types, cv- and
  ref-qualifiers, storage, and the specifiers and attributes the source
  spells (after macro expansion) that change meaning: `noexcept(…)`,
  `constexpr`, `consteval`, `constinit`, `inline`, `alignas`, `packed`,
  `no_unique_address`, calling-convention attributes, `visibility`.
  **Only explicitly written specifiers are hashed**: a specifier a
  member or a lambda gets implicitly from the language (an implicit
  destructor that is `constexpr` in C++20, a lambda's `operator()` that
  is implicitly `constexpr` from C++17, an implicit `noexcept` on a
  destructor) is not part of the hash (N25). A specifier written through
  a macro is written (N26 is therefore a real difference, handled by
  O-32);
- the **accumulated default arguments** of the function at the end of
  the TU, and the default arguments used by calls inside the definition
  (K8: [dcl.fct.default] requires them to agree for inline functions,
  `[std, unverified clause]`);
- each referenced entity by its **name**: an external entity by its
  mangled name, a local entity (parameter, variable, label, structured
  binding) by its declaration index in the definition, never by its
  spelling; an internal-linkage entity by its name and the flag of O-16;
- each **enumerator** reference as the triple (the enumeration's
  `<type>` encoding, the enumerator's name, its value): K7's `isB` hashes
  differently when `B`'s value differs although its name does not;
- each type by its mangled encoding (the type's own definition is kind
  3's business, not hashed into every user);
- a lambda by its closure type's mangled name and its body; a local class
  likewise;
- implicitly defined members as kiln synthesizes them (their bodies:
  member-wise operations; N10), under the "explicitly written" rule above
  for their specifiers.

It is **insensitive** to: whitespace, comments, macro names, `-O`, `-g`,
`-fPIC`, `-ffunction-sections`, the order of unrelated declarations,
source paths and line numbers, the expansions of `__FILE__`, `__LINE__`,
`__DATE__`, `__TIME__`, `__COUNTER__`, `__builtin_FILE`/`_LINE`/
`_FUNCTION`/`_COLUMN` and `std::source_location::current` (each hashes as
its own placeholder: a preprocessor and parser change in kiln, K-3), how
the definition was reached (textual `#include`, a precompiled header, a
header unit), and the attributes `likely`, `unlikely`, `deprecated`,
`nodiscard`, `maybe_unused`, `fallthrough`, `no_sanitize*`, `gnu::used`,
`gnu::cold`, `gnu::hot` (they change neither meaning nor layout).

What it does **not** see: the code generator's choices (the reason K16d
is kind 1's), and options that change meaning without changing tokens
(the reason kind 1 exists).

**O-11 (schemes).** Kind 1's `odr.scheme` names the serialization and
the hash function. Rows of kinds 2 and 3 are compared only between units
whose schemes are equal; a link with several schemes compares within
each and `--odr-stats` reports the pairs it could not compare.

**O-12 (function, width, collisions).** The definition hash is 128 bits.
The function is kiln's choice, named by the scheme; the recommendation
is FNV-1a 128, which kiln already has for reflection mangling, or
SipHash-1-3 with a fixed key (better mixing, a few lines). Mortar never
computes it, only compares bytes. A collision can only hide a
violation, never invent one (equal hashes are treated as equal
definitions): with `n` distinct definitions of one entity the chance is
about n²/2^129; no adversary is assumed (an author who wants two
definitions has `objcopy`). Truncation to 64 bits in the loadable table
(§10) has the same property at 2^-64.

**O-13 (hash tests) [kiln].** kiln keeps two corpora; each is also an L11
link test (K-8).

- **must-equal**: N1, N2, N10, N16, N18, N19; the same definition reached
  through different include paths, macro spellings, `-I` orders and
  `-ffile-prefix-map`; **through a textual `#include`, a precompiled
  header and a header unit** (three objects, one hash); N25's implicit
  destructor and lambda between `-std=c++14`, `c++17`, `c++20` and
  `c++23`; a strong out-of-line definition and an inline one of the same
  static member (N22).
- **must-differ**: K1, K2, K8, K9, K11, K12, K13; **K7 in the `isB(int)`
  shape** (the enum appears only as a constant); one enumerator's value
  changed with its name kept; a changed `alignas`/`packed`; a changed
  default member initializer; an explicitly written `constexpr` or
  `noexcept` added.

A must-equal failure is a false positive in every user's link: it blocks
release of the hasher.

**O-14 (which definitions have rows).** Kind 2 rows exist for
definitions that may legally appear in more than one TU, and for those
that clash with them: inline functions and variables, template
instantiations (implicit or explicit), static data members of class
templates, explicit specializations, the out-of-line members of class
templates, and the out-of-line definition of a `static constexpr` data
member (N22's strong row). Not for ordinary strong functions and
variables (two of them are already `duplicate-symbol`, `spec/resolve.md`
§3.8; their declarations are kind 5's), not for `gnu_inline` (N7), not
for weak definitions that are not vague linkage (N5), and **not from C
translation units** (C has no vague linkage; N21).

**O-15 (row and flags).** A kind 2 row is

| offset | field | |
|---|---|---|
| 0 | `u64 key` | MT-13 |
| 8 | `u8[16] hash` | O-10 |
| 24 | `u32 name` | 0: anchored (O-4); otherwise a string |
| 28 | `u16 class` | 1 function, 2 variable |
| 30 | `u16 flags` | below |
| 32 | `u32 module` | the named module the entity is attached to, as a string; 0 none |
| 36 | `u32 file` | the definition's file (MT-38) |
| 40 | `u32 line` | |
| 44 | `u32 reserved` | 0 |

Flags: bit 0 `VAGUE` (vague linkage), 1 `IMPLICIT_INST`, 2
`EXPLICIT_INST`, 3 `EXPLICIT_SPEC`, 4 `USES_INTERNAL` (O-16), 5
`PARTIAL` (the hasher skipped a construct it does not cover: the row is
compared, and a difference is a warning, never an error), 6
`INLINED_ONLY` (no symbol emitted; written only with
`-fmason-metadata=all`), 7 `GMF` (declared in a global module fragment),
8 `SYSTEM` (defined in a system header). An `EXPLICIT_SPEC` row and an
`IMPLICIT_INST` row for one key are a violation whatever the hashes
(K13: the implicit instantiation was used where the specialization was
not visible, [temp.expl.spec] `[std, unverified clause]`);
`EXPLICIT_INST` and `IMPLICIT_INST` are compatible (N9). A strong row
without `VAGUE` and a `VAGUE` row of one key (N22) are compared by hash
only: no flag combination other than `EXPLICIT_SPEC` + `IMPLICIT_INST` is
a finding by itself.

**O-16 (`USES_INTERNAL`) [kiln].** Set when the definition odr-uses an
entity with internal linkage that is not one of [basic.def.odr]'s
exceptions (a `const` object of literal type whose value is used and
whose address is not; `[std, unverified clause]`). Two rows of one key
with `USES_INTERNAL` from different units refer to different entities
even with equal hashes: `odr-internal-reference`, **`--odr=strict`
only** (X5). Measured with a relocation proxy (`k12proxy`: a COMDAT
group whose relocations reach a local function, object or non-group
section): protoc against libstdc++, 51,596 groups, 95 reach an internal
entity, 3 such entities in two or more files (`std::mutex::lock`,
`unlock`, a `std::call_once` instantiation: libstdc++'s `static inline`
`__gthread_*` wrappers, `gthr-posix.h`); the heavy TU at `-std=c++17` and
`c++20` with `libc++.a` (libc++ 22), 2,872 groups, 219 reach an internal
entity, 2 in two or more files (a `std::variant` visitation table and
`__clang_call_terminate`). The standard library itself sets the flag, so
the check stays strict unless kiln's analysis and a corpus show zero on
user rows with `SYSTEM` rows exempt (Q5).

## 6. Layout records

**O-17 (kind 3 row; reach).** A second, independent line of evidence: one
row per class, struct, union or enumeration whose definition the TU
contains (a C++ TU: O-33 excludes C) and that an emitted entity's
definition or declarator reaches. **Reach** (widened after the review's
B2): the types of members, bases, parameters, returns, locals,
`new`/`sizeof`/`alignof` operands, **the type of every expression**
(K7's enumerator), **every template argument** of an instantiation, and
the **pointee and referee** of every pointer and reference type reached,
when that type is complete in the TU; transitively through members,
bases and template arguments. A type that is incomplete in a TU gets no
row from it (X4). K4 through an `S*`-only use is covered when `S` is
complete in both TUs (model `K4-pointer-only`: kind 2 hashes equal, kind
3 layouts differ).

Size with the wider reach: clang 22's DWARF of the heavy TU, a proxy for
"types the TU needs complete" and "types the TU uses, pointees included",
has 301 complete definitions at `-g` and 334 at `-g -fstandalone-debug`
(`k3count`); the first draft's 334 rows therefore stand as the upper
proxy [estimate until kiln writes kind 3; MT-41's numbers use it].

| offset | field | |
|---|---|---|
| 0 | `u64 key` | FNV-1a 64 of the `<type>` encoding |
| 8 | `u8[16] hash` | O-10 over the type's definition (members, bases, member declarations, in-class initializers) |
| 24 | `u64 layout` | O-18 |
| 32 | `u32 name` | the `<type>` encoding (types have no symbol to anchor to) |
| 36 | `u32 size` | bytes |
| 40 | `u16 align` | bytes |
| 42 | `u16 flags` | 0 `ENUM`, 1 `UNION`, 2 `POLYMORPHIC`, 3 `HAS_VBASES`, 4 `PARTIAL`, 5 `SYSTEM` |
| 44 | `u32 file` | |
| 48 | `u32 line` | |
| 52 | `u32 details` | offset of the type's entry in the kind 4 block, `0xffffffff` none |

**O-18 (layout hash) [kiln].** 64 bits over: size, alignment; per base
(in declaration order) its key, offset and virtuality; per non-static
data member its offset in bits, bit-field width, and type encoding; the
vptr's offset; the vtable shape: slot count and, per slot, the mangled
name of the final overrider's declaration (not of its body); for an
enumeration, the underlying type and the (name, value) list. **Not** the
RTTI slot: whether a vtable's `type_info` slot is filled depends on
`-frtti`, which is legal to mix (N24); K16c is O-25's. From kiln's
`RecordLayout` (`kiln_ast/src/types/layout.rs`) and `VTableLayout`
(`kiln_ast/src/class.rs`).

**O-19 (kind 4, layout details).** Optional (`-fmason-metadata=detail`):
per type, the list O-18 hashes, in readable form (member names, offsets,
type strings), so that a diagnostic can say *what* differs. Mortar reads
kind 4 only to render a diagnostic that kind 3 already decided. Its exact
encoding is fixed by WP-073 with kiln (a list of `{u32 name; u32 type;
u64 bit_offset; u32 width; u32 flags}` entries is the proposal).

## 7. Comparison points

**O-20 (what is compared; normative).** The rows of every unit of every
live file (`meta.md` MT-21), whatever the fate of the sections they
describe: grouped by key and name (O-4), each group is partitioned by
hash (and, for types, by layout); more than one class is a finding. This
rule defines the result; any implementation must report exactly its
findings.

**O-21 (an optimisation of O-20).** For vague-linkage entities the
copies are `comdat_copies` (`spec/resolve.md` §6.7: `(ComdatId, FileId)`
pairs, the keeper first); the diagnostic names the keeper's file as
"kept" so that the user knows which body runs. An implementation may
drive the kind 2 comparison from the groups with two or more live copies
(a binary search of each copy's sorted kind 2 block, MT-11) **only if it
also** probes, against every other live unit, each row that is not in
such a group: strong `EXPLICIT_SPEC` rows (K13 is one strong definition
and one COMDAT copy: a group of one), strong rows without `VAGUE` (N22's
shape), and rows whose group has one live copy. T-O-21 checks that the
optimisation and O-20 report the same findings on the whole corpus.
[estimate: a link of 1,000 heavy TUs has ~600,000 copies; ~10 probes
each, ~60 ms of one core, ~6 ms on 12; 29 MB of rows touched.]

**O-22 (types and declarations).** Kind 3 and kind 5 rows have no group
to drive them: every row of every live unit is read and grouped by key
(a parallel hash-partition by key, merged in `FileId` order). [estimate:
334 type rows and 661 declaration rows a TU; 1,000 TUs: 1 million rows,
45 MB, ~50 ns a row → ~50 ms of one core.] This is the check's largest
cost and what `--odr=off` saves; it is measured, not estimated, by
WP-071's acceptance test and WP-073 (§13) against O-45.

**O-23 (archives not extracted) — opt-in.** With `--odr-check-archives`,
the units of archive members that were **not** extracted are read too,
and a member whose kind 2 row disagrees with a live definition of the
same key is reported as the warning `odr-archive-shadow` (K17). Off by
default: such a member is not part of the program (N14), and reading it
costs a parse of every member's section headers [estimate: linear in
archive size; for a 100 MB archive, ~20 ms].

**O-24 (the static check against shared-object inputs: "dynamic link
time" without a runtime).** `[M6]` for executables, `[M4]` for
shared-object outputs. When the output is dynamic and a shared object
that mortar reads as an input (named on the command line or found by
`-l`; not the indirect `DT_NEEDED` closure, which mortar does not read)
carries a `.note.mason` table (O-50..O-52), mortar compares, before it
writes anything:

- **(a) output against library.** For each row of the library's table
  (key, 64-bit hash, name from its `.dynsym`, O-51) whose name the output
  defines from a live unit with a kind 2 row: if the output **exports**
  the name (it is in the output's `.dynsym` by `spec/dyn.md`'s export
  rule, which exports a definition that a shared input defines or
  references), the library binds the output's definition at load time,
  so the keeper's row hash (its first 8 bytes) is compared with the
  table's: a difference is `odr-violation` naming the library as the
  other place. If the output does not export it (hidden, protected,
  `-Bsymbolic`, a version script's `local:`), the library binds its own
  copy and nothing is compared.
- **(b) library against library.** Two shared-object inputs whose tables
  carry one name (both rows are preemptible, O-50) and that the output
  does not define: at load time the one earlier in the output's
  `DT_NEEDED` order provides the definition to both; their hashes are
  compared and a difference is `odr-violation` naming both, the first as
  the one that runs (K18 at static link time).
- **(c) sizes.** A variable the output references and the library
  defines, with a kind 5 row of the output carrying a size: compared with
  the library's `st_size` (`odr-size-mismatch`, K18b's copy relocation
  would truncate).

`SYSTEM` rows follow O-32. Cost: merges of sorted sequences (the tables
and the output's kind 2 rows are sorted by key) [estimate: the hook's
measured 2.3 ns a row for a merge, `dynamic`]. Status: like kind 2
(`--odr=strict` until its corpus, O-47). What only load time can see: a
different build of the library at run time (K18b's v2), libraries loaded
by `dlopen`, libraries the link never named, indirect dependencies (the
hook, §10.3).

**O-25 (vtables emitted with and without RTTI; K16c).** For each `_ZTV`
symbol defined by COMDAT copies in two or more live files whose units'
kind 1 `abi.rtti` values differ: `odr-vtable-rtti-mismatch` naming the
type and both units (the kept copy's slot is what every user reads).
Model `K16c-two-vtable-copies`: found for `_ZTV1B` and `_ZTV1D`; F5
(N24): nothing (one copy, key function). The catalogue's K16c, where only
one TU emits the vtable, is X3; Q16 asks about the extension that would
catch it (a vtable from an `abi.rtti=off` unit while an `abi.rtti=on`
unit defines the same type's `_ZTI`; the model shows it catches the
catalogue's shape and stays silent on F5). Status: `--odr=strict` until
its corpus (it needs kind 1).

## 8. Checks without records

### 8.1 The foreign checks (W)

These read only what resolution already holds: for each live COMDAT
group with two or more copies, the defined symbols of each copy, and for
W3 the strong definitions of the same keys. Measured from the **rule
text** (`wcheck.py`) over the corpora of experiment 1 (§12.3, Appendix
A): `libstdc++.a` + `libsupc++.a` (254 files, 6,990 names), `libc.a`
(2,022 files), `libgcc.a` + `libgcc_eh.a` (409), libc++ 22 `libc++.a` +
`libc++abi.a` (89), `libgfortran.a` (804), protoc (168 objects and
archives, 32,522 names), protoc rebuilt mixed (823 files, 33,459 names,
links and runs under all three linkers), plus the review's Fortran
COMMON, C tentative-definition and 12-TU mixed links:

| rule | finding | catches (measured) | false positives measured | status (O-47) |
|---|---|---|---|---|
| W1 | an `STT_OBJECT`/`STT_TLS` symbol defined in two copies of one COMDAT group with different `st_size` | K5 (`_ZTV1B` 48/40), K6 (`_ZTI1D` 56/24, `_ZTV1D` 72/40) | 0 on every corpus above; 0 on N20-N24 | **on**, warning `odr-size-mismatch` |
| W2 | one key defined as `STT_FUNC`/`STT_GNU_IFUNC` in one live file and as `STT_OBJECT`/`STT_TLS`/common in another; `STT_NOTYPE` never participates | (none in the catalogue) | 0 on every corpus; N23 (F4) is not a finding under this wording (the first draft's prototype flagged it) | **on**, warning `odr-symbol-type-mismatch` |
| W3 | a strong non-COMDAT definition and a COMDAT copy of the same key | K13 | **3**: N20, N21, N22 (`legal-fp` F1-F3; protobuf ships N22's pattern) | `--odr=strict` only, warning `odr-strong-and-comdat`; never proposed as a default |
| W4 | data contents (bytes and symbol-normalized relocations) differ between copies | K5, K6, K9 | **1**: a libstdc++ vtable (D1/D2 aliases differ between GCC and Clang) | `--odr=strict` only, warning |
| W5 | section alignment differs | — | **4**: `_ZTS*` 8 (GCC) vs 1 (Clang) | not a check |
| F | function code bytes differ | K1, K2, K11 … | everything at different `-O` (N1) | never |

Cost: one pass over the copies resolution keeps, comparing two integers
per symbol [estimate: under 1 ms per 100,000 copies; the Python
prototype read protoc's 168 inputs in about 2 s, dominated by Python's
parsing].

### 8.2 Declarations (kind 5)

**O-35 (kind 5 row) [kiln].** One row per external-linkage function or
variable the TU defines or references (an undefined symbol), excluding
unprototyped C declarations:

| offset | field | |
|---|---|---|
| 0 | `u64 key` | MT-13 |
| 8 | `u64 type` | 64-bit hash of the declared type's `<type>` encoding with array bounds erased |
| 16 | `u64 size` | `sizeof` of a variable's declared type; 0 for a function or an array of unknown bound |
| 24 | `u32 name` | 0: anchored |
| 28 | `u32 type_name` | the `<type>` encoding as a string (deduplicated), for the message |
| 32 | `u16 flags` | 0 `DEFINITION`, 1 `VARIABLE`, 2 `EXTERN_C`, 3 `NOEXCEPT` |
| 34 | `u16 reserved`, `u32 reserved` | 0 |

Rows of one key from different units disagree when the `type` hashes
differ, or both sizes are non-zero and differ, between a definition and
a declaration or between two declarations. For **C++-linkage** entities
that is `odr-declaration-mismatch` (K14; [basic.link] "the types ...
shall be identical", `[std, unverified clause]`), except that a
difference only in `NOEXCEPT` is reported as a note-level warning
[unverified: whether the two compilers mangle and treat it alike in every
mode]. **C-linkage** entities (`EXTERN_C`, and every row of a `lang=c`
unit) follow O-33. Status: `--odr=strict` until its corpus (O-47).

**O-33 (C translation units).** A unit whose kind 1 `lang` is `c`
(`meta.md` MT-26):

- writes **no kind 3 rows** (C types have no linkage; two C libraries with
  different `struct node` are legal C, C11 6.2.7 constrains only types
  used for the same object or function `[std, unverified clause]`: N27),
  and no kind 2 rows (O-14);
- writes kind 5 rows, whose C-linkage comparisons (also against C++
  units' `extern "C"` rows) use **C's compatible-type rules**: an
  enumeration is equal to its compatible integer type (GCC: `unsigned
  int` for an enum without negative values, else `int`; kiln records the
  compatible type it chose, K-5), top-level qualifiers of parameters are
  ignored, an unprototyped declaration is never compared. Mortar compares
  the `type` hash of the canonical C type kiln writes for C-linkage rows
  (K-5), so the rules live in kiln's canonicalization and mortar compares
  bytes;
- C-linkage findings are `odr-declaration-mismatch` under
  `--odr=strict` **only, until a C corpus** (glibc's headers against
  `libc.a`'s objects, and two or three C libraries' headers against
  their objects, e.g. zlib, sqlite, libpng [the selection is WP-074's])
  shows zero. Model `C-enum-compatible-type` shows today's Itanium-type
  comparison flagging N28: the reason for the canonical C type.

K14b, two C++ TUs that disagree about an `extern "C"` function, is also
under this rule; C++-linkage kind 5 (K14) is as designed in O-35.

### 8.3 Mixed links

**O-34.** When one side of a comparison has records and the other has
not: a kind 5 row with a size is compared with the foreign definition's
`st_size` (`odr-size-mismatch`, `--odr=strict` until its corpus: the
foreign definition's declared type is unknown); kind 3 is compared with
nothing (foreign objects carry no layout without DWARF, Q13); kind 2 with
nothing. W1 and W2 run regardless of records.

## 9. Diagnostics and switches

### 9.1 Kinds

Proposed for `spec/diag.md` §3.1 (amendment A-4), a new area
"one-definition rule" between "symbols" and "relocations". Messages follow
the wording policy (no reference linker has these conditions; GCC's
`-Wodr` text is the model where it fits). The severity is the default
mode's (`--odr=warn`, O-40) for a check that is on; O-47 says which are.

| id | variant | message | severity when reported |
|---|---|---|---|
| `odr-abi-mismatch` | `OdrAbiMismatch { option, first, second }` | `translation units compiled with different {option}: {first} and {second}` | warning |
| `odr-archive-shadow` | `OdrArchiveShadow { name }` | `{name} in an archive member that is not linked is defined differently` | warning (opt-in) |
| `odr-declaration-mismatch` | `OdrDeclarationMismatch { name }` | `{name} is declared with a type that does not match its definition` | warning; error under `--odr=error`/`strict` |
| `odr-internal-reference` | `OdrInternalReference { name }` | `{name} refers to an entity with internal linkage, which differs in each translation unit` | warning (strict only) |
| `odr-isa-mismatch` | `OdrIsaMismatch { name }` | `the kept copy of {name} was compiled for other target features than a translation unit that uses it` | warning |
| `odr-size-mismatch` | `OdrSizeMismatch { name }` | `{name} has different sizes in different files` | warning |
| `odr-strong-and-comdat` | `OdrStrongAndComdat { name }` | `{name} has a non-inline definition and an inline copy` | warning (strict only) |
| `odr-symbol-type-mismatch` | `OdrSymbolTypeMismatch { name }` | `{name} is a function in one file and data in another` | warning |
| `odr-type-mismatch` | `OdrTypeMismatch { name }` | `type {name} violates the one-definition rule` | warning; layout differences are errors under `--odr=error`/`strict` |
| `odr-violation` | `OdrViolation { name }` | `{name} violates the one-definition rule` | warning; error under `--odr=error`/`strict` |
| `odr-vtable-rtti-mismatch` | `OdrVtableRttiMismatch { name }` | `the vtable of {name} is emitted by translation units compiled with and without RTTI` | warning |
| `meta-malformed` | `MetaMalformed { reason }` | `malformed Mason metadata: {reason}` | warning; error under `strict` |
| `meta-version` | `MetaVersion { major }` | `Mason metadata version {major} is not supported; its records are ignored` | warning; error under `strict` |

Every `{name}` goes through the demangling hook (`spec/diag.md` §5.4).
Context lines (`spec/diag.md` §2.4), e.g. for K1 linked `a.o b.o`:

```
warning: f(int) violates the one-definition rule
>>> defined in a.o (kept), at h.h:3
>>> defined differently in b.o, at h.h:3
>>> the definitions differ; a.o and b.o were compiled with different -D options? (compare their command lines)
```

- One diagnostic per entity: the copies are partitioned by hash; the
  first class (the keeper's) is "defined in", each other class's first
  file "defined differently in", with at most 3 files a class and a count
  of the rest.
- Notes that explain, from kind 1: every key that differs between the
  two units (`>>> a.o: abi.short-enums=on; b.o: off`, `>>> a.o: std=c++17;
  b.o: c++20`). The question in the K1 example is printed only when no
  kind 1 key differs.
- Types: with kind 4, the first difference (`>>> member 'y' at offset 8
  in a.o only`); without, the sizes and alignments.
- Key (`spec/model.md` §8): phase 40, unit = the `FileId` of the first
  file of the second class, seq = its symbol-table index; for a row
  without a symbol (types, unanchored rows) `2^40 + ` the row's index in
  its unit (amendment A-2).

The load-time hook's messages (§10.3) are the runtime's, not mortar
diagnostics: `odr-violation-at-load`, `odr-hook-out-of-memory`,
`odr-hook-stopped`.

### 9.2 Severity and switches

**O-40 (the default mode).** The default is **`--odr=warn`**: every
finding of a check that is on by default (O-47) is a warning, and no
finding fails a link, until O-46 records a check's move to error. That is
the one statement of the default; O-41, O-46, O-47 and Q1 agree with it.
`PARTIAL` rows, unanchored rows and different schemes never produce
errors in any mode below `strict`. `--fatal-warnings` and `-w` act as on
every warning (`spec/diag.md` §6.3).

**O-41 (switches)** (amendment A-5 to `spec/cli.md`):

- `--odr=warn` (default): O-40.
- `--odr=error`: the default-on record-based findings `odr-violation`,
  `odr-type-mismatch` (layout) and `odr-declaration-mismatch` (C++
  linkage) are errors; everything else as in `warn`.
- `--odr=strict`: **every** check runs, those of O-47's strict column
  included, plus unanchored, `PARTIAL` and `INLINED_ONLY` rows; the
  record-based findings above are errors, every other finding a warning,
  `meta-malformed` and `meta-version` errors.
- `--odr=off`, also spelled **`--no-odr-check`**: no check runs, no
  container byte is read (O-42), the W checks do not run, and no
  loadable table is written (§10). The output is byte-identical to the
  default link's when that link succeeds and the table is off (T-O-41).
- `--odr-check-archives` (O-23), `--odr-table=off|exported|full` (O-50),
  `--odr-stats` (O-44).
- GNU gold's `--detect-odr-violations`: accepted as a **no-op**: mortar
  already checks by default; it prints once `note: mortar checks the
  one-definition rule by default; --detect-odr-violations has no further
  effect` (it must not lower an error to a warning, review M11).

**O-42 (a link that does not use the feature does not pay for it).** A
hard rule. A container byte is read only when a check that needs it is
on: with `--odr=off`, and in the default mode while O-47 lists no
record-based check as on, mortar reads no byte of any `.mason.meta`
(`meta.md` MT-42, amendment A-8 to `spec/input.md` §11.1), hashes no
name (O-4) and writes no table. The W checks read only what resolution
holds already. T-O-42 counts bytes read.

**O-44 (`--odr-stats`).** One line to standard error: units read, rows by
kind, schemes seen and the unit pairs not compared because of them,
ambiguous anchors (O-4), unanchored rows, `SYSTEM` differences between
units of different `-std` (O-32), time spent and container bytes read.
For tuning and for the oracle of §12.3.

**O-45 (cost budget).** With the checks O-47 lists as on, the
comparisons of §7-§8 cost at most 3 % of the link's wall time on the
bench lane's Mason workloads (`spec/harness.md` "bench", amendment A-6);
above it, the check is a performance bug. Every cost in §4-§8 is an
`[estimate]` until WP-071's acceptance test and WP-073 measure it. The
operator's frame (2026-11-15): diagnostics may cost what mold does not
pay, and `--no-odr-check` must make mortar competitive with mold (O-42).

**O-46 (turning a check on).** A check moves from `--odr=strict` to on
by default only after O-3's oracle has run it at zero on its corpus
(§12.2, §12.3; for C-linkage kind 5 the C corpus of O-33). A check moves
from warning to error under the default mode only by the operator's
decision (Q1), after it has been on at zero for a release. Each move is
recorded in the landed record of the package that makes it and in
O-47's table, by amendment.

**O-47 (the register).** Which checks are on by default, today:

| check | needs | status | evidence | moves when |
|---|---|---|---|---|
| W1 `odr-size-mismatch` | foreign facts | **on** (warning) | experiment 1: zero on every corpus of §8.1 | — |
| W2 `odr-symbol-type-mismatch` | foreign facts | **on** (warning) | experiment 1: zero; N23 excluded by its wording | — |
| `meta-malformed`, `meta-version` | container headers | **on** (warning) | no object of the corpora has a container; they fire only on broken producers | — |
| W3 `odr-strong-and-comdat` | foreign facts | strict | N20-N22 | never by default (Q12 withdrawn) |
| W4 | foreign facts | strict | N3's vtable | never by default |
| kind 2 `odr-violation` (hash, K13 flags) | kiln K-1, K-3 | strict | cannot run yet | WP-074's oracle at zero |
| kind 3 `odr-type-mismatch` (layout) | K-4 | strict | cannot run yet | WP-074's oracle at zero |
| kind 3 definition-only differences | K-4 | strict | — | after layout's, separately |
| kind 5 C++ linkage `odr-declaration-mismatch` | K-5 | strict | cannot run yet | WP-074's oracle at zero |
| kind 5 C linkage | K-5 | strict | N28 | the C corpus of O-33 at zero |
| `odr-internal-reference` | K-3 | strict | `k12proxy`: libstdc++ sets it | Q5 |
| `odr-abi-mismatch` (`ENFORCE` keys), `odr-isa-mismatch` | K-2 | strict | cannot run yet | WP-074's oracle at zero |
| `odr-vtable-rtti-mismatch` | K-2 | strict | model only | WP-074's oracle at zero |
| O-24 (shared-object inputs) | K-3 and mortar-made libraries | strict | cannot run yet | with kind 2 |
| O-34 kind 5 vs foreign sizes | K-5 | strict | cannot run yet | WP-074's oracle at zero |
| `SYSTEM` `-std` differences (O-32) | K-3 | `--odr-stats` count only | `std-mix`: 210 in one pair of TUs | Q6 |
| `odr-archive-shadow` | K-3 | opt-in flag | — | never by default |
| load-time hook | the table and runtime | **off** (no table is written by default) | experiment 5 | Q2 |

### 9.3 ABI and ISA summaries

**O-30 (`ENFORCE` keys).** Two live units whose kind 1 rows for the same
name both carry `ENFORCE` with different values: `odr-abi-mismatch`, one
diagnostic per (name, value pair), naming the first unit of each value.
Mixing is legal when no affected type or call crosses the two units, so
the mismatch alone proves nothing; where it does cross, kind 3 sees it
(K16a enum size) and the type's diagnostic names the key in a note. K16e
(i386 struct return) has no kind 3 witness. `abi.rtti` is **not** an
`ENFORCE` key (N24; O-25 uses it). Status: strict until its corpus.
Cost: about 16 rows a unit [estimate: 16,000 rows for 1,000 TUs, under
1 ms].

**O-31 (`isa`).** For a COMDAT group whose copies come from units with
different `isa` strings, when the keeper's `isa` differs from that of a
unit with another copy: `odr-isa-mismatch` (K16d). Mortar does not order
feature sets (strings are opaque); it reports the difference. Q7 asks
whether mortar should instead keep the copy of the least-featured unit.
Status: strict until its corpus.

**O-32 (`-std`).** When a kind 2 or kind 3 difference is between units
whose `std` (kind 1) differs **and every row of the group carries
`SYSTEM`**, it is not a finding: `--odr-stats` counts it (N26: library
headers legitimately change declarators and bodies with the standard;
`std-mix` measured at least 210 such definitions between one pair of
libc++ TUs, and 29 + 53 definitions present under one standard only).
`--odr=strict` reports them as warnings. A difference between `USER`
rows (no `SYSTEM` flag) of units of different `std` stays a finding
(model `mixed-std-user-body`: `#if __cplusplus` in user code changes the
tokens); whether mixed `-std` should also be exempt for user rows is the
operator's Q6.

## 10. The dynamic side

### 10.1 What the table holds

**O-50 (which rows).** With `--odr-table=exported` (not the default:
O-58), a mortar-made executable `[M6]` or shared object `[M4]` carries
one row per definition it **exports preemptibly**: in `.dynsym`,
defined, `STV_DEFAULT`, not bound locally by `-Bsymbolic`,
`-Bsymbolic-functions` (for functions) or a version script's `local:`;
and that has a kind 2 row with `VAGUE` or `EXPLICIT_SPEC`. Protected and
hidden definitions are never bound by another module (K18's second
row), so they have no row. With `=full`, also hidden vague definitions
and kind 3 types (strict use only, Q14). With `=off`, no table and no
runtime.

**O-51 (row).** `{u64 key; u64 hash; u32 dynsym; u16 flags; u16
reserved}` (24 bytes), sorted by key: the key of MT-13; the first 8 bytes
of the kind 2 hash of the **keeper's unit's** row (the copy that runs:
when several units have rows for the key, possibly with different hashes
after a link that only warned, the keeper's is written, never another);
the `.dynsym` index of the symbol (the name comes from
`.dynsym`/`.dynstr`, so the table carries no strings); O-15's flags as a
`u16`, so `SYSTEM` (bit 8) reaches the runtime. Header `{u32 version =
1; u32 count; u32 row_size = 24; u32 reserved}`.

Size: 24 bytes per exported vague definition. Measured counts of
exported weak or unique definitions in system libraries (`nm -D`, by
hand): `libstdc++.so.6` 3,991, `libclang-cpp.so.19.1` 12,929,
`libLLVM.so.19.1` 15,129: 96 KB, 310 KB and 363 KB of table, in pages
that are read at load and never written.

### 10.2 Section and segment

**O-52 (encoding).** An allocated `SHT_NOTE` section `.note.mason`,
`sh_addralign` 8, holding one note: owner `MasonMeta` (`n_namesz` 10),
`n_type` **`0x4d4f4401`** (`NT_MASON_ODR`; a distinctive value, because
binutils `readelf -n` prints type 1 under an unknown owner as
`NT_VERSION`), descriptor = O-51's header and rows. It lies in a
`PT_NOTE` with `p_align` 8, which it may share with other 8-aligned notes
(`spec/layout.md` places one `PT_NOTE` per run of notes of one
alignment); readers walk every note of every `PT_NOTE`. With `n_namesz`
10 the descriptor starts at offset 24 under both the 4-byte and the
8-byte note layouts (glibc's `ELF_NOTE_DESC_OFFSET`: `align(12 +
namesz)`), so readers of either kind agree.

Measured with this exact encoding (`note-final`; aarch64, GCC and GNU
ld): GNU ld gave it a `PT_NOTE` of its own (alignment 8) in both an
executable and a shared object; `readelf -n` prints `Unknown note type:
(0x4d4f4401)` with the descriptor, `eu-readelf -n` `<unknown>:
1297040385`, `file` is unaffected; `strip`, `strip --strip-all`,
`eu-strip`, `llvm-strip` 19 and `objcopy --strip-all` keep it; gdb runs
the program. `eu-elflint` exits non-zero (`unknown object file note type
… with owner name 'MasonMeta'`), as for every unknown note; whether any
distribution's QA runs `eu-elflint` on development builds is not known:
a known cost of `--odr-table`, and one reason it is off by default.

Why a note, measured against the alternatives (`dynamic`; the same table
in a `PT_NOTE`, in a `PT_LOOS`-range segment `0x6d736e00`, and found
through a `DT_` tag `0x6d736e00`; executable and shared object):

| | note `PT_NOTE` | segment `PT_LOOS+…` | `DT_` tag |
|---|---|---|---|
| `ld.so` runs it | yes | yes | yes |
| `strip`, `llvm-strip` 19, `eu-strip` | kept, program runs | kept, runs | kept (tag present), runs |
| gdb 16.3 backtrace | 2 frames, normal | normal | normal |
| lldb 19 backtrace | normal | normal | normal |
| `eu-elflint` | error: unknown note type | error: unknown program header type | (the measured file also had the note; the tag alone is not isolated) |
| readelf | `readelf -n` prints it | `LOOS+0xd736e00` | `Operating System specific: 6d736e00` |
| found by the runtime | `dl_iterate_phdr`, standard type | `dl_iterate_phdr`, private type | needs the dynamic section; absent in static executables |
| patchelf | not verified (not installed) | | |

The note wins on discovery (a standard segment type any tool lists), on
inspection (`readelf -n`), and it is what `dyn.md` §14 anticipated.
`PT_LOOS+0xd736e00` (`PT_MASON`) stays unused, recorded so that nobody
takes it.

### 10.3 The load-time hook

**O-53 (the runtime and how it ships).** A small object, the runtime,
added by mortar as an ordinary input of every output that gets a table.
It is C (about 250 lines; prototype 2, `hook2/mason_rt2.c`, is 230), and
mortar itself stays `forbid(unsafe_code)`: the runtime is bytes it links,
not code it runs. How the bytes are produced and verified is the
operator's Q3 (three options with their costs; the recommendation is the
C source and the generated object bytes for each target **checked in**,
embedded in mortar, with a byte-identical rebuild check in CI). In no
option does building mortar need a cross compiler. Its tests: the matrix
of O-54 (cases A-J) under qemu-user for x86-64 and i386 and natively on
AArch64, per release (WP-077); the verifier checks that the runtime's
section bytes in an output equal the embedded ones (§12.5).

**O-54 (behavior).** Measured on prototype 2 (`hook2`), glibc 2.41,
natively on AArch64 and under qemu-user for x86-64 and i386, identical
outcomes on the three targets:

- **Entry.** One `.init_array.00000` entry and one `.fini_array.00000`
  entry in every module with a table, executable and shared object
  alike (the priority range 0-100 is the implementation's; user code
  starts at 101). Measured with GNU ld, lld and mold, shared object and
  executable: the runtime runs before a user `constructor(101)` and
  before default constructors. No `.preinit_array` entry: nothing needs
  to run before the libraries' constructors (below).
- **One index per scope, learned from the loader's own binding.** Every
  runtime copy defines one exported, default-visibility object,
  `__mason_odr_state` (a pointer), and reads it through its GOT, so each
  module binds the first definition its lookup scope reaches. All modules
  of the global scope (the executable, its `DT_NEEDED` closure,
  `RTLD_GLOBAL` modules) share the index of the first of them; a module
  loaded with `RTLD_LOCAL` shares it if the global scope has one, and
  otherwise has its own; a `dlmopen` namespace never sees another
  namespace's. The first module to run creates the index. No loader
  internal is read.
- **Registration.** At its init entry a module records (load base and
  end, name, table, its `.dynsym`/`.dynstr`, its own `resolve` function)
  in a registry, and inserts its rows into an open-addressing hash index
  of `{key, hash, count, multi}`. A key seen with two different hashes is
  `MULTI` from then on.
- **Comparison only where the loader binds.** For each `MULTI` key the
  new module carries, every registered module `X` that has the key
  (binary search of `X`'s sorted table) is asked where its references
  bind: `X`'s own copy of `resolve(name)` calls `dlsym(RTLD_DEFAULT,
  name)`, which glibc resolves in the **caller's** lookup scope (the
  call is not a tail call, so the caller is `X`); a versioned name uses
  `dlvsym` [not prototyped]. If `X` binds a definition inside another
  registered module `Y` whose row hash differs from `X`'s: a violation
  between `X` and `Y`. A module that binds its own definition is never in
  violation. Two `RTLD_LOCAL` modules are therefore never compared unless
  one binds the other's definition, which glibc does only through a
  dependency of the dlopen'ed module (case D below).
- **`SYSTEM` rows** are skipped unless `MASON_ODR=strict`, so the hook
  never reports what O-32 only counts.
- **Report.** `mason: error: odr-violation-at-load: <name>: <X> binds the
  definition in <Y>, which differs from its own`, written to file
  descriptor 2 with `write` (the shipped runtime; the prototype uses
  stdio), then `abort()`; `MASON_ODR=warn` writes and continues;
  `MASON_ODR=off` skips the check. Read with `secure_getenv`, so a
  set-user-ID program always checks.
- **`dlclose`.** The `.fini_array.00000` entry marks the module's
  registry entry dead and decrements its keys' counts. glibc does not
  unload a module whose definitions another module bound (its
  dependency tracking), so the state object outlives the modules that use
  it [glibc behavior, exercised by case J, not proved].
- **Threads and `fork`.** glibc runs constructors and destructors under
  its load lock (`dl_load_lock`, recursive, so `dlsym` inside is
  allowed); all mutation of the index happens there. Case J: 8 threads ×
  300 `dlopen`/`dlclose` of 4 hooked libraries alternating `RTLD_LOCAL`
  and `RTLD_GLOBAL`, a `fork` every 10th iteration: three runs per
  target, no crash, no hang.
- **Memory and bounds.** The registry and the index live in `mmap`ed
  memory, no `malloc`, grown by doubling; there is no fixed module or
  row limit [estimate: 48 B a row at half load plus 64 B a module]. When
  an `mmap` fails: `mason: warning: odr-hook-out-of-memory: the
  load-time ODR check stopped at <module>; this module and every module
  loaded after it are NOT checked`, once, then `mason: warning:
  odr-hook-stopped: <module> not checked` for each later module: never
  silence (case H, with a test limit at 300 modules: one warning and 291
  "not checked" lines).
- **`DT_STRTAB`/`DT_SYMTAB`.** The runtime reads its own module's
  `_DYNAMIC` (a hidden symbol). glibc relocates these `d_ptr`s in place
  for the executable, libc and ld.so but not for the vDSO
  (`preinit-dynamic`); other loaders (musl) do not relocate them
  [unverified: musl is not installed]. Rule: when `d_ptr` is below the
  module's load base, add the base.
- **Other C libraries.** glibc resolves `RTLD_DEFAULT` in the caller's
  scope; musl resolves it in the global scope only [unverified], which
  would miss case D (never a false report). The runtime calls
  `gnu_get_libc_version` through a weak reference; on any other C library
  it only ever **warns** for dlopen'ed modules and never aborts.
- **Static executables** (and static PIE) get no table and no runtime:
  the static check saw every definition (MT-33). A static executable
  that `dlopen`s is not covered (recorded, not refused).

Detection matrix (`hook2`, cases A-J, every case on all three targets):

| case | shape | outcome |
|---|---|---|
| A1 | two plain plugins with private versions of `fmtlite::version`, `RTLD_LOCAL` | each runs its own copy (1, 2) |
| A2 | the same with tables, non-Mason host | no report (N29) |
| A3 | the same, host carries the runtime (one shared index) | no report (N29) |
| B1 | Mason host exports `fmtlite::version` v1; plugin v2 loaded `RTLD_LOCAL` binds it | report naming both, exit 134 |
| B2 | B1 with `MASON_ODR=warn` | report; the plugin returns 1 (the bug the report names) |
| C1 | plugin 1 `RTLD_GLOBAL`, then plugin 2 `RTLD_LOCAL` (binds plugin 1's) | report |
| C2 | plugin 2 `RTLD_LOCAL`, then plugin 1 `RTLD_GLOBAL` (plugin 2 already bound its own) | no report |
| D | an `RTLD_LOCAL` plugin whose dependency defines the key differently (the dependency binds the plugin's copy) | report naming the dependency and the plugin |
| E | plugin 1 in the base namespace (global), plugin 2 in `dlmopen(LM_ID_NEWLM)` | no report (N30) |
| F | plugin 1 `RTLD_GLOBAL`, `dlclose`d, then plugin 2 `RTLD_GLOBAL` | no report |
| G | 300 shared objects at start-up (`DT_NEEDED`), 1,250 rows each, a violation in module 300 | detected, exit 134 (the first draft's prototype missed it past module 256) |
| H | G with `mmap` failing after 4 maps | one `odr-hook-out-of-memory`, 291 `odr-hook-stopped` |
| I | init order in a `.so` and an executable, bfd/lld/mold (qemu targets: bfd) | runtime, `constructor(101)`, default constructor |
| J | 8 threads × 300 `dlopen`/`dlclose` + `fork` | three runs, exit 0 |

Cost (AArch64, in-process time summed over every module's init entry, three runs; `MASON_ODR_TIME`):

| modules × rows | prototype 2 | first draft's pairwise prototype |
|---|---|---|
| 4 × 1,250 | 0.08-0.09 ms | 0.03 ms |
| 4 × 12,500 | 0.81-0.94 ms | 0.32 ms |
| 4 × 125,000 | 16.5-18.3 ms | 3.9 ms |
| 300 × 1,250, clean | 6.4-6.9 ms | 143 ms (review), and missed past 256 |
| 300 × 1,250, one violating key | 43.8-47.4 ms (one `dlsym` per module holding the key) | |

Linear in rows (about 17-35 ns a row: the index misses the cache at
large sizes), independent of the module count, and dearer than the
merge for few modules; the conflict path costs one `dlsym` per holder,
paid only when a key has two hashes. A clang-sized process (~35,000
rows) [estimate: ~1 ms]. Wall time per start with 4 × 125,000 rows:
0.76 ms off, 21 ms on (two runs). The x86-64 and i386 timings under qemu
emulate the code and are not costs. Making the index cheaper (a
pre-sized table from the summed row counts, a smaller slot) is WP-077's,
measured by the same script.

**O-55 (what only load time sees).** The static link proves what O-24
says; the hook adds a different build of a library at run time,
`dlopen`ed libraries and indirect dependencies.

**O-56 (compatibility).** A mortar-made executable with foreign
libraries: they have no table and are not checked. A mortar-made shared
object in a foreign executable: its init entry checks it against
whatever registered modules share its index. Intentional interposition
of a vague definition (`LD_PRELOAD` of a library that redefines an
inline function differently) is a violation the hook reports:
`MASON_ODR=warn` or `off` is the escape; interposing strong definitions
(`malloc`) is not in any table (N5).

**O-57 (the D-21 boundary).** K18c shows that a library loaded with
`RTLD_LOCAL` gets its own instance of a vague definition the executable
does not export (`spec/dyn.md` §14). The table does not change that; Q8
asks whether an M6 export class should.

**O-58 (off by default; when the hook is built).** No output gets a
table or the runtime unless `--odr-table=exported|full` is given (Q2 is
the operator's question whether a profile turns it on). The package that
ships the runtime (WP-077, §13) is cut only after experiment 5 passes
(it passed on prototype 2 on three targets, cases A-J above) **and** the
operator has decided Q2 and Q3.

**O-59 (what the verifier checks of the dynamic side).** §12.5's rules
for the table and the runtime.

## 11. Requests to kiln and hod

Each request stands alone for a reader without access to mortar's
workspace. Shared context: **Mason** is a C++ toolchain: kiln (front
end), hod (code generator), mortar (linker), tuck (debugger, future).
Mortar checks the one-definition rule across translation units at link
time from records the compiler writes into each object, in a section
`.mason.meta` that every other linker drops. The container is specified
in `spec/meta.md` and the checks in `spec/odr.md` of mortar's repository
(`git@github.com:christopherfretz/mortar.git`, branch `main` once this
draft is accepted; the rule ids below are theirs). Nothing in mortar
depends on these requests to link correctly; each one turns on a check.
Estimates are the designer's, for the receiving agent to correct.

### kiln

- **K-1 — the flag and the emission.** Add
  `-fmason-metadata[=min|default|detail|all]` and `-fno-mason-metadata`
  (`min`: record kinds 1 and 2; `default`: 1, 2, 3, 5; `detail`: + 4;
  `all`: + rows for definitions inlined away, flag `INLINED_ONLY`). Emit
  one IR global per module: `@__mason_meta = private constant [N x i8]
  c"…", section ".mason.meta", align 8, !exclude !0`, listed in
  `@llvm.compiler.used`; llc 19 and 22 already lower exactly this to the
  required section (`SHT_PROGBITS`, flags exactly `SHF_EXCLUDE`, align
  8, in no COMDAT group, no relocations: mortar `spec/meta.md` MT-1,
  MT-2). The bytes are one **unit**: a 32-byte header (`MASN`, version
  1.0, sizes, block count, string table) and one block per record kind,
  rows sorted by key (FNV-1a 64 of the mangled name); layouts in
  `spec/meta.md` §4 and the row tables of `spec/odr.md` O-15, O-17,
  O-35. Rows of kinds 2 and 5 carry no name: they are "anchored" to the
  object's symbol whose FNV-1a 64 equals the key. Where: at the end of
  module emission, when the final set of globals (linkage, comdat) is
  known (`kiln_codegen/src/module.rs`). Deterministic bytes (no pointer
  values, no hash-map order, paths after `-ffile-prefix-map`). C
  translation units write kinds 1 and 5 only. *Why:* nothing else
  reaches the linker. *Test:* mortar's test producer writes the same
  bytes for the same rows. *Estimate:* 2-3 days.
- **K-2 — kind 1 (TU summary).** Rows of (name, value) strings, sorted:
  `producer`, `source`, `odr.scheme`, `std`, `lang` (`c`/`c++`),
  `abi.rtti`, `abi.exceptions`, `abi.short-enums`, `abi.stdlib`,
  `abi.stdlib-abi`, `abi.stdlib-debug`, `abi.struct-return` (i386),
  `abi.char`, `abi.wchar`, `abi.long-double`, `isa`, with the `ENFORCE`
  flag on the layout-changing ones (`spec/meta.md` MT-26 lists which;
  `abi.rtti` is not enforced). From the driver's options and the target.
  *Estimate:* 1 day.
- **K-3 — the definition hasher (kind 2).** A pass over the semantic AST
  after template instantiation, for every emitted definition that may
  appear in more than one TU (inline functions and variables, template
  instantiations, explicit specializations, static members of class
  templates, out-of-line `static constexpr` members; `spec/odr.md`
  O-14), serializing canonically what `spec/odr.md` O-10 lists, hashed
  to 128 bits (FNV-1a 128 or SipHash-1-3, named by a scheme string), with
  the flags of O-15 and the odr-use analysis of O-16. The traps:
  - only **explicitly written** specifiers (`constexpr`, `noexcept`, …,
    including those a macro wrote) are hashed, never the implicit ones the
    language adds (an implicit destructor's C++20 `constexpr`, a lambda's
    implicit `constexpr`);
  - an enumerator reference hashes as (enum type, name, **value**);
  - local entities by declaration index, not spelling;
  - `__FILE__`, `__LINE__`, `__DATE__`, `__TIME__`, `__COUNTER__`,
    `__builtin_FILE` & co. and `std::source_location::current()` hash as
    placeholders. Today kiln's preprocessor expands builtin macros into
    scratch-buffer tokens (`kiln_pp/src/expand.rs`) whose spans are shared
    with `#`/`##` results, so sema sees a plain literal, possibly
    concatenated with neighbours (`__FILE__ ":"`). The hasher needs the
    **preprocessor to mark builtin-macro provenance** on the tokens and the
    **parser to carry it** to the literal nodes (a token flag and a
    literal-node flag): a preprocessor and parser change;
  - the same definition reached textually, through a precompiled header
    and through a header unit hashes equally;
  - implicitly defined members are synthesized before hashing;
    constant-evaluated initializers hash as expressions, not values (K9);
    `gnu_inline` definitions get no row.
  Keep the two corpora of O-13 (must-equal, must-differ) as kiln tests.
  *Estimate:* 3-4 weeks with the corpora (the first estimate, 2-3 weeks,
  omitted the preprocessor/parser provenance change and the PCH/header
  unit cases).
- **K-4 — kinds 3 and 4 (types).** For C++ TUs only: one row per class,
  struct, union or enum whose definition the TU contains and that an
  emitted definition reaches — through members, bases, parameters,
  returns, locals, `new`/`sizeof`/`alignof` operands, the type of every
  expression, every template argument, and the pointee or referee of
  every pointer or reference, when complete (`spec/odr.md` O-17). The
  64-bit layout hash of O-18 (size, alignment, bases with offsets,
  members with bit offsets and types, vptr offset, vtable slots by final
  overrider's mangled name, enumerators with values; **no RTTI slot**),
  from `RecordLayout` and `VTableLayout`. Kind 4 (readable layout) at
  `detail`. *Estimate:* 1-1.5 weeks.
- **K-5 — kind 5 (declarations).** One row per defined or referenced
  external function or variable: a 64-bit hash of the declared type's
  Itanium encoding with array bounds erased, `sizeof` for variables,
  flags (`spec/odr.md` O-35). For **C-linkage** entities, hash a
  canonical **C** type instead: an enumeration replaced by its compatible
  integer type (the one kiln chose), top-level parameter qualifiers
  dropped; unprototyped declarations get no row (O-33). *Estimate:* 4-5
  days.
- **K-6 — modules.** Use the K-3 hash for the comparison
  `kiln/spec/modules.md` §10 item 11 skips today (definitions merged from
  global module fragments; the planned "ODR key / structural hash"): one
  hash, two consumers. Mortar measured that clang 19 and 22 do not
  diagnose two named modules whose global module fragments define one
  inline function differently, and that the linked program runs the first
  module's copy (`spec/odr.md` K19); kiln can diagnose it at import. Rows
  of GMF definitions carry flag `GMF`. *Estimate:* kiln's.
- **K-7 — default.** On in a development profile when kiln has one, off
  otherwise until mortar's oracle (§12.3) is green.
- **K-8 — corpora as objects.** kiln's must-equal and must-differ tests
  (O-13) emit objects that mortar's L11 lane links (§12); the C corpus of
  O-33 likewise.

### hod

- **H-1 — `!exclude`.** hod today refuses `!exclude` metadata on a
  global (`unsupported construct 'global-metadata:exclude'`). Lower it as
  llc does: the global `@__mason_meta = private constant [N x i8] …,
  section ".mason.meta", align 8, !exclude !0` in `@llvm.compiler.used`
  becomes a section `.mason.meta`, `SHT_PROGBITS`, `sh_flags` exactly
  `SHF_EXCLUDE`, alignment 8, not in any group, with no relocations, kept
  although nothing references it. hod's object API can already write
  such a section (`SectionKind::Elf{sh_type, sh_flags, sh_entsize}`).
  Testable against `llc -O0` with hod's irdiff oracle. Without this,
  kiln's hod route cannot carry the records (kiln's llc route can).
  *Estimate:* 1-2 days.
- **H-2 — opacity.** hod never interprets the section's bytes: its
  contents equal the IR constant's bytes (a test).
- Nothing else: per-section content hashes are not needed
  (`spec/incr.md` IN-22); DWARF is unchanged.

### Landing order

Each step is useful and testable alone:

1. **mortar** WP-071 (container reader, writer, test producer; its
   acceptance test is experiment 7) and WP-072 (W1-W3, switches): no other
   project needed.
2. **hod** H-1: useful to any IR producer; tested by irdiff.
3. **kiln** K-1 + K-2: units with kind 1 only.
4. **kiln** K-3, then K-5, then K-4: each kind's mortar check runs under
   `--odr=strict` at once (WP-073 is written against the test producer)
   and turns on by default after the oracle (O-46).
5. **mortar** WP-075 (tables and O-24) after M2's dynamic executable;
   `.so` outputs with M4. WP-077 (the runtime) after O-58's gate.
6. **kiln** K-6 at any time after K-3.

### tuck (nothing asked)

Kinds 0x20-0x2f are reserved (`meta.md` MT-28) for build context, a
stable entity index across incremental relinks, and source context
(macros, include graph), and `--keep-mason-meta` (MT-32) keeps the units
in an output. Reserved now so that no other use takes the numbers.

## 12. Validation (L11) and the verifier

### 12.1 The violation corpus

Every K of §2, per boundary: two objects; one object and an extracted
archive member; an object made by `-r` (each reference linker, and
mortar at M4); executable and mortar-made shared object at static link
time (O-24); at load time (cases A-J of O-54). Per producer: kiln
(records: the expected diagnostic, by id and places, under
`--odr=strict` and, once O-47 lists the check, by default) and, before
kiln emits, the test producer of `meta.md` §12 writing the rows kiln
would (the model's constructions are its first cases); GCC and Clang (no
records: the W outcome of §8.1, which is "not detected" for most kinds,
recorded as such so that a change is visible). Link tests use the `#@`
format (`spec/harness.md` "Link tests") with a new directive `#@ diag:
<id> <file>...` (amendment A-6).

### 12.2 The legal corpus

N1-N30 of §3 as link tests that must link with **no finding of any
default-on check**, and with no finding at `--odr=strict` except where §3
names one as a known strict-only finding (N20-N22: W3; N28: C-linkage
kind 5 until O-33's canonical C type lands). The corpora of experiment
1 (§8.1) and the mixed protobuf build are part of it.

### 12.3 The false-positive oracle

- **libc++'s test suite**, built by kiln with `-fmason-metadata=all`
  against a libc++ built the same way, every test linked by mortar with
  `--odr=strict --odr-stats`: zero findings of each check before O-46
  moves it. Its `-std` matrix is the evidence for O-32 and Q6.
- **The open-source lanes** (`docs/VALIDATION.md`): built with GCC and
  Clang (the W checks; and once per lane with GCC and Clang alternating
  per TU, which is where N3 lives), and with kiln where kiln builds them.
- **The C corpus** of O-33 for C-linkage kind 5.
- A finding is either a real violation (added to the violation corpus
  with its reason and an allowlist entry for that lane) or a false
  positive (the check is fixed or moved back to strict, and the program
  joins §12.2).

### 12.4 Mutation tests and cost

Mutations of kiln's objects and of test-producer units, each with its
expected outcome: one hash byte flipped → exactly one `odr-violation`;
two rows' hashes swapped → two; a unit truncated → one `meta-malformed`,
the neighbours read; rows unsorted → `meta-malformed`; the scheme renamed
in one unit → no comparison, counted by `--odr-stats`; a symbol renamed
by `objcopy --redefine-sym` → no report (N12); the container removed by
`eu-strip`, by `llvm-strip` (default and `--strip-all`) or by
`llvm-objcopy --strip-non-alloc` → no report and no warning; the symbols
removed by a default `strip` → rows unanchored, no report; the rows of
an output's `.note.mason` reordered → the verifier's failure (12.5).
The cost of O-45 is measured by the bench lane on Mason workloads, with
each check on against `--odr=off`.

### 12.5 The verifier

`mortar_plumb` depends on `mortar_elf` and `mortar_diag` only, so it
re-reads `meta.md`'s format itself (about 150 lines: the format is small
on purpose, and a second reader is the point). Proposed rules (amendment
A-7 to `spec/plumb.md`):

- an executable or shared object contains no `.mason.meta` unless
  `--keep-mason-meta` (MT-31, MT-32); a relocatable output's
  `.mason.meta` is the concatenation of the live inputs' units in input
  order (MT-30);
- a `.note.mason`, if present, lies in a `PT_NOTE` with `p_align` 8; its
  rows are sorted; each row's key is the FNV-1a 64 of its `.dynsym` name;
  each row's hash is the first 8 bytes of the kind 2 row of **the
  keeper's unit** for that name (O-51; the keeper is `comdat[G]`'s file
  for a vague definition, the defining file otherwise); the row set
  equals O-50's (preemptible exports only); each row's flags equal the
  keeper's row's;
- with a table, the output has exactly one `.init_array.00000` and one
  `.fini_array.00000` entry pointing into the runtime's input section,
  and that section's bytes equal mortar's embedded runtime for the target
  (O-53).

Whether `mortar_plumb` should also re-run the comparison (a second
implementation of the verdict) is Q11.

### 12.6 Tests of the rules

| rule | test | what it shows |
|---|---|---|
| O-1, O-3, O-46, O-47 | T-O-1 | a CI check that every check O-47 lists as on has a recorded zero-finding oracle run in the landed record of the package that moved it; the legal corpus has no finding of a default-on check |
| O-2 | T-O-2 | `static` functions of one name in two TUs, with rows forced by the test producer: no comparison |
| O-4 | T-O-4 | anchored, unanchored (`--redefine-sym`), ambiguous (two names, one FNV-1a 64, constructed) and versioned (N6) rows |
| O-10..O-13 | T-O-10 (kiln) | the corpora of O-13, as kiln tests and as L11 links |
| O-14, O-15 | T-O-14 | rows per definition kind; K13's flags; N9; N22 (strong + `VAGUE`, equal) |
| O-16 | T-O-16 | K12 under strict; nothing by default |
| O-17, O-18 | T-O-17 | K4 (also pointer-only), K7, K16a, N24 (no RTTI bit) |
| O-19 | T-O-19 | a kind 4 entry renders the first difference |
| O-20, O-21 | T-O-21 | the optimisation and the normative rule report the same findings over the whole corpus, K13 included |
| O-22 | T-O-22 | kind 3 and 5 rows from 3 units grouped in `FileId` order |
| O-23 | T-O-23 | K17 with and without `--odr-check-archives` |
| O-24 | T-O-24 | (a) an executable exporting a vague definition a mortar-made `.so` also defines differently; the same unexported; (b) two `.so` inputs; (c) K18b |
| O-25 | T-O-25 | K16c with two vtable copies; F5 silent |
| O-30, O-31 | T-O-30 | K16a/K16e/K16d notes under strict; `abi.rtti` never a finding |
| O-32 | T-O-32 | `mixed-std-system-row` → a stats count; `mixed-std-user-body` → a finding |
| O-33 | T-O-33 | N27, N28 (strict only), K14b |
| O-34 | T-O-34 | kind 5 against a foreign definition's size |
| O-35 | T-O-35 | K14, N15 |
| O-40, O-41 | T-O-41 | each mode's severities; `--odr=off` output byte-identical; `--detect-odr-violations` a no-op with its note |
| O-42 | T-O-42 | `meta.md` T-MT-42: bytes read counted |
| O-44 | T-O-44 | the stats line's fields on a corpus link |
| O-45 | T-O-45 | the bench lane's comparison |
| O-50, O-51 | T-O-50 | the row set (preemptible only; protected, hidden, `-Bsymbolic` excluded); the keeper's hash after a warned link |
| O-52 | T-O-52 | `note-final`'s tolerance matrix |
| O-53, O-54, O-56 | T-O-54 | `hook2` cases A-J on three targets (WP-077), the verifier's runtime-bytes rule |
| O-55, O-57 | T-O-57 | K18c's two outcomes unchanged by the table |
| O-58 | T-O-58 | no table and no runtime without `--odr-table` |
| O-59 | T-O-59 | §12.5's rules on good and mutated outputs |

### 12.7 The model (experiments 2 and 3)

Until kiln emits records, `model/odrmodel.py` constructs the records of
13 cases by hand (each definition's serialization written out under the
revised O-10, and under the first draft's rules where the case is about
a change), encodes real units, assembles them, joins them to the real
objects with `ld -r` (so anchoring runs against real symbols), reads them
back and applies the revised comparison: K7 (old rules: missed; revised:
`odr-violation` + `odr-type-mismatch`), K4 pointer-only (layout), N25
(two cases: nothing), N26 (a stats count), a user `#if __cplusplus` body
(a finding), N24 (nothing), K16c in both shapes (X3 and O-25), N22
(nothing), N27 (nothing), N28 (a strict-only C finding). All 13 as
expected. It is the first content of WP-071's test producer and WP-073's
corpus, not evidence about kiln's hasher.

## 13. Work packages

| WP | what | crates / files | depends on | tests alone against | estimate | parallel with |
|---|---|---|---|---|---|---|
| WP-071 | container reader and writer; MT-6 classification; `meta-malformed`/`meta-version`; the test producer; the `--odr=off` path and A-8 (MT-42, O-42). **Acceptance test = experiment 7**: real units written by the test producer at the row counts of the heavy TU's object and of protoc's 167 objects (kind 2 per vague symbol, kind 5 per defined/undefined global, kind 3 per DWARF type); unit sizes by kind; bytes read with `--odr=off` and in the default mode under the staged store (zero container bytes for objects over 64 KiB); the cost of reading and grouping the rows (O-22) on the bench lane against O-45. MT-41's gate is set from these numbers in its landed record | `mortar_meta`; `mortar_input` (A-8); `mortar_resolve` (A-1's row); `xtask` (producer) | A-1, A-2, A-8 applied | `.s` units assembled by GNU as | 4-5 days [estimate] | WP-072 |
| WP-072 | W1-W3 (W1, W2 on; W3, W4 strict); the diagnostic kinds; `--odr=`, `--no-odr-check`, `--odr-stats`, `--detect-odr-violations` no-op; O-47's register as code | `mortar_odr`, `mortar_diag`, `mortar_cli` | A-4, A-5; `comdat_copies` (M1) | GCC/Clang objects of the catalogue and the legal corpus, experiment 1's corpora | 4-5 days | WP-071 |
| WP-073 | record checks: kinds 1, 2, 3, 5 (O-4-O-35, O-25, O-33), all `--odr=strict`; notes from kind 1; the L11 static corpus (the model's 13 cases first) | `mortar_odr`, `tests/link/odr/` | WP-071, WP-072 | the test producer (no kiln) | 1.5-2 weeks | kiln K-1..K-5 |
| WP-074 | the Mason L11 lane and the false-positive oracle (§12.3, the C corpus of O-33); moves checks per O-46 | `xtask`, `tests/mason/` | WP-073; kiln K-1-K-5; hod H-1 | kiln objects | 1-1.5 weeks | WP-075 |
| WP-075 | loadable table (O-50-O-52) for executables, the O-24 check against `.so` inputs; `[M4]` the same for `.so` outputs | `mortar_odr`, `mortar_synth`, `mortar_layout` | WP-073; M2's dynamic executable; A-3; M4 for `.so` | test-producer units; `note-final` | 1-1.5 weeks | WP-074 |
| WP-076 | opt-ins: `--odr-check-archives`, `--odr-table=full`, `--keep-mason-meta`, `-r` pass-through (MT-30, with M4) | `mortar_odr`, `mortar_meta` | WP-073 | | 4 days | WP-075 |
| WP-077 | the runtime (O-53, O-54) and the hook; its qemu test matrix (cases A-J, three targets); the cheaper index | `runtime/mason_rt/`, `mortar_odr`, `xtask` | **gated (O-58)**: experiment 5 passed (prototype 2) **and** the operator's Q2 and Q3; WP-075 | `hook2` | 1.5-2 weeks | WP-078 |
| WP-078 | verifier rules (§12.5) | `mortar_plumb` | A-7; WP-071 (format only) | the outputs of WP-073/075/077 | 3-4 days | WP-077 |
| WP-079 | apply the amendments of §14 | specs | this draft accepted | — | 1 day | first |

WP-071 and WP-072 need no other project and can start the day this
draft is accepted. M6's "done when" (`docs/ROADMAP.md`) is WP-074's
oracle green with WP-075 landed; WP-077 is not on M6's path.

## 14. Requests to other specs (ranked: cheapest now, dearest later)

1. **A-1 `spec/resolve.md` §10** — a row before the `SHF_EXCLUDE` row,
   with MT-6's predicate: "| a container: name `.mason.meta`,
   `SHT_PROGBITS`, flags exactly `SHF_EXCLUDE` (`spec/meta.md` MT-6) |
   `Meta` | `Dropped(Consumed)`; `mortar_meta` reads it only when a check
   that needs it is on | GNU ld, lld, mold drop it (`meta.md` §2) |". A
   section of that name that is not a container takes the existing rows
   (MT-6). And §6.7: "`mortar_odr` (`spec/odr.md` O-21) reads
   `comdat_copies`; the symbol sets of the copies are W1-W3's input." And
   §13: "a file's units are part of its summary (I-file)".
2. **A-8 `spec/input.md` §11.1** — the staged store's row "every other
   section of at most 64 KiB" gains "except a Mason container
   (`spec/meta.md` MT-6), which is never resident; `mortar_meta` reads it
   with positioned reads, only when a check that needs it is on
   (`spec/odr.md` O-42)". The whole-file row for objects of at most 64 KiB
   is unchanged (one read, container included; `meta.md` MT-42 states the
   cost and Q15 the alternative). A test-only byte counter per file range
   for T-MT-42.
3. **A-2 `spec/model.md`** — `SectionKind::Meta`; per file, the list of
   its `Meta` sections (so that `mortar_odr` finds units without a
   rescan); in §8, phase 40's `seq` for row diagnostics without a symbol:
   "`2^40 +` the row's index in its unit". `comdat_copies` stays as it is.
4. **A-3 `spec/dyn.md`** — §14 "M6: ODR data" becomes: "`.note.mason`
   (`spec/odr.md` O-52) is an allocated note of alignment 8, placed with
   the other notes (layout L-60 rank 2); the runtime object is an input
   mortar adds (O-53) whose `.init_array.00000` and `.fini_array.00000`
   entries are ordinary; the exported object `__mason_odr_state` is
   exported like any default-visibility definition; O-24 reads the
   tables of shared-object inputs". D-21 unchanged at M2; Q8's class, if
   accepted, is added there by amendment.
5. **A-5 `spec/cli.md`** — rows `--odr=warn|error|strict|off` (default
   `warn`), `--no-odr-check` (= `--odr=off`), `--odr-check-archives`,
   `--odr-table=off|exported|full` (default `off`), `--odr-stats`,
   `--keep-mason-meta` (mortar spellings); `--detect-odr-violations`
   (gold) accepted as a no-op with a note; §3.9 gains `odr` in
   `plan.diagnostics`.
6. **A-4 `spec/diag.md`** — §3.1's area "one-definition rule" with the
   rows of §9.1; §3.2's notes on them.
7. **A-6 `spec/harness.md`** — "Link tests": the directive `#@ diag:`;
   "bench": the Mason workloads and the comparison with `--odr=off`
   (O-45); the L11 lane, the oracle of §12.3 and the C corpus.
8. **A-7 `spec/plumb.md`** — the rules of §12.5 (a stage after M2's).

`spec/layout.md` needs no amendment: O-52 no longer asks for a
`PT_NOTE` of its own.

## 15. Open questions

The operator's decisions (Q1, Q2, Q3, Q6) are written out with options
and costs in the WP-070 record (`spec/landed/meta/WP-070.md`, "Operator
decisions"); their recommendations are repeated here.

- **Q1. Default severity.** *Recommend:* `--odr=warn` until each check
  has been on at zero for a release, then the operator decides the move
  to error per check (O-40, O-46).
- **Q2. The loadable table and hook by default.** *Recommend:* off
  everywhere (O-58) until WP-077 has run a release in the lanes; then on
  in a development profile with `MASON_ODR=warn`.
- **Q3. How the runtime's bytes are made.** *Recommend:* C source and
  generated object bytes checked in, embedded in mortar, byte-identical
  rebuild check in CI (O-53).
- **Q4. Anchored rows vs own strings.** *Recommend:* anchored (MT-12):
  kinds 1 and 2 cost 3-6 % of an object anchored, ~20 % named (`size`,
  kind 2 only, measured counts; percentages [estimate]); the cost is that
  a row whose symbol was renamed or inlined away is not compared by
  default (X1, X8).
- **Q5. `odr-internal-reference`.** *Recommend:* strict only; revisit if
  kiln's analysis exempts `SYSTEM` rows and the corpus shows zero.
- **Q6. Mixed `-std` on user rows.** *Recommend:* keep it a finding (O-32),
  operator's decision (record).
- **Q7. An ISA-safe COMDAT keeper** (keep the copy of the unit with the
  smallest feature set). *Recommend:* not now; report (O-31) and revisit
  with a measured need.
- **Q8. Export vague and unique definitions from executables** so that a
  `dlopen`ed library shares them (K18c; `spec/dyn.md` D-21).
  *Recommend:* not by default (it changes behavior relative to every
  other linker); an option later.
- **Q9. Unextracted archive members.** *Recommend:* opt-in only (O-23).
- **Q10. The hash function.** *Recommend:* kiln's choice under a scheme
  name; FNV-1a 128 (already in kiln) or SipHash-1-3-128.
- **Q11. A second implementation of the verdict in `mortar_plumb`.**
  *Recommend:* yes for kind 2 at L11 only (cheap: compare hashes per
  key), not in `--verify`.
- **Q12.** Withdrawn: W3 is never proposed as a default (N20-N22).
- **Q13. DWARF-based foreign checks** (type sizes and enumerators from
  `.debug_info` for GCC/Clang objects with `-g`). *Recommend:* defer:
  `.debug_info` is the largest part of a `-g` input, reading it breaks
  O-42 unless opt-in, and hod emits no type units to deduplicate by.
- **Q14. Type rows in the loadable table.** *Recommend:* `=full` only
  (O-50); two libraries with private same-named types are common and
  never bind each other.
- **Q15. Small objects and A-8.** An object of at most 64 KiB is read
  whole in one read today, container included. *Recommend:* keep the
  single read (a second read per small object costs more than the bytes
  it saves); the alternative is to read around the container once the
  section headers are known.
- **Q16. K16c in the catalogue's shape (X3).** The ruling compares two
  `_ZTV` copies only. *Recommend:* add, under `--odr=strict`, "a vtable
  defined by an `abi.rtti=off` unit while an `abi.rtti=on` unit defines
  the type's `_ZTI`" (the model catches the catalogue's shape with it and
  stays silent on F5); coordinator's decision.

## Appendix A. Experiments

Scripts in `/Volumes/AgentWork/mortar-helper/.build/review-meta/`
(uncommitted; the record `spec/landed/meta/WP-070.md` lists their
outputs). Tool versions: g++ 14.2.0 (Debian 14.2.0-19, also as
`x86_64-linux-gnu-` and `i686-linux-gnu-`), clang 19.1.7 and 22.1.8, GNU
ld 2.44, lld 19.1.7, mold 2.37.1, gdb 16.3, lldb 19.1.7, elfutils 0.192,
glibc 2.41, qemu 10.0.13; kiln and hod release builds of the workspace.

| name | script | what |
|---|---|---|
| `catalogue` | `catalogue.sh` | K1-K16c: build, link (3 linkers × 2 orders × 2 compilers), run |
| `catalogue2` | `catalogue2.sh` | K16d, K16e, K17, K18, K18b, K18c; GCC LTO `-Wodr`, `-Wlto-type-mismatch`; GCC ASan |
| `weak-catalogue` | `run-all.sh` + `weakcheck.py` | W1-W5 and F over the catalogue's objects |
| `legal` | `legal.sh` | B1-B3 bytes across options/compilers; mixed static link with every extracted `libstdc++.a`/`libc.a` member through W1-W5; legal shapes |
| `container` | `container.sh` | encodings P, N, U, K, G, R × 3 targets × (3 linkers final and `-r`, 7 strip/objcopy variants, `--redefine-sym`, `ar`), eu-elflint |
| `size` | `size.sh` | entity counts of a libc++-heavy TU: kiln+hod (libc++, libstdc++), clang++ 22 (`-O0`, `-O2`, `-g`), g++ |
| `dynamic` | `dynamic.sh`, `hook/mason_rt.c`, `hook/gentab.py`, `patchelf_mini.py` | note / segment / `DT_` tolerance; the first prototype's detection and cost; `.preinit_array` order |
| experiment 1 | `rev/batch1.sh`: `adv/wcheck.py` over six corpora, `adv/legal-fp.sh` (F1-F5, N9), `adv/mixed-protobuf.sh` | W1-W3 from the rule text |
| experiments 2, 3 | `model/odrmodel.py` | §12.7 |
| experiment 3 (baseline) | `adv/std-mix.sh` | `-std=c++17` vs `c++20` vague definitions of the heavy TU |
| experiment 4 | `adv/k12proxy.py` (protoc; heavy TU × 2 + `libc++.a`) | O-16's proxy |
| experiment 5 | `hook2/hook2.sh`, `hook2/mason_rt2.c`, `hook2/gentab2.py`, `hook2/patch2.py`, `rev/batch2.sh`, `adv/preinit-dynamic.c` | O-54's cases A-J on three targets; `DT_STRTAB` |
| experiment 6 | `adv/note-final.sh` (type `0x4d4f4401`) | O-52's final note |
| experiment 8 | `adv/container-adv.sh` | MT-5's matrix |
| `k19`, `n6`, `n11` | `rev/k19-n6-n11.sh` | K19 (clang 19/22 modules), N6, N11 |
| `k3count` | `rev/k3count.sh` | O-17's row count proxy |

Experiment 7 (real units at real row counts) is WP-071's acceptance test
(§13).

## Appendix B. Not verified

- SIGILL of K16d on a CPU without AVX2 (qemu-user executes AVX2).
- Clang's ASan ODR checker (runtime not installed); gold
  `--detect-odr-violations`, `abidiff`, MSVC, patchelf (not installed).
- musl: its loader's `RTLD_DEFAULT` scope, `DT_STRTAB` convention and
  constructor locking (musl is not installed; Debian's `musl-tools`
  1.2.5 is available through `apt` but not in `scripts/provision.sh`).
- `#pragma pack`, point of instantiation, `-std` bodies in user
  templates: not built (§2).
- The standard's clause numbers marked `[std, unverified clause]`.
- `eu-elflint` on a `DT_` tag alone; whether distribution QA runs it.
- Whether `-r` by the references keeps the relative order of same-named
  sections (`meta.md` MT-16).
- What `strip` does to a non-allocated `.mason.meta` in an executable
  (MT-32).
- Every cost marked `[estimate]`: O-4, O-21, O-22, O-24, O-30, the W
  checks' cost in mortar (only Python prototypes ran); the hook's cost on
  real x86 hardware.
- Everything that needs kiln's hasher: the false-positive rates of the
  record checks, of the odr-use analysis and of the C canonical types;
  the model (§12.7) checks the rules, not the hasher.
- The O-24 check itself (specified, not prototyped).
- The hook's `dlvsym` path for versioned names and its `secure_getenv`
  and `write`-only output (the prototype uses stdio).
