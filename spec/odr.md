# spec: one-definition-rule checks (`mortar_odr`)

**Status: final draft (WP-070, revision 2 after the adversarial review
and re-review, `briefs/reviews/review-070-meta-odr.md` and
`review-070-meta-odr-re.md`, and the coordinator's rulings). The
container and the static checks (§1-§9, §10.1-§10.2, O-24) are accepted
for implementation; the load-time hook (§10.3) is a design direction for
a prototype package, not a rule; the operator's decisions are §15.** Rules are `O-n`. The
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
bytes as with them off (`--odr=off`, O-41), except the loadable table
(§10.1) when `--odr-table` asks for it, and, if a runtime ever ships
(§10.3), the runtime object (an input, so it would shift every later
`FileId` and address).

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
documented exclusion** (X0-X12 below); a **false positive is a defect**:
the check is fixed or moved back to `--odr=strict`, and the program joins
the legal corpus (§3, §12.2). O-47 is the register of which checks are on
today and why; O-46 is how a check moves.

**The guarantee (of `--odr=strict`).** This paragraph describes what
`--odr=strict` guarantees; the default mode guarantees the subset that
O-47 lists as on (below). Assume

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
- **A4** the link runs with `--odr=strict`.

Then every violation of kinds K1, K2, K4-K9, K11-K14b, K16a-K16e, K19 of
§2 **between definitions emitted as symbols in at least two live files**
of a static link is reported, by the check named in §2; and K18 at
static link time **between the output and a shared-object input that
stays in its `DT_NEEDED` and whose table has a row for the same name and
version** (O-24, O-50: exported definitions, protected and `-Bsymbolic`
ones included). A violation is missed only if two different definitions
have equal 128-bit hashes (O-12; about 2^-128 per pair, no adversary
assumed), or if it is one of these exclusions.

**Missed even under `--odr=strict`:**

- **X1** a definition inlined in every TU that uses it (no symbol), unless
  the TUs were compiled with `-fmason-metadata=all` (named rows without
  a symbol, compared under strict only, O-15). K1, K2, K8 and K11 at `-O2` are often
  of this shape.
- **X2** a variable whose every use was constant-folded, so that no TU
  emits it (K9's folded uses): no symbol, no row.
- **X3** K16c in the catalogue's shape, where only one TU emits the
  vtable (the other uses the type's `type_info` through `dynamic_cast`):
  O-25 compares two COMDAT copies of one `_ZTV` only. The model shows the
  miss and an extension that would catch it (Q16, open).
- **X4** a type whose layout some TU did not compute (it is incomplete
  there, or only passed by pointer or reference): that TU writes no kind
  3 row for it, so only the TUs that computed its layout are compared
  (O-17).
- **X7a** C translation units' types: C writes no kind 3 rows (O-33).
- **X8** a TU whose container or symbols were lost (A2's tools): its rows
  are absent or unanchored; nothing reports the loss (MT-5).
- **X9** objects without records (GCC, Clang, foreign archives): only the
  W checks of §8 apply (best effort, below).
- **X10** `-flto` objects (bitcode) carry no unit.
- **X12** K17 (a definition in an archive member that is not linked) is
  reported only with `--odr-check-archives` (O-23); K18 that only load
  time sees (a different build of a library at run time, `dlopen`ed
  libraries, libraries the link never named, indirect dependencies, and
  libraries without a table) is not reported: the load-time hook is a
  prototype direction, not a rule (§10.3).
- **X13** two units with different `odr.scheme` strings (typically
  different kiln versions): their kind 2 and 3 rows are incomparable, an
  `--odr-stats` count only (O-11).
- **X14** a dependent default argument of an instantiated function that
  differs between TUs and is not used inside the definition: not hashed
  until a serializer handles dependent nodes (O-10, K-6).
- **X15** a definition whose declarator contains a lambda: no kind 2 row
  (O-10, O-14).
- **X16** attributes not on the scheme's closed list, and C++26 contracts
  (`pre`, `post`, `contract_assert`): not hashed in scheme 1 (O-10).
- **X17** a difference hidden inside a whole-literal placeholder: a
  concatenated string literal with an unclassifiable piece, or a `#`/`##`
  result, hashes as one placeholder until kiln's side table and
  preprocessor mark exist (O-10).
- **X18** a type that fails O-17's syntactic TU-local test without being
  TU-local (a specialization over a pointer to a global object, a
  typedef-named unnamed class [requested of kiln, O-17]): no kind 3 row
  (O-17). Kiln's K-4 also writes none for: a class with
  `[[clang::internal_linkage]]`; a specialization over a floating-point
  or pointer value, a template template argument or a vector type;
  AArch64's implicit `std::__va_list`; a class one of whose bases,
  member types or final overriders' classes fails the test (O-18).
- **X18a** an enumeration that differs between two units and is used
  only through its enumerators as constants in function bodies (K7's
  shape), while kiln's row rule is "named in the type of an emitted
  function or variable" (O-17).
- **X22** a class definition that differs between two units without a
  difference O-18's layout hash covers (a member function's
  declaration, a default member initializer nobody uses, access): kind
  3's `hash` is zero until kiln's K-6 (O-17).
- **X23** a reference, inside a hashed definition, to an entity or type
  whose encoding has a component numbered per translation unit (an
  unnamed type at namespace scope, a closure with no mangling number
  declared outside the definition): it hashes as one placeholder (O-10).
- **X24** scheme `kiln-odr-1.d2` only (O-10, "a callee that is a
  function template specialization"): the caller of a specialization
  whose template signature holds a dependent name that one unit
  redeclared with another spelling hashes differently in that unit: a
  FALSE finding under `--odr=strict` (kind 2 is strict's, O-47; fmt's
  chrono test, two keys per standard library). Not excluded by mortar
  (it sees two hashes, not the callee symbols); it ends with kiln's
  K-3e and its scheme.
- **X19** C-linkage signature differences that O-33's conservative
  canonical C type maps to one string (record types, `va_list`
  spellings, `noexcept`, reference vs pointer, `restrict`): equal under
  kind 5 by construction, so never reported.

**Missed by today's default, reported under `--odr=strict`:**

- **X5** K12 (`USES_INTERNAL`): `odr-internal-reference` is strict only
  (O-16; libstdc++'s own `std::mutex` sets it).
- **X6** differences in `SYSTEM` rows between units of different `-std`:
  an `--odr-stats` count by default, warnings under strict (O-32).
- **X7b** C-linkage declaration mismatches (O-33).
- **X11** K16d, K16e (ISA and struct-return conventions): kind 1 notes
  (O-30, O-31).
- **X0** every record-based check (kinds 1, 2, 3, 5; O-24; O-25): strict
  only until O-46 moves it (O-47's register).

- **X20** objects without records: a weak definition outside every
  COMDAT group that lost resolution is not compared by W2 or W3, in any
  mode (§8.1 compares the COMDAT copies and the resolved definition
  only; `wcheck.py` would, and finds nothing on the corpora of §8.1).

- **X21** objects without records: W4 compares a relocation whose
  target is unit-local (§8.1, W4's rule) by the string or merged entry
  it refers to, and any other unit-local target (a local object or
  function, a section symbol and addend outside a mergeable section, a
  type that is not absolute) by its kind (code or data) only; a key on
  one side and a unit-local target on the other agree. Missed: two
  copies that point at different local objects or functions, or at
  different contents of them (`inline int* p = &x;` with a
  unit-local `x` of 1 in one unit and 2 in another), at different
  strings in a plain `.rodata` (GCC `-O0`), or at a global on one side
  and a local on the other. A bounded comparison of a local target's
  bytes over its `st_size` and its own relocations (depth-bounded,
  cycles cut) would catch the named-local form (GCC `-O2`), not a
  section symbol and addend (no extent); proposed in
  `spec/landed/meta/WP-146.md`, not implemented (the corpus needs none
  for zero false findings).

Numbers X1-X12 are those of revision 1; X7 is split into X7a and X7b;
X13-X19 were added by the peer review of kiln and hod (2026-11-15); X20
by WP-072; X21 by WP-146.

**Best effort (no proof).** Objects without records: W1 and W2 of §8 are
on by default and catch K5 and K6 when two copies' sizes differ; nothing
else measured. Mixed links (one side with records): O-34.

**What the default guarantees today.** Following O-1, only checks that
have run on a corpus are on: W1, W2, and `meta-malformed` for a
section named `.mason.meta` that is not a container (O-47); the
container findings `meta-malformed` and `meta-version` are reported
under strict only, the one mode that reads containers (O-42). The default mode guarantees what those checks find and nothing
more; it grows into the guarantee above check by check, as O-46 moves
each one and records it in O-47.

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
| K4 | class with an extra member in one TU | wrong values: `fb=` garbage (`a,b`), `fa=0` (`b,a`) | no: no symbol carries a size | kind 3 layout (O-18), when both TUs computed the class's layout (O-17; the catalogue's TUs access members, so they do; model `K4-pointer-only`); not when one TU only passes `S*` along or sees `S` incomplete (X4) |
| K5 | class with a different set of virtual functions | ran correctly by luck; `_ZTV1B` 48 vs 40 bytes | **W1** (size), W4 | kind 3 layout (vtable shape) |
| K6 | class with different base classes | clang `a,b`: SIGBUS (135) under all three; g++ ran; `_ZTI1D` 56 vs 24, `_ZTV1D` 72 vs 40 | **W1**, W4 | kind 3 layout |
| K7 | enum with different enumerators, used only as constants in `inline int isB(int e){ return e == B; }` | wrong results, order-dependent (`fa=1 fb=0` / `fa=0 fb=1`) | no | kind 2 hash (an enumerator hashes as type, name and **value**, O-10) and kind 3 (O-17: the enumeration's enumerators appear in an emitted definition) — model `K7-revised`; the first draft's rules missed it (`K7-old-rules`) |
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
| K18 | inline function in two shared objects, different bodies | default visibility: the first in lookup order wins for both libraries (`-lA -lB`: 1,1; `-lB -lA`: 2,2); with `-fvisibility-inlines-hidden` each keeps its own (1,2) | no | at static link time when both libraries stay in the output's `DT_NEEDED` and carry tables (O-24 b); at load time not reported (X12; the hook is a prototype, §10.3) |
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
| N12 | a vendored copy renamed with `objcopy --redefine-sym` or a prefix macro | the records still name the old symbol | anchoring (O-4): a row is compared only if its object defines the symbol it names. The one exception is chosen by the user: units compiled at `-fmason-metadata=all` and linked under `--odr=strict` (O-15: a named row without a symbol is compared as an inlined-away definition, and `objcopy` leaves the row's name as it was) |
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
| N29 | two `dlopen(RTLD_LOCAL)` plugins, each with its own version of a header-only library's inline function at default visibility | one key, two hashes in one process (the first draft's hook aborted, exit 134) | no static check sees it (no link has both); a must-not-report case of the hook's prototype matrix (§10.3; `hook2` A2, A3 pass on three targets) |
| N30 | the same plugin loaded in the base namespace and in a `dlmopen(LM_ID_NEWLM)` namespace | one key, two hashes | a must-not-report case of the hook's prototype matrix (§10.3; `hook2` E passes) |
| N31 | two TUs, each with its own unnamed-namespace `struct Impl`, a `std::optional<Impl>`, a local class `Local` of a `static` function, and a namespace-scope `static` lambda, with different members (`kind3-tulocal`) | equal `<type>` encodings, different layouts (`N12_GLOBAL__N_14ImplE` 4 vs 56 bytes, `St8optionalIN12_GLOBAL__N_14ImplEE` 8 vs 64, `ZL8local_tivE5Local` 4 vs 56; closures `NL3lamMUlvE_E` (g++), `3$_0` (clang)) | TU-local types are never keys and write no kind 3 row (O-2, O-17). Built (`n31`): links and runs (exit 0) with g++ and clang++ 19 under GNU ld, lld and mold; W1-W3 zero with each compiler and mixed |
| N32 | `--wrap=malloc` (`free`, `open`) on a C++ program | `malloc` and `__wrap_malloc` both defined; a reference named `malloc` binds elsewhere | `--wrap` redirects references (`spec/resolve.md` §7.1), never definitions; the wrapped key is not vague linkage. A wrapped vague-linkage function with a dropped COMDAT copy is refused by resolution before the ODR pass (§7.1 item 3, WP-153), so the table never sees it |

**O-2 (keys never compared).** Local symbols, names of a file's
`STB_LOCAL` symbols, and **translation-unit-local** entities are never
keys, for every record kind. TU-local (C++20 [basic.link] p13-15,
`[std, unverified clause]`): an entity with internal linkage (an
unnamed-namespace member, a `static` function or variable); an entity
with no linkage (a local class, including one of an internal-linkage or
TU-local function); a closure type that is not declared in an inline or
templated entity with external linkage, or in a default argument or
initializer of one; and every type, function or variable whose name,
or one of whose template arguments, involves a TU-local entity
(`std::optional<(anonymous)::S>`, `std::vector<decltype(lam)>`). Two
TUs may legally define different TU-local types of one spelling: N31
(`kind3-tulocal`: `N12_GLOBAL__N_14ImplE` 4 vs 56 bytes,
`St8optionalIN12_GLOBAL__N_14ImplEE` 8 vs 64, a local class
`ZL8local_tivE5Local` 4 vs 56). Kiln needs no TU-locality engine for
this: for kind 3 it writes a row only for a type that passes O-17's
syntactic test (a conservative under-approximation; anything it cannot
prove non-TU-local gets no row); kind 2 rows of instantiations over TU-local types name local symbols
(internal linkage), so they are never anchored and are not compared.

**O-3 (the oracle).** Before a check is on by default, and on every
release after: the false-positive oracle of §12.3 runs the check and
must report **zero** findings, or each finding is a real violation
recorded in the corpus with its reason. "Zero" is measured, not argued;
the measurement is recorded where O-46 says.

## 4. The entity key

**O-4 (key, anchoring).** An entity's key is the pair (name, class):

- the **name** is the mangled name as it appears in the symbol table,
  including ABI tags (`B5cxx11`), module attachment (`W`, Itanium modules
  mangling as kiln writes it) and a symbol version that `.symver` wrote
  into the object (`foo@V1` and `foo@@V2` are two keys, N6); a version
  that a version script assigns at link time is not part of the key but
  is part of the identity where it exists (`meta.md` MT-13, the one
  statement of the rule; O-24, O-51); for a type, the Itanium
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
instantiation, serialized canonically. The rule throughout (peer review
of kiln, 2026-11-15): **where kiln cannot tell, the item is not hashed**
(a possible miss, a documented exclusion), never guessed (a possible
false positive).

- **Nodes.** Node kinds and operators of the written expressions and
  statements. Of the implicit nodes sema adds, only the **resolved
  function of each call and of each user-defined conversion** hashes (by
  mangled name), so that a call that resolves to another overload in
  another TU differs. Structural implicit nodes do **not** hash:
  temporary materialization, lvalue-to-rvalue and other built-in
  implicit conversions, cleanups and full-expression wrappers, constant
  expression wrappers, elidable copies (so the hash does not move with a
  sema refactor or with `-std` changing copy elision, C++14 vs C++17).
- **Literals by value and type**, not spelling (kiln stores value and
  type): `16` and `0x10` are equal, `16` and `16u` differ.
- **The declarator**: return and parameter types, cv- and ref-qualifiers,
  storage, and these specifiers, under a rule that needs no AST bit for
  "written": `consteval`, `constinit` and a function's `inline` as kiln
  records them; `constexpr` **except** on implicitly declared members,
  defaulted functions and closure call operators (never hashed there:
  kiln cannot tell written from implied, and C++20 makes implicit
  destructors `constexpr`, N25); `noexcept` as the **effective** exception
  specification (kiln stores implicit and written alike; the effective
  one is what callers rely on); a **variable's** `inline` is not hashed
  (not recorded; an inline variable is vague linkage either way). A
  specifier written through a macro is written (N26 stays a real
  difference, handled by O-32).
- **Attributes: a closed list**, named by the scheme (O-11); every
  attribute not on it is **not hashed** (kiln does not store unknown or
  accepted-but-unimplemented attributes). Scheme 1's list, for kind 2:
  `alignas` and `gnu::aligned`, `packed`, `no_unique_address`,
  calling-convention attributes, `[[noreturn]]`
  ([dcl.attr.noreturn] requires it in every TU's first declaration,
  `[std, unverified clause]`), `gnu::target` / `target_clones`
  (multiversioning changes which body runs), `[[assume(e)]]` (hashes `e`:
  it changes what is undefined), `[[indeterminate]]` (C++26, a
  cross-TU declaration rule). Considered and **not** hashed: `weak`,
  `section` and `visibility` (they change the symbol, which the linker
  sees directly, not the meaning: a symbol's visibility is in its
  `st_other`, and the copies' visibilities are merged by
  `spec/resolve.md`), `exclude_from_explicit_instantiation` (which
  symbols an explicit instantiation emits, not what the definition
  means) and `abi_tag` (it is part of the mangled name: two definitions
  that differ in it are two keys and are never compared). Ruling of
  2026-11-17 on kiln's question (K-3b): `visibility` was on the first
  list; libc++ marks nearly every inline function with all three
  (`_LIBCPP_HIDE_FROM_ABI`), 44 % of its definitions, and hashing them
  bought nothing the symbol table does not already say; `trivial_abi`, `ms_struct` and `gcc_struct` are
  properties of a class and belong to kind 3 (O-17's definition hash and
  O-18's layout), not to every user. C++26 contracts (`pre`, `post`,
  `contract_assert`) are **not hashed in scheme 1** (X16).
- **Default arguments** (K8: [dcl.fct.default] requires them to agree,
  `[std, unverified clause]`): the default arguments of the function's
  **namespace-scope** declarations accumulated at the end of the TU
  (block-scope redeclarations have their own set and are not hashed); for
  an instantiated function template or member of a class template, only
  **non-dependent** default arguments (dependent ones exist only as the
  pattern and are not instantiated for hashing, [temp.inst]; X14); and,
  at every call inside the definition, the arguments as sema built them,
  default arguments included (so a default argument the definition uses is
  hashed at the call, instantiated).
- **Default member initializers** used inside the definition (by
  aggregate initialization or by a constructor, including a synthesized
  one) hash as the initializer expression at that use; the class's own
  copy is kind 3's (O-17).
- **Referenced entities by name**: an external entity by its mangled
  name, a local entity (parameter, variable, label, structured binding)
  by its declaration index in the definition, never by its spelling; an
  internal-linkage entity by its **mangled** name, as kiln would emit
  the symbol (it separates overloads), and the flag of O-16. An entity
  or type whose encoding has a component **numbered per translation
  unit** (an unnamed class or enumeration at namespace scope, a closure
  without a mangling number declared outside the definition, and
  everything named through one) hashes as ONE placeholder: the number
  depends on the unit's other declarations, so anything else could
  differ between two units of equal source (X23; ruling of 2026-11-17).
  Unnamed types and closures declared INSIDE the definition are not
  such: they hash by their index in the definition, below.
- **Enumerators** as the triple (the enumeration's `<type>` encoding, the
  enumerator's name, its value): K7's `isB`. An enumerator of an
  unnamed enumeration: (the placeholder above, name, value): name and
  value do not depend on the numbering, and `enum { N = 4 };` in a
  header is the common shape.
- **Types** by mangled encoding (a type's definition is kind 3's).
- **Lambdas**: a closure by its mangling number, or, when it has none, by
  its index among the definition's lambdas in source order; then its
  **resolved capture set** (each captured entity by local index and
  capture kind, implicit captures included, so `[=]` and `[x]` capturing
  only `x` are equal) and its body. The set is a list in sema's capture
  order (the closure's field order); each element is the captured
  entity's declaration index in the enclosing definition and the
  capture kind; `this` is a token of its own. A lambda in a default argument or a
  default member initializer is hashed where that expression is hashed.
  A definition whose **declarator** contains a lambda (C++20 lambdas in
  unevaluated operands or template arguments of a signature) gets **no
  row**: such declarations in two TUs declare different entities
  ([temp.over.link], `[std, unverified clause]`; X15). Local classes are
  hashed like lambdas, by index.
- **Implicitly defined members** as kiln synthesizes them (sema builds
  real bodies; N10), under the specifier rule above. A virtual
  destructor's serialization includes the mangled name of the
  deallocation function its definition selects, if sema records it
  (the same for every variant symbol, O-14); if not, it is not hashed.
- **Builtin-macro and source-location values** hash as placeholders (one
  per kind), never as their values: `__LINE__`, `__COUNTER__`, `__DATE__`,
  `__TIME__` and a `__FILE__` string are recognisable today from their
  expansion record (kiln's preprocessor gives each its own
  `ExpansionKind::Macro { name }` at the macro-name token); `__func__`,
  `__builtin_FILE`/`_LINE`/`_FUNCTION`/`_COLUMN` and
  `std::source_location::current()` are node kinds of their own. Two cases
  need new kiln work (K-3): a string literal concatenated from pieces
  (`"at " __FILE__ ":"`) keeps only bytes and its first and last piece's
  location, so per-piece provenance needs a **sema side table**; and a
  `#`/`##` result carries the enclosing macro's name, not its operands'
  origin, so a stringized or pasted builtin (`STR(__FILE__)`) needs a
  **small preprocessor mark**. Until each exists, the conservative form
  applies: a concatenated literal any of whose pieces cannot be
  classified, and every `#`/`##` result, hash whole as one placeholder
  (X17); never as text, which would break must-equal across include-path
  spellings. **Scheme 1 keeps the conservative form for all of K-3**
  (ruling of 2026-11-17): a precompiled header is built without the
  flag, so a side table recorded only under the flag would make the
  textual and the precompiled route hash apart, and must-equal across
  the routes outranks detection. Per-piece provenance is a later
  scheme, and only if the table is recorded whatever the flag.
- **Modules**: a definition imported from a named module takes the hash
  computed when the module was built and stored in its BMI (direction,
  shared with K-6; stored only when the module is built with the flag:
  from a BMI without hashes the importer writes **no row**, a miss); textual inclusion, precompiled headers (identical AST
  snapshots) and header units (in-process parses) are hashed in place and
  must agree.

It is **insensitive** to: whitespace, comments, macro names, literal
spelling, `-O`, `-g`, `-fPIC`, `-ffunction-sections`, the order of
unrelated declarations, source paths and line numbers, the values of the
placeholders above, how the definition was reached (textual `#include`,
a precompiled header, a header unit, a module), and every attribute not
on the scheme's list (among them `likely`, `unlikely`, `deprecated`,
`nodiscard`, `maybe_unused`, `fallthrough`, `no_sanitize*`, `gnu::used`,
`gnu::cold`, `gnu::hot`, `weak`, `section`).

What it does **not** see: the code generator's choices (the reason K16d
is kind 1's), and options that change meaning without changing tokens
(the reason kind 1 exists).

**kiln K-3c, accepted** (2026-11-17; kiln PR #783, scheme
`kiln-odr-1.d2`: a kind 2 row carries the definition hash). Where the
paragraphs above leave a choice, kiln's are these, none objected to:
1. An elidable copy or move is transparent (the source expression
   hashes in its place: `return T();` is equal at C++14 and C++17), as
   are temporaries, their materialization and binding, and cleanups.
2. An initializer list hashes in its semantic form (type, union member,
   every element in field order, the array filler); a designated
   initializer is its resolved field; a skipped field hashes as
   value-initialization, which differs from a written `.x = {}` (other
   tokens).
3. A constructor: its initializers in initialization order as (kind,
   target, initializer), a base by type, a member by owner and name;
   then the body. A virtual destructor adds the selected deallocation
   function (a token and its mangled name), in every variant.
4. An implicit or defaulted special member: one fixed token, then the
   initializers and body the compiler built; its exception-specification
   byte is fixed (lazy analysis cannot move the hash).
5. A default argument and a default member initializer are hashed as
   their expression AT THE USE, inside the caller. Default arguments on
   a definition's own parameters keep the row `PARTIAL` until K-3e.
6. A variable: declared type and initializer as an expression, never
   its value (K9; N22's C++14 and C++17 forms are equal).
7. A type trait: the trait, its types and its value (two units in which
   a trait's value differs with equal tokens are ill-formed already: a
   finding there is true); `noexcept(e)`: its operand only.
8. A constructor or destructor callee is named by the hasher's own
   mangling of the variant called, never by the symbol the code
   generator emits (a unit that folds C1 into C2 hashes as one that
   does not).
`visibility` no longer makes a row `PARTIAL`.

**A callee that is a function template specialization (ruled
2026-11-17, kiln's Q1 on K-3c).** The mangler spells a dependent name
in a function template's signature as the unit's first declaration of
it did (kiln and Clang 22 alike), so one specialization can have two
symbols in two units of a legal program (fmt's chrono test redeclares
`fmt::detail::has_tm_gmtoff` fully qualified: `format_utc_offset<tm>`
has another symbol there, and its callers, of equal tokens, hash
differently in `kiln-odr-1.d2`). The hash names the ENTITY: such a
callee is hashed by its template (qualified name, template parameter
list, and the signature hashed by the hasher with every name by entity,
so independent of spelling; one computation per template declaration)
and its template arguments, never by its symbol. Two overloads that
differ only in a dependent signature (`enable_if_t<A<T>> f(T)` beside
`enable_if_t<!A<T>> f(T)`) stay different callees: a caller that
selects another overload in another unit is what kind 2 is for. kiln
may instead hash the template without its signature and flag the
caller's row `PARTIAL` whenever an overload of the same name and
template-parameter count is in scope at the call (the miss is then
counted, not silent). This is kiln's K-3e, with a scheme bump; until
then it is X24.

**O-11 (schemes).** Kind 1's `odr.scheme` names the serialization, the
attribute list of O-10, the provenance form (per-piece or whole-literal
placeholders) and the hash function. **Two rows** (ruling of
2026-11-17, so that a change of the definition hasher does not make
type rows incomparable): `odr.scheme` names the DEFINITION
serialization (kind 2's `hash`, kind 3's `hash`); `odr.layout-scheme`
names the LAYOUT serialization (kind 3's `layout`, O-18). A unit with
kind 3 rows and no `odr.layout-scheme` row uses its `odr.scheme` for
layouts (the units kiln wrote before the row existed). Wherever this
spec says "equal schemes" for kind 3's `layout`, `size`, `align` and
flags, the layout scheme is meant; for a `hash`, `odr.scheme`. Each of
the two has the bump rule below for what it names. [Mortar reads
`odr.layout-scheme` from WP-151 on: kind 3's layout classes are formed
between units of equal layout scheme, its definition hashes between
units of equal `odr.scheme`; a unit without `odr.scheme` (kiln writes
it only with kind 2 rows) has no hash compared, no diagnostic, and is
counted among the pairs not compared. A layout difference across units
of different `odr.scheme` but equal layout scheme is reported as any
layout difference; a hash difference across layout schemes has the
note "the definitions differ (members, initializers or member
declarations)", without "the layouts agree", since the layouts were
not compared.] **Bump rule**: any change to what
is serialized or how changes the scheme string; kiln guards it with a
golden-hash test (a fixed corpus whose hashes are checked in, so that an
unannounced serialization change fails kiln's tests). Rows of kinds 2
and 3 are compared only between units whose scheme strings are equal.
Two different scheme strings are **incomparable**: never a diagnostic,
only an `--odr-stats` count of the unit pairs not compared (X13).
**One exception, under `--odr=strict` only** (WP-073, a check of its own
in O-47 that never moves to on by default): kind 3 rows of one key are
compared across LAYOUT schemes, and across units without one, on `size`,
`align` and the flags `ENUM`, `UNION`, `POLYMORPHIC` and `HAS_VBASES`,
never on `hash` or `layout`; a difference is `odr-type-mismatch` with a
note that the layout schemes differ ("the translation units use
different layout schemes (odr.layout-scheme, or odr.scheme without it):
compared on size, alignment and kind only (spec/odr.md O-11, strict)";
WP-151), and `--odr-stats` counts the groups so compared. The reason: none of these depends on the scheme. The key is
FNV-1a 64 of the `<type>` encoding (O-4, MT-13), which is the Itanium
mangling, not the scheme's serialization; size and alignment are the
type's `sizeof` and `alignof` in bytes; the four flags are fixed bits of
the row (O-17's table, versioned with the container, not with the
scheme) and name properties of the language (`std::is_enum`,
`std::is_union`, `std::is_polymorphic`, a virtual base). O-11's scheme
names only the serialization, the attribute list, the provenance form
and the hash function, so none of them can change these fields. In
practice objects from different kiln versions will often be
incomparable; a full rebuild makes them comparable again.

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
  static member (N22). Added with the peer review, one per rule of O-10:
  `16` vs `0x10` (literals by value); `return T();` at `-std=c++14` and
  `c++17` (structural nodes and elision not hashed); a defaulted
  (`= default`) special member and an implicit one under C++17 and C++20
  (`constexpr` not hashed there); a destructor written `noexcept` and the
  same destructor implicitly `noexcept` (effective `noexcept`); a
  `static constexpr` data member's definition hashed in a C++14 and a
  C++17 TU (implicitly `inline` only in C++17; a variable's `inline` is
  not hashed); an unknown attribute
  (`[[vendor::x]]`), `[[gnu::section("s")]]`, `[[deprecated]]` and
  `pre`/`post` contracts present in one TU only (not on the list);
  `STR(__FILE__)` and `"at " __FILE__` reached through two include
  spellings (placeholders); a function template whose dependent default
  argument is used by a call in one TU's other code only (dependent
  defaults not hashed); a block-scope redeclaration adding a default
  argument in one TU; `[=]` vs `[x]` capturing only `x`; a definition
  from a named module, in the module's own unit and in an importer (the
  BMI's hash).
- **must-differ**: K1, K2, K8, K9, K11, K12, K13; **K7 in the `isB(int)`
  shape** (the enum appears only as a constant); one enumerator's value
  changed with its name kept; a changed `alignas`/`packed`; a changed
  default member initializer; an explicitly written `constexpr` added to
  an ordinary inline function; an **effective** `noexcept` changed (a
  `noexcept(false)` member making an implicit destructor potentially
  throwing). Added with the peer review: `16` vs `16u`; a call resolving to
  another overload because one TU declares an extra overload before the
  definition; a default member initializer used by aggregate
  initialization inside the definition, changed; `[[noreturn]]` added;
  `gnu::aligned` changed; `gnu::target("avx2")` added; `[[assume(x > 0)]]`
  vs `[[assume(x > 1)]]`; `[[indeterminate]]` added; a lambda's resolved
  capture set changed (by copy vs by reference); a lambda body changed
  where the enclosing function is inlined everywhere but the closure's
  `operator()` is emitted (its own row, O-14); with per-piece provenance
  only, `"at " __FILE__` vs `"in " __FILE__`.

A must-equal failure is a false positive in every user's link: it blocks
release of the hasher.

**O-14 (which definitions have rows).** Kind 2 rows exist for
definitions that may legally appear in more than one TU, and for those
that clash with them: inline functions and variables, template
instantiations (implicit or explicit), static data members of class
templates, explicit specializations, the out-of-line members of class
templates, and the out-of-line definition of a `static constexpr` data
member (N22's strong row). **Closure members**: the closure's
`operator()` and each specialization of a generic lambda's call operator
that is emitted as a symbol get a row (vague linkage), whose hash is the
lambda's serialization of O-10 (closure number, resolved captures, body);
the static invoker and the conversion function to a function pointer get
**no** row (synthesized forwarding with nothing written; a difference
shows in `operator()`). Reason, from must-equal safety: the row hashes
exactly the sub-serialization that the enclosing definition's hash
already contains, under the same rules (`constexpr` never hashed on a
closure call operator), so identical source cannot make it differ where
the enclosing hash agrees; it adds detection where the enclosing
function was inlined everywhere and the closure's operator was emitted
(K11 at `-O2`, X1's shape). A definition whose declarator contains a
lambda gets no row (X15). **One definition, several symbols** (ruling
of 2026-11-17): every symbol the object defines that names the
definition gets its own row, with the same hash and flags: the
constructor and destructor variants (`C1`/`C2`, `D0`/`D1`/`D2`) and
aliases among them (a row is per key, and two units may emit different
subsets). Symbols a definition OWNS get none: static locals (`_ZZ…`:
their type and initializer hash inside the function), guard variables,
thunks, thread-local wrappers and initializers (`_ZTW`, `_ZTH`), and
vtables, VTTs and `type_info` objects (kind 3's). Not for ordinary strong functions and
variables (two of them are already `duplicate-symbol`, `spec/resolve.md`
§3.8; their declarations are kind 5's), not for `gnu_inline` (N7), not
for weak definitions that are not vague linkage (N5), and **not from C
translation units** (C has no vague linkage; N21).

**kiln K-3a, accepted** (2026-11-17; kiln's first kind 2 rows: every row
`PARTIAL`, the hash over the declared type only). The row set kiln
chose, none of it against this section:
1. kind 2 rows are written from level `min`;
2. a unit with kind 2 rows carries `odr.scheme=kiln-odr-1.d0` (`.d2`
   when the hash covers bodies); `odr.layout-scheme=kiln-odr-1` goes
   with kind 3 rows. A reader therefore meets units with kind 3 rows and
   NO `odr.scheme` row (O-11: nothing of kind 2 to compare);
3. a member of an explicitly specialized class is `EXPLICIT_SPEC`; its
   place is its own definition; a C++14 out-of-line `constexpr` member's
   place is the in-class line;
4. a definition with an asm label is keyed by the label;
5. NO row for: aliases, ifuncs, internal linkage, weak definitions that
   are not vague, vague `always_inline` functions and structors, thunks,
   guards, TLS wrappers, a coroutine's outlined parts;
6. `selectany` is vague linkage;
7. a definition the optimizer dropped keeps its row, unanchored (named);
8. X15 is decided by closure names and default-argument lambdas; an
   inline variable of closure type hashes as the per-TU placeholder;
9. two-deep unnamed nesting is described in kind 4 (such a class newly
   gets a kind 3 row).
Item 5's "vague `always_inline` functions and structors" narrows the
first sentence above: without a row nothing is compared, so a
difference between two units' copies is a miss, never a false report.

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
`PARTIAL` (the hasher skipped a construct its SCHEME says it covers:
the row is compared under `--odr=strict` only, and a difference is a
warning, never an error; see below), 6
`INLINED_ONLY` (no symbol emitted; a producer that knows may set it;
kiln does not, see below), 7 `GMF` (declared in a global module fragment),
8 `SYSTEM` (defined in a system header); bits 9-14 reserved (0); bit 15
is reserved for `.note.mason` rows (`NONPREEMPTIBLE`, O-51) and is 0 in
kind 2 rows. An `EXPLICIT_SPEC` row and an
`IMPLICIT_INST` row for one key are a violation whatever the hashes
(K13: the implicit instantiation was used where the specialization was
not visible, [temp.expl.spec] `[std, unverified clause]`);
`EXPLICIT_INST` and `IMPLICIT_INST` are compatible (N9). A strong row
without `VAGUE` and a `VAGUE` row of one key (N22) are compared by hash
only: no flag combination other than `EXPLICIT_SPEC` + `IMPLICIT_INST` is
a finding by itself.

*`PARTIAL`* (rulings of 2026-11-17). A hasher may land in steps: a
construct it does not cover yet hashes as a fixed token per node kind
(not as nothing) and sets `PARTIAL`, so that equal source still hashes
equal; every step that changes what is hashed changes `odr.scheme`
(O-11). The flag is NOT set for what the scheme itself excludes (X14,
X16, X17: the row is complete under its scheme; X15 is no row), or
nearly every row of a standard library would carry it. The K13 flag
check does not read the hash and applies to `PARTIAL` rows; it is a
warning while any row of the group is `PARTIAL`, an error when none is.

*Rows of definitions inlined away* (`-fmason-metadata=all`; ruling of
2026-11-17). Kiln writes IR before optimization and cannot know which
definitions the optimizer deletes, so at `all` it writes the `name`
string on every vague-linkage row and never sets `INLINED_ONLY`. A row
with a name whose file defines a symbol of that name is compared as an
anchored row is. A row with a name whose file defines NO such symbol
describes a definition without a symbol and is compared **under
`--odr=strict` only**, as a row flagged `INLINED_ONLY` is. The price is
N12's: a symbol renamed with `objcopy` keeps its row's name, so its
definition is compared under the old name in such a link (the units
were compiled at `all` and the link asked for `strict`). [Implemented
by WP-151: "defines a symbol" is a defined global of the row's name in
the row's file, the first in symbol order; such a row is anchored on
it (its place line names the symbol, and it can be the keeper). A named
row without one is counted among the unanchored rows when strict does
not run, and compared beside the `INLINED_ONLY` rows under strict. While
kind 2 runs under strict only (O-47), the first half changes the place
lines and the keeper of strict's findings, not which links report.]

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

**O-17 (kind 3 row; which types).** A second, independent line of
evidence, for C++ TUs (O-33 excludes C). After the peer review the rule
is the cheap one kiln proposed, which can only miss, never invent:

- one row per **class, struct or union whose layout this TU computed**
  (kiln's `RecordLayout` exists for it: the TU accessed a member, took
  `sizeof`/`alignof`, constructed, copied or destroyed it, used it by
  value, derived from it, or emitted its vtable), and per **enumeration
  whose definition the TU contains and whose type or enumerators appear in
  an emitted definition** (K7's enumerator used as a constant);
- and only if the type is **provably not TU-local** by the syntactic test
  below. Otherwise no row (N31).

*The syntactic test [kiln]* (a conservative under-approximation of
[basic.link] p13-15 that needs no TU-locality engine; `[std, unverified
clause]`). A class or enumeration gets a row only if all hold:

1. it has a name of its own: **unnamed types never get a row** (anonymous
   structs and unions, unnamed enumerations, closure types, and an
   unnamed class with a typedef name for linkage purposes);
2. every enclosing scope is a **named** namespace (inline namespaces
   included) or a class that itself passes this test: no unnamed
   namespace at any depth, no enclosing function (local classes), no
   enclosing lambda;
3. if it is a template specialization: its template passes 1-2, and
   every template argument is either a type built only from fundamental
   types and types that pass 1-3 (through pointers, references, arrays,
   function types, member pointers and cv-qualifiers), or an integral,
   enumeration or `nullptr` value. A non-type argument that names or
   points to an entity, a template template argument, or a pack
   containing any argument that fails gives **no row**.

*Enumerations: which subset.* The one-definition rule on an enumeration
does not depend on its use, so ANY subset of "the complete, named, not
TU-local enumerations the unit defines" is sound (it can only miss).
The rule above ("appears in an emitted definition") needs a usage
record kiln does not keep; kiln's K-4 wrote no enumeration rows; its
K-4b (d854b23bc) writes them under the preferred rule below, "complete
and referenced anywhere in the unit" (sema's referenced mark on the
enumeration or an enumerator; a member enumeration of a class template
specialization counts when the template's own is marked), so X18a no
longer applies to kiln's rows. The rule accepted for the first ones was "complete in the unit and named
in the type of an emitted function or variable", or, if kiln has a
reference mark on the enumeration or its enumerators, "complete and
referenced anywhere in the unit" (preferred: the former misses K7's
own shape, an enumerator used only as a constant in a body: X18a).

*Typedef-named unnamed classes* (`typedef struct { … } T;`, a typedef
name for linkage purposes) are excluded by item 1 today (X18).
**Requested of kiln** (2026-11-17, after K-3 and K-6): a row for them,
keyed by the encoding of the typedef name, when 2 and 3 hold. The
reason: it is the C-header idiom of old code bases, where a layout that
differs between units by a macro or a packing pragma is what this check
is for.

So `std::optional<(anonymous)::S>`, `std::vector<std::pair<int,
(anonymous)::S*>>`, `Local` and every closure write no row, while
`std::optional<int>` and `ns::Widget<ns::Gadget*>` write one. A type that
fails the test without being TU-local (a specialization over a pointer
to a global object, a typedef-named unnamed class) is a miss (X18),
never a false positive. The test is made on the AST, never by parsing the
mangled name; a kiln test cross-checks it against "the `<type>` encoding
contains `_GLOBAL__N`, a local-entity `Z…E` or an unnamed `Ul`/`Ut`
component" on the corpus.

K4 through an `S*`-only use is covered when both TUs computed `S`'s
layout; a TU that only passes `S*` along does not, and is X4. The model's
`K4-pointer-only` case is caught by kind 3 when both TUs access a member
(they do in the catalogue's shape).

Size: clang 22's DWARF of the heavy TU has 301 complete definitions at
`-g` and 334 at `-g -fstandalone-debug` (`k3count`); "layout computed"
is at most the former, and TU-local types write no row, so 334 is an
upper proxy [estimate until kiln writes kind 3; MT-41's numbers use it].

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

Because K-4 lands before K-3 (§11's order), a unit whose scheme has no
definition serializer yet writes `hash` as all zero bytes: "not hashed".
It stays zero through K-3 and is designed with K-6 (ruling of
2026-11-17): a class definition "as declared" includes member templates
(dependent nodes) and members declared or instantiated lazily, which
cannot be made must-equal-safe before K-6's serializer (X22).
Two rows are compared on `hash` only when both are non-zero and their
schemes are equal; `layout`, `size`, `align` and the flags `ENUM`,
`UNION`, `POLYMORPHIC` and `HAS_VBASES` within one scheme, and across
schemes only `size`, `align` and those flags, under `--odr=strict`
(O-11's exception).

**O-18 (layout hash) [kiln].** 64 bits over: size, alignment; per base
(in declaration order) its key, offset and virtuality; per non-static
data member its offset in bits, bit-field width, and type encoding; the
vptr's offset; the vtable shape: slot count and, per slot, the mangled
name of the final overrider's declaration (not of its body); for an
enumeration, the underlying type and the (name, value) list. **Not** the
RTTI slot: whether a vtable's `type_info` slot is filled depends on
`-frtti`, which is legal to mix (N24); K16c is O-25's. It also covers
the class attributes of O-10's list that are a class's property:
`trivial_abi` (changes how the class is passed), `ms_struct` /
`gcc_struct` and `packed` (their effect on offsets is already in the
layout; the attribute is hashed too, so the record says why). From kiln's
`RecordLayout` (`kiln_ast/src/types/layout.rs`) and `VTableLayout`
(`kiln_ast/src/class.rs`). Kiln's choices in K-4 (scheme `kiln-odr-1`),
accepted 2026-11-17: a member of unnamed class type is hashed by its
structure and one of unnamed enumeration type as its underlying type;
the vtable shape is the whole vtable group (secondary vtables too),
without the RTTI slots; `gcc_struct` counts when written, `ms_struct`
when in effect; a class gets NO row when a base, a member's type or the
class of a final overrider fails O-17's test (a miss, X18); `PARTIAL`
and `ENUM` are never set and `hash` is zero until K-3. Measured by kiln
against mortar e3f9ed7: a class that differs between two units is
`odr-type-mismatch`; three real units with libstdc++ (1,020 rows) and
libc++ (1,331 rows) under `--odr=strict` give 0 record findings; the
heavy unit has 337 kind 3 rows (18,888 B, names 14,342 B).
Kiln's K-4b (kiln d854b23bc; accepted 2026-11-17): **an enumeration's
layout hash** is the token `E`, one byte (bit 0 scoped, bit 1 a fixed
underlying type), the underlying type's `<type>` string, a `u32` count
and per enumerator its name and `u64` value: `enum E : int` and `enum
E` hash differently (different definitions of one key; scoped and
unscoped enumerations share a `<type>` encoding, so the byte is
needed), and `ENUM` is set. **A virtual direct base** is hashed twice
(the direct-base list, then the list of all virtual bases). **A
member of unnamed class type** is hashed as the member (no type
string) followed by the nested class's own structure. **A vtable
slot's index** is the component's index in the group as hashed (RTTI
slots left out; vcall, vbase and offset-to-top components counted);
pure and deleted slots are named `__cxa_pure_virtual` and
`__cxa_deleted_virtual`. Kind 4's items (O-19) follow the same order:
direct bases, all virtual bases, members, the vptr, vtable slots; a
slot item's `value` is that index. Measured by kiln on the heavy unit:
`default` 72,976 B = 6.1 % of the `-O0 -g` object (368 kind 3 rows of
which 31 enumerations, 800 kind 5 rows); `detail` 125,792 B (kind 4:
24,704 B, 368 entries, 906 items, 10,231 B of strings); under
`--odr=strict` 0 findings (libstdc++ 1,113 rows, libc++ 1,430).

**O-19 (kind 4, layout details).** Optional (`-fmason-metadata=detail`):
per type, the list O-18 hashes, in readable form (member names, offsets,
type strings), so that a diagnostic can say *what* differs. Mortar reads
kind 4 only to render a diagnostic that kind 3 already decided. The
encoding (fixed 2026-11-17; kiln writes it after K-3, mortar reads it
with WP-145): the block's body is a sequence of **entries**, each
aligned to 8 bytes from the body's start; a kind 3 row's `details` is
its entry's byte offset in the body. An entry is `{u32 count; u32
reserved}` (`reserved` 0) followed by `count` **items** of 24 bytes:

| offset | field | |
|---|---|---|
| 0 | `u32 name` | a string offset as kind 3's `name`; 0 when the item has none |
| 4 | `u32 type` | a type string, the one O-18 hashes; 0 when none |
| 8 | `u64 value` | by item kind, below |
| 16 | `u32 width` | a bit-field's width in bits, else 0 |
| 20 | `u32 flags` | bits 0-3 the item kind; bit 4 `BITFIELD`; bit 5 `VIRTUAL` |

Item kinds: 0 a non-static data member (`name`, `type`, `value` its
offset in bits, `width` and `BITFIELD` for a bit-field); 1 a base
(`type` the base's `<type>` encoding, `value` its offset in bits,
`VIRTUAL`); 2 the vptr (`value` its offset in bits); 3 a vtable slot
(`name` the mangled name of the final overrider's declaration, `value`
the slot's index over the whole group); 4 an enumerator (`name`,
`value`: the enumerator's value as a 64-bit two's complement number,
sign-extended when the underlying type is signed, zero-extended
otherwise; ruling of 2026-11-17); 5 an enumeration's underlying type (`type`). **A member of
unnamed class type** (K-4b: a kind 0 item with `name` 0 and `type` 0,
followed by the nested class's own items) carries in `width` the
number of items that follow it and describe it, counted through every
depth (a nested unnamed member's own items included, and that member's
item too), so that a reader knows where the nested items end without
guessing; such an item is never a bit-field (`BITFIELD` clear), so
`width` has no other meaning there. Ruling of 2026-11-17 (WP-134's
finding: without the count an unnamed member followed by a named one
reads the same as an unnamed member with two members); kiln writes 0
there until its next metadata package, and a reader that sees 0 on
such an item treats every following item as the class's own, as before
(mortar's reader: WP-151; a count is of raw items, kinds the reader
does not know included, and a count that runs past the enclosing
class's items or the entry is cut at their end). The items stand
in exactly the order O-18 hashes what they describe, one for everything
O-18 hashes that has an item kind, so the first item that differs
between two entries is the first difference. A reader ignores an item
kind it does not know and a set `reserved`.

*What a writer may do, and what a reader does* (WP-145; the lenient
reader-side answers, for kiln to confirm). The body starts right after
the block's 16-byte header; a kind 4 block's `row_size` and `row_count`
are 0 (MT §7) and a reader does not use them. Entries may stand in any
order, leave gaps (zero or not) between them, overlap, and be shared by
several rows (two rows whose `details` are equal name one entry). An
entry with `count` 0 is legal (a class without members, bases or
vptr). `count` has no maximum but the block: a reader checks `offset +
8 + 24 × count` against the body's end in 64 bits. Bits 6-31 of an
item's `flags` are ignored, as are `BITFIELD` and `VIRTUAL` on an item
kind they do not belong to (they still take part in comparing two
items); an item of an unknown kind is skipped whole, its string fields
unchecked. A block of version above 1 is skipped (MT-25: O-19 does not
say later versions only append). *Validity* (MT-14's rules, applied when
the entry is read): a row's `details` that is not a multiple of 8, an
entry header or items that run past the body, a string field of an item
of a known kind past the string table or without NUL, or `details` in a
unit with no kind 4 block, is `meta-malformed` (§9.1). A reader reads
and validates kind 4 only on a finding (MT-42): an entry no finding
needs is never read, so its malformations are not reported, and a
malformed entry never takes the unit's other rows out of the
comparison.

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
default: such a member is not part of the program (N14). The member's
section headers are read by every link already (the input store parses
every member it is given); the option adds the read of the members'
containers and of the live units' (O-42).

Implemented (WP-076, `mortar_odr::archives`;
`spec/landed/meta/WP-076.md`):

- "Not extracted" is resolution's final answer: a member that a later
  reference extracted is live and is compared by the record checks
  (O-10..O-17) in the modes that run them, never by O-23. Only members of
  archives (`ar` and thin): objects between `--start-lib` and
  `--end-lib` that were not loaded are not read (an open question of
  the record, §7). A member that is not ELF, or has no container, is never read
  (T-O-23's read counter).
- Each kind 2 row of a member is anchored to the member's own defined
  global (O-4) and compared with the first kind 2 row of the same class
  in the keeper's unit (the unit of the definition resolution kept). Only
  the hash is compared: K13's flags are not (O-15 compares them under
  strict, for live units). A row of a member or of the keeper flagged
  `PARTIAL` is compared under `--odr=strict` only; below strict it is
  set aside and counted in `--odr-stats`' "rows set aside" (WP-151: the
  option is opt-in in every mode, and O-15 compares `PARTIAL` rows under
  strict only). Units of different schemes, or without one,
  are counted as pairs not compared (O-11); C units are not compared
  (O-14); O-32's `SYSTEM`/`-std` case is reported under strict only, as
  for live units. At most one warning per (member, symbol).
- A warning in `warn`, `error` and `strict` (`--fatal-warnings` makes it
  an error); nothing under `--odr=off`. A member unit that does not parse
  gives no rows and is not reported, in any mode: the member is not part
  of the program (strict's `meta-malformed` covers live units only).
- The warning's places: "defined in" the keeper, "defined differently
  in" `libx.a(m.o)` with the row's source line; a note says why the
  member is not linked. `--odr-stats` adds "archive members N (U units,
  B bytes, C rows compared)".
- Cost (measured, §5 of the record): with no container in any member,
  the option costs nothing measurable; with containers, the reads of
  the members' and the live units' containers and one binary search per
  member row.

**O-24 (the static check against shared-object inputs: "dynamic link
time" without a runtime).** `[M6]` for executables, `[M4]` for
shared-object outputs. Prototyped (WP-075, `spec/landed/meta/WP-075.md`:
every shape below on aarch64 and x86-64, linked by GNU ld and by lld,
run under glibc 2.41's loader; musl's loader is not measured). When the
output is dynamic, mortar compares, before it writes anything, the
output's rows with the tables (`.note.mason`, O-50..O-52) of the shared
objects that are **in the output's scope**: those it reads as inputs
(named on the command line or found by `-l`) **and** that end up in the
output's `DT_NEEDED`. A library that `--as-needed` dropped is never
loaded with this output and is not compared; the indirect `DT_NEEDED`
closure is not read. Lookup order is the output first, then its
`DT_NEEDED` order. A library without a table is not compared and no byte
of it is read beyond what the link reads today.

Identity (`meta.md` MT-13) is (name, version node): a table row's
version is its `.dynsym` entry's `.gnu.version` index (index 1, or no
`.gnu.version` at all, is **unversioned**); a row's version is its
`.dynsym` entry's version node (`spec/dyn.md` D-115; O-60, WP-129): a
mortar-made library's rows carry the node its version script or
`.symver` gives the definition, as any other library's (R-O10). Two rows of
one name are compared when the loader connects them, which glibc's does
as follows (each measured, the rows of the record):

- **Same version node name, or both unversioned:** one entity; the one
  earlier in lookup order provides it to both (`libs1`, `libs2` at node
  `V1`: 1,1).
- **Different node names:** different entities, never compared (R6: one
  node per library, the `libLLVM`/`libclang-cpp` practice, is legal;
  `libv1@LIB1_1`, `libv2@LIB2_1`: 1,2).
- **Unversioned against versioned:** an unversioned definition earlier
  in lookup order binds every versioned reference to its name, default
  (`f@@V2`) and non-default (`f@V1`) alike. Measured with the
  unversioned definition in a library with `.gnu.version` and no version
  definitions (index 1), in one built `-nostdlib` with no `.gnu.version`,
  and in one with version definitions where the symbol has index 1 (the
  base node): the three behave the same (1,1,1). With the versioned
  library earlier, an unversioned reference binds that library's
  definition at version index 2, whether default (`libv2@@LIB2_1`: 2,2)
  or not (`f@V1` when `f@@V2` is index 3: 21, not 22).

Which comparisons that makes:

- **(a) output against library.** The output's definition of a name it
  **exports** (in the output's `.dynsym`, `STV_DEFAULT` or
  `STV_PROTECTED`, not reduced to local by a version script), from a
  live unit with a kind 2 row, is first in lookup order, so every
  in-scope library whose references to the name are not bound locally
  runs it. The keeper's row hash (its first 8 bytes, O-51) is compared
  with each library row of that name that is unversioned, or that is
  the library's default version (`@@`, the version its own references
  name); a difference is `odr-violation` naming the library as the
  other place. A row flagged `NONPREEMPTIBLE` is compared like any
  other: the flag stops only the library's own references from binding
  the output's copy, and the output and every other module still run
  the output's (re-review R4/R5). A name the output does not export at
  all (hidden, version-script `local:`) is not compared: no other module
  can bind it (shape "exe hidden": 2,1, none).
- **(b) library against library.** Two in-scope libraries whose tables
  carry rows of one name that the output does **not export** (export as
  (a) defines it; a hidden copy in the output binds nothing, so the
  libraries still bind each other: "exe hidden, liba, libb": 2,1,1).
  When the output does export the name, (a) against each library is the
  whole report: the output's copy is first in lookup order. Of the
  library earlier in lookup order (E) and the later one (L):
  - rows of one entity (same node name, or both unversioned): compared;
  - E unversioned, L versioned: E's row against L's default row (L's
    own references name that version and bind E's copy);
  - E versioned, L unversioned: L's row against E's row at version
    index 2 (L's references bind it); when E has no row of the name at
    index 2, not compared (see the exclusions).

  A `NONPREEMPTIBLE` row does not remove a pair: its module runs its own
  copy while the other runs another, which is the violation either way
  (R4/R5). A difference is `odr-violation` naming both.
- **(c) sizes.** A variable the output references and an in-scope
  library defines, with a kind 5 row of the output carrying a size:
  compared with the library's `st_size` (`odr-size-mismatch`): K18b's
  copy relocation would truncate (linked against the old size, glibc
  prints ``Symbol `arr' has different size in shared object`` and the
  program reads `0`).

The diagnostic's note says which module runs which copy, from the
lookup order, the rows' `NONPREEMPTIBLE` flags (O-51) and the binding
rows above: "`liba` is earlier in lookup order: it provides the
definition to `libb`" and, for a `NONPREEMPTIBLE` row, "`libsym` runs
its own copy". Measured: `-Bsymbolic` and protected libraries run their
own copy in both orders and the other library binds theirs when they
come first (1,1 then 1,2); `-Bsymbolic-functions` the same (1,1).

**Not compared** (the outcome depends on what the link cannot see; no
false positive is the rule): a library's non-default version (`f@V1`
beside the default `f@@V2`), except as the index-2 row that a later
unversioned library binds in (b): only modules that name that version
bind it, and they are outside the scope (a module linked against the old `f@V1` ran the
unversioned provider's copy when it came first, 1, and `libnv`'s `f@V1`
otherwise, 21); an unversioned reference against a versioned library
earlier in lookup order whose name has no definition at index 2 (glibc
then takes a non-hidden version only when there is exactly one; not
measured); and everything only load time sees (a different build of
the library at run time, `dlopen`ed libraries, libraries the link never
named, indirect dependencies: shape "closure", where `libd` comes in by
`libe`'s `DT_NEEDED`, is X12).

`SYSTEM` rows follow O-32. Cost: a merge of sorted sequences (the tables
and the output's rows are sorted by key); measured (WP-075, release
build, generated tables): about 150 ns of CPU and 65 bytes of peak RSS
per library row read (two tables of 15,000 rows: +4.6 ms, +1.9 MB), and
about 200 ns per row written by O-50; zero unless `--odr=strict` or
`--odr-table` asks for it.
Status: like kind 2 (`--odr=strict` until its corpus, O-47). T-O-24's
shapes: (a) an executable exporting a vague definition that a
mortar-made `.so` defines differently, and the same unexported
(hidden): one finding, then none; (b) two `.so` inputs, differing, both
orders; a `.so` linked `-Bsymbolic`, `-Bsymbolic-functions`, and one
with a `protected` definition, each against a default-visibility
library (R4/R5: findings); a differing library that `--as-needed` drops
(no finding); two libraries with one version node each (R6: no finding)
and with one shared node (a finding); unversioned against versioned in
both orders; (c) K18b.

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

These read only what resolution already holds: the defined global
symbols of every live COMDAT copy, a group with a single copy included
(W2 and W3 compare it with the key's resolved definition, which is
outside the group), and for W2 and W3 each such key's resolved
definition. A definition that is neither in a COMDAT copy nor the
resolved one is not compared (X20). Measured from the **rule
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
| W4 | data contents differ between copies: the bytes, and the relocations compared by W4's rule below (a global or weak target by name and addend; a unit-local target by what it refers to, never by its place in the unit) | K5, K6, K9; a string or merged entry a copy refers to that differs (`"true"` vs `"True"`) | **1**: a libstdc++ vtable (D1/D2 aliases differ between GCC and Clang); the first rule's (names and addends of local targets) 425 on WP-146's 72 equal-source unit pairs, 0 under this rule | `--odr=strict` only, warning `odr-contents-mismatch` |
| W5 | section alignment differs | — | **4**: `_ZTS*` 8 (GCC) vs 1 (Clang) | not a check |
| F | function code bytes differ | K1, K2, K11 … | everything at different `-O` (N1) | never |

**W4's comparison (normative; WP-146).** W4 compares the data
definitions of one key that cover their section (`st_value` 0,
`st_size` the section's size) in the copies of its COMDAT groups. Two
copies are the same when all of the following hold, and a copy is put in
the first class (copies in the order of file, then symbol index) whose
first copy it is the same as; a finding is two classes or more. A copy
whose section, relocation section or a target's section cannot be read,
or one of whose `SHT_REL` relocations has a field width the table below
does not give, takes no part (no finding from it).

1. *Bytes.* The section's bytes are equal (for `SHT_NOBITS`, its size),
   except at the field of every relocation of an `SHT_REL` section whose
   target is unit-local (2): those bytes hold the implicit addend, an
   address in the unit, and are left out of both sides. The field's
   width is that of the relocation type: `EM_386` `R_386_32`,
   `R_386_PC32`, `R_386_GOT32`, `R_386_PLT32`, `R_386_GOTOFF`,
   `R_386_GOTPC`, `R_386_GOT32X` and the `R_386_TLS_*` types 4 bytes,
   `R_386_16` and `R_386_PC16` 2, `R_386_8` and `R_386_PC8` 1. A global
   or weak target's field stays in the bytes (its implicit addend is
   compared there).
2. *Relocations.* Both sides have the same number of relocations; sorted
   by (`r_offset`, `r_type`), ties in the relocation section's order, the
   n-th of each side has the same `r_offset` and `r_type`, and targets
   that agree. A relocation's **target** is a *key* when its symbol's
   binding is not `STB_LOCAL` (`STB_GLOBAL`, `STB_WEAK`,
   `STB_GNU_UNIQUE`; encoded by its symbol-table name and the addend);
   otherwise (`STB_LOCAL`, `STT_SECTION` included) it is
   *unit-local*, encoded by what it refers to:
   - **a string**: the type is absolute (its value is S + A:
     `R_AARCH64_ABS64`, `_ABS32`, `_ABS16`; `R_X86_64_64`, `_32`,
     `_32S`, `_16`, `_8`; `R_386_32`, `_16`, `_8`), the symbol's
     section has `SHF_MERGE` and `SHF_STRINGS` with `sh_entsize` E > 0,
     and p = `st_value` + addend (for `SHT_REL`, the addend is the
     field's bytes, little-endian, sign-extended) is a multiple of E
     inside the section, and a terminator (E zero bytes at a multiple of
     E) lies at or after p in the section: the target is the bytes from
     p up to that terminator (not included) and E. So `"xtrue"+1` and
     `"true"` refer to the same string; `"true"` and `"true"+1` do not.
   - **an entry**: the type is absolute, the section has `SHF_MERGE`
     without `SHF_STRINGS`, E > 0, and p lies inside the section: the
     target is the E bytes of the entry that holds p (from p − p mod E),
     p mod E, and E.
   - **anything else unit-local** (a local object or function, a section
     symbol and addend in a section that is not mergeable, a target the
     two forms above refuse, a type that is not absolute): only its
     *kind*: code (the symbol's section has `SHF_EXECINSTR`) or data
     (any other section, or none).
   Two targets agree when: both are keys with equal names and addends;
   both are strings with equal bytes and E, or both entries with equal
   bytes, offset and E; or one of them is an "anything else" target, or
   the two are a string and an entry, and their kinds are equal; or one
   is a key and the other unit-local (the conservative path, X21: a key
   and a local alias of a function, GCC's `.localalias` or Clang's
   `$local`, are the same target).
3. The note of a finding says where the first two classes' first copies
   first differ: "its bytes", "its relocations", or "the data its
   relocation at offset N refers to" (N in decimal, the first relocation
   whose targets are both strings or both entries and do not agree);
   when the bytes differ too, "its bytes and relocations" or "its bytes
   and the data its relocation at offset N refers to".
4. *`-std`* (WP-151, WP-134 §9). When the record checks read the
   containers (`--odr=strict`), and the first two classes' first copies
   are in files whose units' `std` rows agree within each file (units
   without one left out) and the two files' differ,
   a second note says so: "the copies' translation units differ in -std
   (std=A and B): the initialization of a static local, constant in one
   and dynamic in the other, may differ legally (spec/odr.md §8.1)". The
   finding stays a warning; a file without a `std` row, or whose units
   disagree on `std`, adds no note. As O-32 for rows, the note names the
   legal case and does not withdraw the finding
   (`odr_records.rs::w4_notes_a_std_difference`).

Why by content: the address of a unit-local target is its place in the
unit, which differs whenever the units' other contents do. Measured
(WP-146, a header with eleven inline and template static data members
in two units whose other strings and objects differ, GCC 14, Clang 19
and 22, `-O0`/`-O2`, `-fPIC`/`-fno-PIC`, AArch64, x86-64 and i386, 36
same-compiler pairs, 24 GCC-vs-Clang pairs and 12 `-fPIC`-vs-not
pairs): Clang refers to every unit-local target by its section symbol
and addend (`.L.str` and `.L` labels never reach the symbol table);
GCC does too at `-O0` and for strings at `-O2`, and refers to a local
object by its own name (`_ZL3arr`, in `.rodata._ZL3arr`) at `-O2`. GCC
`-O0` puts string literals in a plain `.rodata` (no `SHF_MERGE`), GCC
`-O2` in `.rodata.str1.8` on AArch64 and `.rodata.str1.1` on x86, Clang
in `.rodata.str1.1`; a table of pointers is in `.rodata` without
`-fPIC` and in `.data.rel.ro`/`.data.rel.ro.local` with it. So the kind
is code or data only: whether the section is writable or mergeable
differs between legal units of one source. The first rule's encoding
gave 425 findings on those 72 equal pairs, this one 0.

Cost: one pass over the copies resolution keeps, comparing two integers
per symbol [estimate: under 1 ms per 100,000 copies; the Python
prototype read protoc's 168 inputs in about 2 s, dominated by Python's
parsing].

### 8.2 Declarations (kind 5)

**O-35 (kind 5 row) [kiln].** One row per external-linkage function or
variable the TU defines or references (an undefined symbol), excluding
unprototyped C declarations and the entities listed under "No row"
below:

| offset | field | |
|---|---|---|
| 0 | `u64 key` | MT-13 |
| 8 | `u64 type` | FNV-1a 64 of the declared type's `<type>` encoding (the string `type_name` names, whether or not it is written), with the **major** array bound of a variable's own type erased (`A_`; inner bounds and the arrays a pointer points to are kept: N15 needs no more), in C and C++ units alike |
| 16 | `u64 size` | `sizeof` of a variable's declared type; 0 for a function or an array of unknown bound |
| 24 | `u32 name` | 0: anchored |
| 28 | `u32 type_name` | the `<type>` encoding as a string (deduplicated), for the message; 0 (MT-12's empty string: not written) for a C++-linkage **function** row below the level `detail` of `-fmason-metadata` (`meta.md` MT-41: its mangled name already carries the parameter types); variables and every C-linkage row carry it at `default` |
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
unit) follow O-33. Rows of one key whose `EXTERN_C` bits differ (a
C++-linkage variable of the global namespace against a C or `extern "C"`
one: one unmangled name, two encodings) are compared by `size` only,
never by `type`, and the difference of language linkage is not reported
([dcl.link]'s rule is not diagnosed: the Itanium ABI gives both the same
symbol). A finding whose rows carry no `type_name` names the two objects
and says that `-fmason-metadata=detail` records the types (both
sentences implemented by WP-134: `crates/mortar_odr/src/records.rs`
`kind5`; each language linkage's rows are compared among themselves
first, and the rows of both by size only if neither of those reports).
Status: `--odr=strict` until its corpus
(O-47).

**No row** (kiln K-5, kiln commit 9744b3649; rulings of 2026-11-17):
- an entity with **C++ language linkage whose symbol name comes from an
  asm label**: its name is not a function of its declaration, and
  several C++ types share the symbol by design (glibc's `extern "C++"`
  overloads `strchr`, `strrchr`, `strpbrk`, `strstr`, `strcasestr`,
  `strchrnul`, `memchr`, `memrchr`, `rawmemchr`, `basename`,
  `at_quick_exit`: `char *(char *, int)` and `const char *(const char *,
  int)` against C's `char *(const char *, int)`). C-linkage asm-labelled
  declarations (glibc's `__REDIRECT` inside `extern "C"`, C units) keep
  their rows, keyed by the label; a unit that declares one symbol both
  ways (a C-linkage declaration and a C++-linkage asm-labelled one)
  writes no row for it (kiln's choice, accepted: a miss);
- a unit holds **at most one kind 5 row per key**: if its declarations
  of one symbol disagree, it writes none for that key;
- constructors and destructors (the name carries the type), aliases,
  IFUNCs, `weakref`s; functions with a non-default calling convention,
  `regparm`, SME attributes, or a nested unprototyped function type;
  library builtins the source does not declare (a `memcpy` the compiler
  introduces for a struct copy; `abort` or `strlen` declared by the
  source get rows).

Further readings: an `available_externally` use counts as a reference
(if the object ends with no symbol of that name the row is unanchored
and not compared, O-4); a variable of incomplete class type has `size`
0; cv- and ref-qualified member function types are written in Itanium
form (`KF…E`, `F…RE`, `F…OE`); every row of a C unit has `EXTERN_C`;
`NOEXCEPT` is set only on C++-linkage functions.

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
  compatible type it chose, K-5), and **the signedness of `int` is
  erased**: `unsigned int` and `int` are one token (`i`) at every level,
  so an enumeration whose compatible type is either is `i` (an
  enumeration of another width, under `-fshort-enums`, with a C23 fixed
  underlying type or with 64-bit values, keeps the compatible type kiln
  chose). The reason: an enum must equal `unsigned int` (N28, GCC's
  documented choice) AND `int` (glibc declares `getitimer`'s and
  `setitimer`'s first parameter `enum __itimer_which` in GNU C and `int`
  in C++), and a hash has only equality; the finding lost (`int` against
  `unsigned int`) is harmless at the ABI of all three targets. Top-level
  qualifiers of parameters are
  ignored, an unprototyped declaration is never compared (before C23,
  `f()` is unprototyped; from C23 it means `f(void)` and is compared,
  decided by the unit's `std`). The C++ side of an `extern "C"` row is
  canonicalized to the same C type, with C++'s builtin types mapped to
  their C equivalents for the target: `wchar_t` to the target's
  `wchar_t` typedef (`unsigned int` on AArch64 Linux, `int` on x86-64 and
  i386 Linux), `char16_t` to `uint_least16_t` (`unsigned short`),
  `char32_t` to `uint_least32_t` (`unsigned int`), `char8_t` to `unsigned
  char` (C23's `char8_t`), and `bool` and `_Bool` to one type. Mortar compares
  the `type` hash of the canonical C type kiln writes for C-linkage rows
  (K-5), so the rules live in kiln's canonicalization and mortar compares
  bytes;
- the canonical C type is **conservative** (peer review of kiln,
  2026-11-15: wherever the C and the C++ spelling of one declaration can
  legitimately differ, the canonical form erases the difference; X19):
  typedefs are resolved; **every record type**, behind a pointer or by
  value, is one opaque token `R` (`typedef struct {…} T;` has linkage
  name `T` in C++ and none in C, and a namespace-qualified C++ struct is
  a C tag; size differences of by-value records stay visible through
  `size` for variables); `va_list` and its target spellings
  (`std::__va_list` on AArch64 C++, `__va_list_tag[1]`, `char*`) are one
  token `V`; `noexcept` is dropped at **every** level, including nested
  function-pointer types; references become pointers; `restrict` is
  dropped (other pointer-level qualifiers, `const` and `volatile`, are
  kept); a GCC transparent union parameter becomes its first member's
  type (glibc's `__SOCKADDR_ARG`, measured below); `(void)` and `()` in a
  prototyped declaration are one form; **an array bound is erased
  wherever C lets it be omitted**: on every array type that is not the
  element type of another array (a variable's own major bound, O-35,
  and the outer bound of an array reached through a pointer, a typedef
  or a nested function type), because `int (*)[]` and `int (*)[4]` are
  compatible in C and a hash has only equality; inner dimensions are
  kept, and `[3]` against `[4]` behind a pointer is a miss (X19; ruling
  of 2026-11-17 on kiln's reading). A C-linkage declaration whose type
  contains a **variable** array bound anywhere (`[n]`, `[*]`, at any
  depth, in a parameter or a nested function-pointer type) gets **no
  row**: `void f(int n, int b[][n][n])` and `void f(int n, int
  b[][4][4])` are compatible in C, and erasing bounds in the unit that
  has the variable one would still leave the other unit's `PA4_A4_i`
  to differ from it; a unit without a row is never a party (a miss,
  X19; ruling of 2026-11-17, kiln's K-4b question). *Measured by kiln with its own
  types* (K-5, AArch64, 24 headers, 2,644 functions, before the two
  rulings of 2026-11-17): 13 differences, 11 the asm-labelled
  `extern "C++"` overloads (O-35 "No row") and 2 `getitimer`/`setitimer`
  (the erased signedness above); with both rulings (kiln PR #769): **0**
  canonical types and 0 flags differ of the 2,633 functions with rows in
  both units. The proxy measurement that preceded it missed
  both: *measured* [proxy: clang's types,
  not kiln's; T-O-33]: glibc's public declarations (24 headers,
  `-D_GNU_SOURCE`) compiled as C17 and as C++20 `extern "C"`, clang 19
  and 22, AArch64, x86-64 and i386: 2,573 / 2,461 / 2,461 functions per
  target, raw type strings differ for 2,318-2,329 of them, canonical
  strings differ for **0** (8, all socket functions, without the
  transparent-union step). i386: GCC's C `wchar_t` is `long int`, clang's
  `int` (both mangle `w`); kiln writes records for the 64-bit targets
  only, so this matters only if i386 ever gets records, and then `int`
  and `long` must be one token there. zlib, SQLite and Lua (named as the
  C corpus below) are not in the container and were not used;
- C-linkage findings are `odr-declaration-mismatch` under
  `--odr=strict` **only, until a C corpus** shows zero: C libraries
  compiled by kiln with records (objects without records exercise
  nothing here, so glibc's `libc.a` is not a corpus), with their test
  programs and a C++ client of each through `extern "C"`. Candidates:
  zlib, SQLite (amalgamation) and Lua; WP-074 fixes the selection. Model `C-enum-compatible-type` shows today's Itanium-type
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
| `odr-contents-mismatch` | `OdrContentsMismatch { name }` | `{name} has different contents in different files` | warning (strict only) |
| `odr-declaration-mismatch` | `OdrDeclarationMismatch { name }` | `{name} is declared with a type that does not match its definition` | warning; error under `--odr=error`/`strict` |
| `odr-internal-reference` | `OdrInternalReference { name }` | `{name} refers to an entity with internal linkage, which differs in each translation unit` | warning (strict only) |
| `odr-isa-mismatch` | `OdrIsaMismatch { name }` | `the kept copy of {name} was compiled for other target features than a translation unit that uses it` | warning |
| `odr-size-mismatch` | `OdrSizeMismatch { name }` | `{name} has different sizes in different files` | warning |
| `odr-strong-and-comdat` | `OdrStrongAndComdat { name }` | `{name} has a non-inline definition and an inline copy` | warning (strict only) |
| `odr-symbol-type-mismatch` | `OdrSymbolTypeMismatch { name }` | `{name} is a function in one file and data in another` | warning |
| `odr-type-mismatch` | `OdrTypeMismatch { name }` | `type {name} violates the one-definition rule` | warning; layout differences are errors under `--odr=error`/`strict` |
| `odr-violation` | `OdrViolation { name }` | `{name} violates the one-definition rule` | warning; error under `--odr=error`/`strict` |
| `odr-vtable-rtti-mismatch` | `OdrVtableRttiMismatch { name }` | `the vtable of {name} is emitted by translation units compiled with and without RTTI` | warning |
| `meta-malformed` | `MetaMalformed { reason }` | `malformed Mason metadata: {reason}` | a container's contents: error, `strict` only (O-42, O-47); a section of the name that is not a container (MT-6): warning in every mode |
| `meta-version` | `MetaVersion { major }` | `Mason metadata version {major} is not supported; its records are ignored` | error, `strict` only (O-42, O-47) |

Every `{name}` goes through the demangling hook (`spec/diag.md` §5.4).
Context lines (`spec/diag.md` §2.4), e.g. for K1 linked `a.o b.o`:

```
warning: f(int) violates the one-definition rule
>>> defined in a.o (kept), at h.h:3
>>> defined differently in b.o, at h.h:3
>>> the definitions differ; a.o and b.o were compiled with different -D options? (compare their command lines)
```

- Places are `spec/diag.md` §2.4's `file:(symbol)`. A kind 5 finding's
  place says what each row is (WP-151): a row with `DEFINITION`
  "defined in", without it "declared in" (as the first class) or
  "declared differently in" (`>>> defined in a.o:(_Z1xv)`,
  `>>> declared differently in b.o:(_Z1xv)`). The W checks
  (no record, no source position) print `>>> defined in a.o:(f)` and
  `>>> defined differently in b.o:(f)`, and say in a note which class is
  kept (`the first is kept`) or used; the `(kept)` marker and `at h.h:3`
  of the example above are the record checks' (WP-073), once a place can
  carry a source position.
- One diagnostic per entity: the copies are partitioned by hash; the
  first class (the keeper's) is "defined in", each other class's first
  file "defined differently in", with at most 3 files a class and a count
  of the rest.
- A note naming the two units, where they carry them: kind 1's `source`
  and `producer` (`>>> a.o: source=a.cc, producer=kiln 0.3; b.o:
  source=b.cc, producer=gcc`), first among the notes (WP-073: a
  developer needs "which TU, built by what").
- Notes that explain, from kind 1: every key other than `source` and
  `producer` that differs between the two units (`>>> a.o: abi.short-enums=on; b.o: off`, `>>> a.o: std=c++17;
  b.o: c++20`). The question in the K1 example is printed only when no
  kind 1 key differs. A kind 5 finding between rows of C and C++
  language linkage (O-35, compared by size only) notes only the keys
  that both units carry with `ENFORCE` (O-30), not `lang`, `std` or any
  other key O-32 does not judge between a C and a C++ unit (WP-151;
  WP-134 §9).
- Types: with kind 4, the first difference (`>>> member 'y' at offset 8
  in a.o only`); without, the sizes and alignments. Exactly (WP-145):
  when `odr-type-mismatch` compared layouts within one scheme (O-17,
  O-18) and found two classes, the first rows of the first two classes
  (A of file `a.o`, B of `b.o`, as the context lines name them) both
  have `details`, and both entries read (O-19), the note that would give
  the sizes and alignments (`a.o: size 8, alignment 4; b.o: size 12,
  alignment 4`, or `the layouts differ (members, offsets, bases or
  vtable) with the same size 8 and alignment 4`) is replaced by the
  first difference below; when the shape flags differ at equal sizes,
  their note (`a.o: a class; b.o: a union`) stays and the first difference
  follows it. With one row's `details` only, neither, an entry of a
  block version above 1 (MT-25), or a definition-hash or cross-scheme
  finding, the notes are as before, byte for byte. Kind 4 never decides
  a finding and is not read when kind 3 found none.

  *An item as a note writes it* (`D`): a member `member 'y' (i) at
  offset 8`, a bit-field `member 'y' (i) at offset 70 bits, bit-field of
  3 bits` (a member without a name: `unnamed member (i) ...`; without a
  type string the `(i)` is left out); a base `base 1B at offset 0`, a
  virtual one `base 1B at offset 0 (virtual)`; `vptr at offset 0`;
  `vtable slot 2: _ZN1S1gEv`; `enumerator 'B' = 1`, in signed decimal
  (`-1`) when the entry's underlying type item names a signed type
  (`a`, `s`, `i`, `l`, `x`, `n`; `c` when kind 1's `abi.char` is
  `signed`), in unsigned decimal when it names an unsigned one (`h`,
  `t`, `j`, `m`, `y`, `o`, `b`, `Ds`, `Di`, `Du`; `c` with `abi.char`
  `unsigned`), and otherwise (no item, `w`, another type) as before: a
  value with bit 63 set in hex, `0xffffffffffffffff` (WP-151); `meta
  dump` and the test producer print values the same way; `underlying
  type j`. An item nested in unnamed class-type members (O-19's `width`)
  has ` of the unnamed member at offset N` appended for each enclosing
  one, innermost first, and an unnamed member with a count reads
  `unnamed member at offset 0 with 2 nested items` (WP-151). Offsets: O-19's bits divided by 8,
  without a unit, when the item is not a bit-field and the bit offset is
  a multiple of 8; otherwise the bits, followed by `bits`. Type strings
  and names as the unit writes them (no demangling). `D'` is `D` without
  its leading kind words (`member `, `base `, `vptr `, `vtable slot `,
  `enumerator `, `underlying type `).

  *The first difference.* Items of kinds O-19 does not define are left
  out. Two items are equal when their kinds, name strings, type strings
  (by content), `value`, `width` and flag bits 0-5 are. An item's
  *identity* is its kind and name (member, vtable slot, enumerator), its
  kind and type (base), its kind (vptr, underlying type). Let `i` be the
  first position where A's and B's items differ:

  | case | note |
  |---|---|
  | none (same items, same count) | `the layout details are equal: the difference is in something kind 4 does not describe`; for two rows flagged `ENUM`, `the layout details are equal: the enumerations differ in something kind 4 does not describe (whether the underlying type is fixed, or whether the enumeration is scoped)` (WP-151) |
  | B ends at `i` (B is a prefix of A) | `D(A[i]) in a.o only`, then `, and N more items` when A has N > 0 items after it (`, and 1 more item`) |
  | A ends at `i` | `D(B[i]) in b.o only`, the same way |
  | A[i], B[i] of one identity | `D(A[i]) in a.o; D'(B[i]) in b.o` |
  | A[i]'s identity not in B, B[i]'s in A | `D(A[i]) in a.o only` |
  | B[i]'s identity not in A, A[i]'s in B | `D(B[i]) in b.o only` |
  | neither identity in the other | `D(A[i]) in a.o; D(B[i]) in b.o` |
  | both in the other (same items, different order) | `the items stand in a different order: D(A[i]) in a.o; D(B[i]) in b.o` |

  E.g. `member 'extra' (i) at offset 4 in a.o only`; `member 'y' (i) at
  offset 70 bits, bit-field of 3 bits in a.o; 'y' (l) at offset 8 in
  b.o`; `vtable slot 1: _ZN1S1gEv in b.o only`. When the item at `i`
  is a vtable slot, a second note follows (WP-151): `vtable slots are
  numbered over the whole vtable group from 0, its offset-to-top, vcall
  and vbase entries counted and its RTTI entries not (spec/odr.md O-18,
  O-19)`.

  Without both entries, for two rows flagged `ENUM` of equal size and
  alignment, the note that would say `the layouts differ (members,
  offsets, bases or vtable) with the same size 4 and alignment 4` says
  `the enumerators or the underlying type differ, with the same size 4
  and alignment 4` (WP-151).

  An entry that does not read (O-19's malformations) gives
  `meta-malformed` (an error, `strict` only, as MT-14's) naming the file,
  `.mason.meta` and the unit's offset, with the reason `unit at offset
  U: kind 4 block at offset K: entry at offset E: ...` (or `a kind 3 row
  names the kind 4 entry at offset E, and the unit has no kind 4 block`),
  once per (file, unit, entry); the type's note is then the sizes and
  alignments.
- Key (`spec/model.md` §8): phase 40, unit = the `FileId` of the first
  file of the second class, seq = its symbol-table index; for a row
  without a symbol (types, unanchored rows) `2^40 + ` the row's index in
  its unit (amendment A-2); for a COMDAT group's finding (O-25, O-31)
  `2^41 +` the group's index; for a container finding `2^42 +` its index
  in the file. The record checks add a tag `t << 44` to seq, so that two
  checks never share a key: 1 `ENFORCE` (O-30), 2 kind 5, 3 kind 3, 4
  kind 2, 5 `odr-internal-reference`, 6 `isa` (O-31), 7 O-25, 8 the
  container findings; the W checks have tag 0 (WP-073).

The load-time hook's prototype messages (§10.3) would be the runtime's,
not mortar diagnostics: `odr-violation-at-load`,
`odr-hook-out-of-memory`, `odr-hook-stopped`.

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
  included, plus `PARTIAL` and `INLINED_ONLY` rows. Of the unanchored
  rows, strict compares the kind 2 rows flagged `INLINED_ONLY` (their
  entity has no symbol by construction) and the named kind 2 rows whose
  file defines no symbol of that name (O-15); other unanchored rows are
  counted, not compared (§3 N12, N13). The record-based findings above
  are errors, every other finding a warning; the container findings
  `meta-malformed` and `meta-version` (MT-14, MT-35) are errors, and
  strict is the only mode that reports them, since no other mode reads
  a container (O-42).
- The W checks (§8.1) are warnings in every mode, `error` and `strict`
  included (O-40); `--fatal-warnings` makes them errors.
- `--odr=off`, also spelled **`--no-odr-check`**: no check runs, no
  container byte is read (O-42), the W checks do not run, and no
  loadable table is written (§10). The output is byte-identical to the
  default link's when that link succeeds and the table is off (T-O-41).
- `--odr-check-archives` (O-23), `--odr-table=off|exported|full` (O-50),
  `--odr-stats` (O-44). With `--odr=off` the first two do nothing (no
  check runs, no table is written); in `warn`, `error` and `strict` each
  does the same thing, except that the record checks O-24 and O-32 make
  strict-only decisions inside them (WP-076). `--keep-mason-meta`
  (`meta.md` MT-32) is not an ODR switch: it keeps the containers in
  every mode, `--odr=off` included.
- GNU gold's `--detect-odr-violations`: accepted as a **no-op**: mortar
  already checks by default. It prints nothing (`spec/diag.md` §2.1 has
  no note severity); the option table's reason for the row is `mortar
  checks the one-definition rule by default; --detect-odr-violations has
  no further effect`. It must not lower an error to a warning (review
  M11).

**O-42 (a link that does not use the feature does not pay for it).** A
hard rule. A container is read only when a check that needs it is on:
with `--odr=off`, and in the default mode while O-47 lists no
record-based check as on, mortar makes the reads it would make with no
container, keeps no container byte resident and parses no unit
(`meta.md` MT-42, amendment A-8 to `spec/input.md` §11.1), hashes no
name (O-4) and writes no table. The W checks read only what resolution
holds already. T-O-42 counts bytes read. The opt-ins that need
containers read them only when asked: `--odr-check-archives` (O-23),
`--odr-table` (O-50) and `--keep-mason-meta` (MT-32, in every mode);
each one's read counter is tested off (WP-076).

**O-44 (`--odr-stats`).** One line to standard error: units read, rows by
kind, schemes seen and the unit pairs not compared because of them
(`schemes N, pairs not compared M`, of `odr.scheme`; then `layout
schemes N, layout pairs not compared M`, of the layout schemes, WP-151),
the kind 3 groups compared across schemes (O-11's exception), ambiguous
anchors (O-4), unanchored rows, `SYSTEM` differences between
units of different `-std` (O-32), time spent and container bytes read,
and the kind 4 entries read for notes (`layout details read N`, before
`record findings`; WP-145: non-zero only on a kind 3 layout finding).
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
decision (Q1), after it has been on at zero for a release. Checks move
independently, per kind, in the order kiln lands their records (§11):
kinds 5 and 3 may move before kind 2. Each move is
recorded in the landed record of the package that makes it and in
O-47's table, by amendment.

**O-47 (the register).** Which checks are on by default, today:

| check | needs | status | evidence | moves when |
|---|---|---|---|---|
| W1 `odr-size-mismatch` | foreign facts | **on** (warning) | experiment 1: zero on every corpus of §8.1 | — |
| W2 `odr-symbol-type-mismatch` | foreign facts | **on** (warning) | experiment 1: zero; N23 excluded by its wording | — |
| `meta-malformed`, `meta-version` (a container's contents, MT-14, MT-35) | container headers | strict (error); not reported in other modes, which read no container (O-42) | no object of the corpora has a container; they fire only on broken producers | with the first record check that moves to on |
| W3 `odr-strong-and-comdat` | foreign facts | strict | N20-N22 | never by default (Q12 withdrawn) |
| W4 | foreign facts | strict | N3's vtable | never by default |
| kind 5 C++ linkage `odr-declaration-mismatch` | kiln K-1, K-5 (first in kiln's order) | strict | cannot run yet | WP-074's oracle at zero |
| kind 5 C linkage | K-5 | strict | N28 | the C corpus of O-33 at zero |
| kind 3 `odr-type-mismatch` (layout) | K-4 (second) | strict | cannot run yet | WP-074's oracle at zero |
| kind 3 definition-only differences (X22) | K-6's serializer (O-17) | strict | — | after layout's and after kind 2's, separately |
| kind 3 size and alignment across schemes (O-11) | K-4 | strict | WP-073's tests only | never by default while O-11's exception stands (O-11) |
| kind 2 `odr-violation` (hash, K13 flags) | K-3 (third) | strict | cannot run yet | WP-074's oracle at zero |
| `odr-internal-reference` | K-3 | strict | `k12proxy`: libstdc++ sets it | Q5 |
| `odr-abi-mismatch` (`ENFORCE` keys), `odr-isa-mismatch` | K-2 | strict | cannot run yet | WP-074's oracle at zero |
| `odr-vtable-rtti-mismatch` | K-2 | strict | model only | WP-074's oracle at zero |
| O-24 (shared-object inputs) | K-3 and libraries with tables | strict | prototyped and implemented (WP-075): (a), (b), (c) | with kind 2 |
| `.note.mason` table | K-3 | off (`--odr-table`) | `note-final` | Q2 |
| O-34 kind 5 vs foreign sizes | K-5 | strict | cannot run yet | WP-074's oracle at zero |
| `SYSTEM` `-std` differences (O-32) | K-3 | strict (warnings, O-32); an `--odr-stats` count in any other mode that compares | `std-mix`: 210 in one pair of TUs | Q6 |
| `odr-archive-shadow` | K-3 | opt-in flag | — | never by default |
| load-time hook | a runtime | **none**: a prototype direction (§10.3), never in M6 | the matrix of O-58 not yet green | O-58, then Q2 and Q3 |

Outside the table: `meta-malformed` for a section named `.mason.meta`
that is not a container (`spec/meta.md` MT-6) is resolution's
classification, not a container read; it is a warning in every mode,
`--odr=off` included, and never an error (zero on every corpus of §12.3,
MT-6).

### 9.3 ABI and ISA summaries

**O-30 (`ENFORCE` keys).** Two live units whose kind 1 rows for the same
name both carry `ENFORCE` with different values: `odr-abi-mismatch`. Per
name, the **first value** is the value of the first unit in file order
(the lowest `FileId`, then the unit's place in its file; never the order
of threads); each distinct value other than the first gives one
diagnostic, naming the first unit of that value and the first unit of
the first value. The diagnostics are linear in the values: `n` distinct
values give `n - 1`, never a diagnostic per pair (WP-073).
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

**O-50 (which rows).** With `--odr-table=exported` (not the default,
Q2), a mortar-made executable `[M6]` or shared object `[M4]` carries
one row per definition it **exports**: in `.dynsym`, defined,
`STV_DEFAULT` or `STV_PROTECTED`, not reduced to local by a version
script; and that has a kind 2 row with `VAGUE` or `EXPLICIT_SPEC`.
Protected definitions and definitions bound locally by `-Bsymbolic` or
`-Bsymbolic-functions` **have rows**: they stop only the defining module
from being preempted, and other modules still bind them (re-review
R4/R5: `liby` binds `libx`'s copy in all three variants); their rows
carry `NONPREEMPTIBLE` (O-51). Hidden definitions are never bound by
another module and have no row. With `=full`, also hidden vague
definitions and kind 3 types (strict use only, Q14). With `=off` (the
default), no table. The table is data only: it adds no code to the
output. The runtime of §10.3 is added only by the prototype option of
WP-077, never by `--odr-table`.

Implemented (WP-075) for dynamic executables, PIE or not: `=exported`
writes the table; `=full` writes it and a second note (WP-076, O-52);
a static executable exports nothing and gets no table, with `=full`
too; a shared object's table is O-60's (WP-129). Every row of an executable's table carries `NONPREEMPTIBLE`:
an executable comes first in lookup order and binds its own definitions
at link time, so it always runs its own copy. The keeper's unit is the
file whose definition resolution kept; its first kind 2 row for the
symbol is the row written. Bytes are a function of the rows (sorted by
key, then `.dynsym` index), so the output stays deterministic, and the
verifier's verdict on an output does not change with the table (T-O-50).

`=full` (WP-076): the table above, unchanged, and a second note (O-52)
of two arrays. **Hidden definitions**: one row per live definition that
the table above does not carry (not exported: hidden or internal, or
not in `.dynsym`, which in an executable is most definitions) whose
keeper's kind 2 row has `VAGUE` or `EXPLICIT_SPEC`; its key is the
FNV-1a 64 of the symbol's name, its hash and flags the keeper's row's
(`NONPREEMPTIBLE` clear: the row says nothing about binding). **Types**:
one row per kind 3 group of the live units (key and `<type>` encoding,
O-17), the group's first row in (file, unit, row) order: key, hash,
layout, size, alignment and flags. A link that only warned may have
several layouts for a key; the table carries the first, as the
definitions carry the keeper's. The rows are a function of the live
units: the bytes are deterministic (T-O-50). What O-24 does with them:
**nothing** today. O-24 reads the first note only (`table::find`); the
second is read by `table::find_full` (`xtask meta table`'s reader and
the tests), and a malformed second note is not reported by a link. A
reader that compares types across modules (Q14: strict use only) is
future work (WP-077's runtime, Tuck); the encoding is fixed here so that
it does not change under them.

**O-51 (row).** `{u64 key; u64 hash; u32 dynsym; u16 flags; u16
reserved}` (24 bytes), sorted by key: the key of MT-13; the first 8 bytes
of the kind 2 hash of the **keeper's unit's** row (the copy that runs:
when several units have rows for the key, possibly with different hashes
after a link that only warned, the keeper's is written, never another);
the `.dynsym` index of the symbol (the name comes from
`.dynsym`/`.dynstr`, and its version node from `.gnu.version`, so the
table carries no strings and the identity is (name, version), MT-13);
O-15's flags as a `u16`, so `SYSTEM` (bit 8) reaches readers, plus bit
15 `NONPREEMPTIBLE` (table rows only: the module binds its own copy,
because the definition is protected or bound by `-Bsymbolic`/
`-Bsymbolic-functions`). `-Bsymbolic-functions` leaves no trace in the
`.so` (GNU ld and lld set `DF_SYMBOLIC` for `-Bsymbolic`, nothing for
`-Bsymbolic-functions`; measured), so the row's `NONPREEMPTIBLE` flag is
the only carrier of that fact for a reader of the output. The key is
FNV-1a 64 of the `.dynsym` name (without its version, which
`.gnu.version` carries). Header `{u32 version = 1; u32 count; u32
row_size = 24; u32 reserved}`. A table that does not parse (a note too
short for its header, a version other than 1, a `row_size` other than
24, a `count` the descriptor cannot hold, rows out of key order, a
second `MasonMeta` table note in the same object, a `dynsym` index
outside `.dynsym`) is `meta-malformed` at the library's `.note.mason`,
never a panic (`mortar_meta::table`'s mutation test: every truncation
and every single-byte change of a table is a table or a named error).
Tables are read only under `--odr=strict` (O-24), where a malformed one
stops the link like any other malformed container (MT-14).

`=full`'s note: header `{u32 version = 1; u32 definitions; u32 row_size
= 32; u32 types}`, then `definitions` rows and `types` rows, each array
sorted by its bytes as integers in field order (key first). A row is
`{u64 key; u8 hash[8]; u64 layout; u32 size; u16 align; u16 flags}`
(32 bytes); a definition's layout, size and alignment are zero. It is
malformed for the same causes as the table (too short, version, a
`row_size` other than 32, counts the descriptor cannot hold, an array
out of order, a second note of the type), never a panic (the mutation
test of `mortar_meta::table`).

Size: 24 bytes per exported vague definition. Measured counts of
exported weak or unique definitions in system libraries (`nm -D`, by
hand): `libstdc++.so.6` 3,991, `libclang-cpp.so.19.1` 12,929,
`libLLVM.so.19.1` 15,129: 96 KB, 310 KB and 363 KB of table, in pages
that are read at load and never written.

**O-60 [M4; landed: WP-129, `spec/landed/meta/WP-129.md`] (the table of a shared object).** Request
R-O10 of `spec/dyn.md` §18.2 (D-126 item 1). A `Shared` output's table
is O-50's and O-51's with three statements:

1. **Version.** A row's version is its `.dynsym` entry's version node
   (`spec/dyn.md` D-115: the node a version script or a `.symver`
   definition gives it; index 1, unversioned, otherwise). The row
   carries no version of its own (O-51), so this is a statement about
   `.gnu.version`, which O-24's identity reads.
2. **`NONPREEMPTIBLE`** is set exactly for the definitions `spec/dyn.md`
   D-112 binds inside the library (protected; `-Bsymbolic`;
   `-Bsymbolic-functions` for a function; not listed by
   `--dynamic-list`), which are `Resolution`'s exported keys outside
   `preemptible` (`spec/model.md` §3.3); a preemptible definition's row
   has it clear. Unlike an executable's, a library's rows do not all
   carry it. The bit is truthful because D-117's rule, its boundary
   rule and its analysis refuse a library whose code binds a
   preemptible datum without naming it, with one exception: D-117's
   residual hole (forms the analysis does not follow; 3 of 306 measured
   bound definitions), where a row says preemptible while the library's
   code is bound. A reader must not take a clear bit as proof of
   interposability for a datum defined by an object without a
   `reloc-model` row saying `pic` (H-3, `spec/meta.md` MT-26); with that
   row, it is proof.
3. **Readers.** A consumer of a row (O-24's comparison across
   libraries, Tuck, and the executable link's check of a copy of
   locally bound data, `spec/dyn.md` D-43 as amended by R-D43, WP-122)
   reads `NONPREEMPTIBLE` as that check does: the library runs its own
   copy whatever an earlier module defines.

*Test:* `tests/link/fixtures/shared-o60.fixture` (rows; the bit for
default, protected and `-Bsymbolic-functions` definitions; versions by
a version script), `tests/link/shared/o60-table.test` (the bindings the
bits describe, run), `o60-reader.test` and `o60-reader-symbolic.test`
(statement 3: an executable's O-24 under `--odr=strict` reads the bit
of a mortar-made library), `o60-table-wp128.test` (`-Bsymbolic`,
`--dynamic-list`: skipped for mortar, WP-128). The verifier
judges O-60 as P-M2's (`spec/plumb.md` §13.2, WP-164): every link of
those tests is judged. A `Shared` link also runs
O-24 against its own `DT_NEEDED` libraries, as an executable does,
with the library first in lookup order for its exported names
(implemented by the same check; untested beyond the executable's
shapes).

### 10.2 Section and segment

**O-52 (encoding).** An allocated `SHT_NOTE` section `.note.mason`,
`sh_addralign` 8, holding one note: owner `MasonMeta` (`n_namesz` 10),
`n_type` **`0x4d4f4401`** (`NT_MASON_ODR`; a distinctive value, because
binutils `readelf -n` prints type 1 under an unknown owner as
`NT_VERSION`), descriptor = O-51's header and rows. With `=full` it holds
a second note after the first: the same owner, `n_type`
**`0x4d4f4402`** (`NT_MASON_ODR_FULL`), descriptor = `=full`'s header
and rows (O-51) (WP-076). A reader of the first note never sees the
second (a different type), so `=exported`'s readers read a `=full`
table unchanged. With `--odr-runtime=report|fatal` (O-53a, `spec/cli.md`
C-ODR-RT; WP-198) it holds a third note, last: the same owner,
`n_type` **`0x4d4f4403`** (`NT_MASON_ODR_RT`), `n_descsz` 16, the
descriptor four little-endian `u32`: `{version 1, mode, flags 0,
runtime id}`, mode 1 `report` or 2 `fatal` (the module's default, which
`MASON_ODR` overrides at load time; a library's entry takes the main
program's note first, then its own), the runtime id the index of the
embedded runtime object mortar linked (its table, with each object's
SHA-256, is `crates/mortar/src/odr_runtime.rs`: 0 AArch64 `so`, 1
AArch64 `exe`, 2 x86-64 `so`, 3 x86-64 `exe`, 4 and 5 i386, embedded
and refused). The note is 40 bytes (24 of header and owner, 16 of
descriptor), a multiple of 8, so the notes before and after it keep
their alignment. The runtime finds it by its type in any `PT_NOTE` of
the module (`dl_iterate_phdr`), whatever notes come before it; a reader
of the first or the second note never sees it. It lies in a
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

Measured again on mortar's own output (WP-075; aarch64 natively, x86-64
under qemu; binutils 2.44, elfutils 0.192, gdb 16.3, lldb 19):
`.note.mason` gets a `PT_NOTE` of its own (alignment 8) after the
4-aligned one; `readelf -n` and `eu-readelf -n` print the note as
above; `strip` (default, `--strip-debug`, `--strip-unneeded`,
`--strip-all`) and `eu-strip` keep the descriptor byte for byte;
`objcopy --remove-section=.note.mason` removes the section and the note
(`readelf -n` no longer finds it), and the stripped and the
note-removed programs run; `eu-elflint --gnu-ld` makes the one
complaint above twice (the segment and the section), and nothing else:
the same output without the table passes with no allowance (`dyn.md`
D-71); gdb (aarch64) and lldb stop at `main` and print the backtrace
(`tests/link/dyn/odr/o50-table.test`).

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

### 10.3 The load-time hook (a design direction, not a rule)

Nothing in this section is a rule an implementation follows. It records
the direction a prototype package (WP-077) explores, what two
prototypes measured, the open problem that stops it, and the matrix the
prototype must pass before any rule is written. No mortar output gets a
runtime in M6 without a further decision (O-58, Q2).

**O-53 (the runtime, if one ships).** A small object, the runtime, that
mortar would add as an ordinary input of an output that asks for it. It
is C (prototype 2, `hook2/mason_rt2.c`, is 230 lines), and mortar stays
`forbid(unsafe_code)`: the runtime is bytes it links, not code it runs.
How its bytes would be produced and verified is the operator's Q3 (three
options with costs; recommended: the C source and the generated object
bytes per target checked in, embedded in mortar, a byte-identical
rebuild check in CI). In no option does building mortar need a cross
compiler.

**O-54 (the direction, and what the prototypes measured).**

*What holds* (prototype 2, glibc 2.41, AArch64 natively, x86-64 and
i386 under qemu-user, identical outcomes; the re-review re-ran it
natively):

- **Entry** in `.init_array.00000` and `.fini_array.00000` of every
  module that carries the runtime, executable and shared object alike
  (priorities 0-100 are the implementation's; user code starts at 101).
  Case I: the runtime runs before a user `constructor(101)` and before
  default constructors, under GNU ld, lld and mold.
- **One index per scope, found through the loader's own binding**: every
  runtime copy defines one exported `__mason_odr_state` and reads it
  through its GOT, so each module binds the first copy its lookup scope
  reaches; a `dlmopen` namespace never sees another's (case E). No
  loader internal is read. Under a version script the state object must
  be exempt or carry a fixed node (`MASON_1`): with per-library nodes
  each library binds its own versioned copy and is silently compared
  with nothing (re-review R6, `vhook`).
- **Memory**: registry and hash index in `mmap`ed memory, no `malloc`, no
  module limit; an `mmap` failure prints `odr-hook-out-of-memory` once
  and `odr-hook-stopped` for each later module, never silence (cases G,
  H).
- **`SYSTEM` rows** are skipped unless `MASON_ODR=strict`.
- **`DT_STRTAB`/`DT_SYMTAB`**: glibc relocates these `d_ptr`s in place
  for the executable, libc and ld.so but not for the vDSO
  (`preinit-dynamic`); a reader adds the load base when `d_ptr` is below
  it. Other loaders (musl) [unverified].
- **Threads and `fork`**: glibc runs constructors and destructors under
  its load lock (`dl_load_lock`, recursive); case J (8 threads × 300
  `dlopen`/`dlclose`, `fork`) ran clean three times per target.

*The open problem.* **The runtime can ask where a module would bind now,
not where it bound.** Prototype 2 asks, for a key seen with two hashes,
each holder `X` to call `dlsym(RTLD_DEFAULT, name)` from its own code.
That answers where a lookup from `X` would go at this moment, which is
not where `X`'s GOT and PLT went:

- **immediate vs lazy binding.** An `RTLD_NOW` module bound its
  references at its load; a later global module changes the answer to
  the question but not the binding: R1m (`h2` `LOCAL|NOW` bound its own
  copy, then `h1` `GLOBAL` and `h1b`) is a correct process that
  prototype 2 aborts. A lazy module binds at first call, after its init
  ran: R3m (`h2lazy` `LOCAL|LAZY`, then `h1lazy` `GLOBAL|LAZY`, then the
  call) really binds the other body and prototype 2 does not see it.
  Debian links lazily by default.
- **promotion.** `dlopen(h1, RTLD_NOLOAD|RTLD_GLOBAL)` moves a loaded
  module into the global scope without loading anything: R2m is a
  correct process that prototype 2 aborts.
- **per-library version nodes.** References carry the version of the
  definition they were linked against (`.gnu.version` of the referencing
  module); an unversioned `dlsym` finds another library's default
  version: R6 (`vmhook`, two libraries with nodes `LIB1_1`, `LIB2_1`) is a
  correct process that prototype 2 aborts.
- **protected and `-Bsymbolic` definitions** are bound by other modules
  (R4/R5); prototype 2's tables had no rows for them (fixed in O-50).

In M6 the hook **never aborts**: whatever WP-077 produces reports and
continues. `MASON_ODR=abort` is a later decision, after its matrix is
green.

*Candidate mechanisms WP-077 must evaluate, without this spec choosing:*

1. **Recorded answers**: each module records, once at its own init (or
   at first need), where each of its keyed references bound, by reading
   its own GOT entry for the symbol (filled at load for data, and for
   functions under `-z now`, which mortar could force for modules with a
   table) and never re-asks. Evaluate: lazy PLT entries (R3m) unless
   `-z now`; the cost of forcing `-z now`; promotion (R2m).
2. **glibc's audit interface** (`LD_AUDIT`, `la_symbind32/64`,
   `la_objopen`): reports actual bindings, lazy ones included, with the
   binding module and the version. Evaluate: it needs a separate audit
   library named in the environment (it cannot be a linked-in object of
   the program), it disables some lazy-binding fast paths, it is
   glibc-only, and whether that is usable for development builds at all.
3. **Comparing only within the initial global scope** (the executable
   and its `DT_NEEDED` closure, bound before any user code runs) and
   treating every `dlopen`ed module as a named exclusion or a warning
   only. Evaluate: what it misses (C1, B1, D) against what it avoids
   (R1m, R2m, R3m).
4. **Versioned keys**: rows identified by (name, version node) as in
   O-24, lookups with `dlvsym` using the version the reference names.
   Evaluate together with 1-3; it is needed by every option (R6).

*Measurements of prototype 2* (AArch64, in-process time summed over every
module's init entry, three runs; `MASON_ODR_TIME`):

| modules × rows | prototype 2 | first draft's pairwise prototype |
|---|---|---|
| 4 × 1,250 | 0.08-0.09 ms | 0.03 ms |
| 4 × 12,500 | 0.81-0.94 ms | 0.32 ms |
| 4 × 125,000 | 16.5-18.3 ms | 3.9 ms |
| 300 × 1,250, clean | 6.4-6.9 ms | 143 ms (review), and missed past 256 |
| 300 × 1,250, one violating key | 43.8-47.4 ms (one `dlsym` per module holding the key) | |

The clean path is linear in rows and independent of the module count;
the **conflict path is quadratic** in the number of holders of one key:
N legal `RTLD_LOCAL` plugins with private versions of one key make the
key `MULTI` from the second on, and each load asks every earlier holder
(re-review `hook-quadratic`: 0.1 ms at 50 plugins, 0.6 ms at 100,
3.2-3.4 ms at 200). WP-077 measures it at 300 with K = 10 keys and
either bounds it or caches each holder's answer (candidate 1 needs the
cache anyway). The x86-64 and i386 timings under qemu emulate the code
and are not costs.

*The matrix* (Appendix A, experiment 5 and the re-review's cases;
REQUIRED for WP-077, on AArch64, x86-64 and i386; R cases need not pass
today):

| case | shape | required outcome | prototype 2 |
|---|---|---|---|
| A1-A3 | two `RTLD_LOCAL` plugins with private versions, non-Mason and Mason host (N29) | no report | pass |
| E | `dlmopen(LM_ID_NEWLM)` (N30) | no report | pass |
| F | `dlclose`, then another global plugin | no report | pass |
| C2 | plugin 2 `RTLD_LOCAL`, then plugin 1 `RTLD_GLOBAL` | no report | pass |
| R1m | `h2` `LOCAL\|NOW`, `h1` `GLOBAL`, `h1b` `LOCAL` (same body as `h1`), Mason host | no report | **fails** (abort) |
| R2m | `h2` `LOCAL`, `h1` `LOCAL`, `h1` `NOLOAD\|GLOBAL`, `h1b` `LOCAL`, Mason host | no report | **fails** (abort) |
| R6 | two libraries, one version node each, different bodies, Mason executable (`vmhook`) and none (`vhook`) | no report; `vhook` must not be silently unchecked | **fails** (abort; `vhook` silent) |
| R8m | `RTLD_DEEPBIND` plugin binding its own copy | no report | measured by the re-review's script |
| B1, B2 | Mason host exports v1, `RTLD_LOCAL` plugin v2 binds it | report | pass |
| C1 | plugin 1 `GLOBAL`, plugin 2 `LOCAL` binds it | report | pass |
| D | a plugin's dependency binds the plugin's different copy | report | pass |
| G | 300 modules × 1,250 rows, a violation in module 300 | report; cost recorded | pass |
| R3m | lazy binding of another module's different body after init | report, or a named exclusion of the prototype's rule | **fails** (silent) |
| R4/R5 | `-Bsymbolic`, `-Bsymbolic-functions`, `protected` definition bound by a later library's references | report, or a named exclusion | **fails** (no rows) |
| versioned | `dlvsym` with the reference's version; the version in the key | per (name, version) | not prototyped |
| H | `mmap` failure | named warnings, never silence | pass |
| I | init order, three linkers, `.so` and executable | runtime first | pass |
| J | threads, `dlopen`/`dlclose`, `fork` | no crash, no hang | pass |
| quadratic | `hook-quadratic` at 300 plugins, K = 10 | cost recorded and bounded | not run at K = 10 |

**O-55 (what only load time sees).** The static link proves what O-24
says; only a runtime could add a different build of a library at run
time, `dlopen`ed libraries and indirect dependencies (X12 until then).

**O-56 (compatibility, for the prototype).** A mortar-made executable
with foreign libraries: they have no table and are not checked. Intentional
interposition of a vague definition (`LD_PRELOAD`) would be reported;
interposing strong definitions (`malloc`) is in no table (N5).

**O-57 (the D-21 boundary).** K18c shows that a library loaded with
`RTLD_LOCAL` gets its own instance of a vague definition the executable
does not export (`spec/dyn.md` §14). The table does not change that; Q8
asks whether an M6 export class should.

**O-58 (the gate).** No output gets the runtime in M6. WP-077 is a
**prototype package**: its deliverable is the matrix above, green on
three targets with the candidate mechanism it chose and the reasons it
rejected the others, not shipped code. Experiment 5 does **not** count
as passed until the matrix's must-not-report cases (A1-A3, E, F, C2,
R1m, R2m, R6, R8m) give no report and its must-report cases (B1, C1, D,
G, R3m, R4/R5) report or are named exclusions, with versioned lookup and
the quadratic cost measured. A package that ships a runtime is cut only
after that **and** the operator's decisions Q2 and Q3; `MASON_ODR=abort`
after a further decision.

**O-59 (what the verifier checks of the dynamic side).** §12.5's rules
for the table; the runtime rule applies only if a runtime ships.

## 11. Requests to kiln and hod

Each request stands alone for a reader without access to mortar's
workspace. Shared context: **Mason** is a C++ toolchain: kiln (front
end), hod (code generator), mortar (linker), tuck (debugger, future).
Mortar checks the one-definition rule across translation units at link
time from records the compiler writes into each object, in a section
`.mason.meta` that every other linker drops. The container is specified
in `spec/meta.md` and the checks in `spec/odr.md` of mortar's repository
(`git@github.com:christopherfretz/mortar.git`, default branch `main`,
files `spec/meta.md` and `spec/odr.md`; the rule ids below are theirs).
These requests are sent once the specs are merged to `main`. Nothing in mortar
depends on these requests to link correctly; each one turns on a check.
Estimates are the receiving agents' own (kiln's and hod's review,
2026-11-15), except K-2's, which kiln confirmed.

### kiln

- **K-1 — the flag and the emission.** Add a flag with levels
  (`min`: record kinds 1 and 2; `default`: 1, 2, 3, 5; `detail`: + 4;
  `all`: + rows for definitions inlined away: the `name` string on
  every vague-linkage kind 2 row, O-15) and
  its negation; this spec calls it `-fmason-metadata[=…]` and the global
  `@__mason_meta`, but **the flag's and the global's spellings are
  kiln's to choose** (the operator's call there): mortar fixes only the
  section name `.mason.meta` and the bytes. Emit one IR global per
  module, e.g. `@__mason_meta = private constant [N x i8] c"…", section
  ".mason.meta", align 8, !exclude !0`, listed in `@llvm.compiler.used`
  (without it `opt -O2` deletes the unreferenced global: measured), with
  no `comdat` and an initializer that needs no relocation; llc 19 and 22 already lower exactly this to the
  required section (`SHT_PROGBITS`, flags exactly `SHF_EXCLUDE`, align
  8, in no COMDAT group, no relocations: mortar `spec/meta.md` MT-1,
  MT-2). The bytes are one **unit**: a 32-byte header (`MASN`, version
  1.0, sizes, block count, string table) and one block per record kind,
  rows sorted by key (FNV-1a 64 of the mangled name); layouts in
  `spec/meta.md` §4 and the row tables of `spec/odr.md` O-15, O-17,
  O-35. Rows of kinds 2 and 5 carry no name: they are "anchored" to the
  object's symbol whose FNV-1a 64 equals the key. Where: at the end of
  module emission, when the final set of globals (linkage, comdat) is
  known (`kiln_codegen/src/module.rs`). How kiln finds the emitted
  vague-linkage definitions is kiln's choice (it collects kind 2 after
  emission from the unit's declaration maps, as kinds 3 and 5); what
  binds: rows only for definitions the object defines a symbol for
  (below `all`), deterministic bytes, nothing runs with the flag off. Deterministic bytes (no pointer
  values, no hash-map order, paths after `-ffile-prefix-map`). C
  translation units write kinds 1 and 5 only. **Survival through the
  optimizer, measured** (`peer/run.sh`, `run2.sh`, 2026-11-15): IR
  carrying the global in `@llvm.compiler.used`, through `opt` 19 and 22
  at `-O1`, `-O2`, `-O3`, `-Os` (AArch64 and x86-64), keeps the global and
  its `!exclude`; `llc -O2` of the result, and `clang` 19 and 22 `-O2 -c`
  of the IR for AArch64, x86-64 and i686, give `.mason.meta`,
  `SHT_PROGBITS`, flags `E` only, align 8, the input's bytes, no
  `.rela.mason.meta`, no group. The section's index in the section table
  varied (2 or 3), which is why mortar finds it by name and type only
  (MT-1). **LTO is a named non-goal** until mortar has LTO (X10). On the
  hod route (`-fkiln-backend=hod`) H-1 is a **hard prerequisite**: before
  it, `!exclude` makes hod fail and kiln falls back to llc with a warning
  per TU. *Why:* nothing else reaches the linker. *Test:* mortar's test
  producer writes the same bytes for the same rows. *Estimate:* with K-2,
  one kiln package (about 1000 changed lines; kiln's estimate).
- **K-2 — kind 1 (TU summary).** Rows of (name, value) strings, sorted:
  `producer`, `source`, `odr.scheme`, `std`, `lang` (`c`/`c++`),
  `abi.rtti`, `abi.exceptions`, `abi.short-enums`, `abi.stdlib`,
  `abi.stdlib-abi`, `abi.stdlib-debug`, `abi.stdlib-hardening`,
  `abi.struct-return` (i386),
  `abi.char`, `abi.wchar`, `abi.long-double`, `isa`, with the `ENFORCE`
  flag on the layout-changing ones (`spec/meta.md` MT-26 lists which;
  `abi.rtti` is not enforced). From the driver's options and the target.
  *Estimate:* 1 day (kiln agrees); lands with K-1.
- **K-3 — the definition hasher (kind 2).** A pass over the semantic AST
  after template instantiation, for every emitted definition that may
  appear in more than one TU (inline functions and variables, template
  instantiations, explicit specializations, static members of class
  templates, out-of-line `static constexpr` members; `spec/odr.md`
  O-14), serializing canonically what `spec/odr.md` O-10 lists, hashed
  to 128 bits (FNV-1a 128 or SipHash-1-3, named by a scheme string), with
  the flags of O-15 and the odr-use analysis of O-16. The traps (after
  kiln's review; O-10 is normative):
  - specifiers under the rule that needs **no new AST bit**: `constexpr`
    is never hashed on implicitly declared members, defaulted functions
    and closure call operators; `noexcept` is hashed as the effective
    specification; a variable's `inline` is not hashed;
  - literals by **value and type**; of the implicit nodes, only the
    resolved function of each call and user-defined conversion;
  - an enumerator reference hashes as (enum type, name, **value**);
  - local entities by declaration index, not spelling; lambdas and local
    classes by mangling number or index, closures by resolved captures;
  - the closed **attribute list** of O-10 (scheme 1), every other
    attribute not hashed; contracts not hashed in scheme 1; default
    arguments as O-10 states (namespace-scope, non-dependent only);
  - builtin-macro and source-location values hash as placeholders. No
    preprocessor change is needed for direct builtin results: each
    already has its own `ExpansionKind::Macro { name }` record at the
    macro-name token (`kiln_pp` `expand.rs`), so `__LINE__`,
    `__COUNTER__` and a single-piece `__FILE__` are recognisable today;
    `__func__`, `__builtin_FILE` & co. and `source_location::current()`
    are node kinds of their own. New work is (i) a **sema side table**
    giving a concatenated string literal per-piece provenance, so the
    hasher hashes a builtin-macro piece as its placeholder and every other
    piece as text, and (ii) a **small preprocessor mark** on `#`/`##`
    results whose operand came from a builtin macro. Until each exists,
    the conservative form (O-10, X17) hashes the whole literal, and every
    `#`/`##` result, as one placeholder, named in the scheme;
  - O-11's **bump rule** with a golden-hash test;
  - the same definition reached textually, through a precompiled header
    and through a header unit hashes equally (kiln: PCH restores the
    identical AST, header units are in-process parses); a definition
    imported from a named module takes the hash stored in its BMI when the
    module was built (direction, with K-6);
  - implicitly defined members as sema synthesizes them;
    constant-evaluated initializers hash as expressions, not values (K9);
    `gnu_inline` definitions get no row.
  Keep the two corpora of O-13 (must-equal, must-differ) as kiln tests.
  **Acceptance criterion: at most 3% of front-end time at `default` on
  kiln's benches, measured and reported** (it is one more walk over every
  emitted body): kind 2's INCREMENT, front-end time at `default` with
  kind 2 against `default` without it, as a fraction of front-end time
  with the flag off; the whole flag (on against off) is reported beside
  it and has no gate of its own yet. **Landing in steps** (kiln's plan
  of 2026-11-17, accepted): rows first with a hash over the declared
  type and `PARTIAL` on every row, then the serializer by construct
  class, rows losing `PARTIAL` as their constructs are covered (O-15);
  one row per symbol of a definition (O-14); an internal-linkage entity
  by its mangled name and per-unit-numbered encodings as one placeholder
  (O-10); kind 3's `hash` stays zero (O-17); `module` 0 and `GMF` unset
  until K-6. *Estimate:* 5-7 kiln packages of about 1000 changed
  lines (kiln's estimate; it replaces the designer's 3-4 weeks).
- **K-4 — kinds 3 and 4 (types).** For C++ TUs only: one row per
  class, struct or union **whose layout this TU computed** (a
  `RecordLayout` exists), and per enumeration defined in the TU whose type
  or enumerators appear in an emitted definition, **only if** it passes
  O-17's **syntactic** not-TU-local test (named; every enclosing scope a
  named namespace or a passing class; template arguments built only from
  fundamental and passing types or integral, enumeration and `nullptr`
  values). It is a conservative under-approximation of [basic.link]
  p13-15: kiln needs no TU-locality engine (it has none); anything not
  provably non-TU-local gets no row; **unnamed types never get a row**
  (their manglings are numbered per TU). The 64-bit layout hash of O-18
  (size, alignment, bases with offsets, members with bit offsets and
  types, vptr offset, vtable slots by final overrider's mangled name,
  enumerators with values; **no RTTI slot**), from `RecordLayout` and
  `VTableLayout`, which already hold every field. Kind 4 (readable
  layout) at `detail`. *Estimate:* 2 kiln packages (the syntactic test,
  then the rows; kiln's estimate).
- **K-5 — kind 5 (declarations).** One row per defined or referenced
  external function or variable: a 64-bit hash of the declared type's
  Itanium encoding with a variable's major array bound erased, `sizeof`
  for variables, flags (`spec/odr.md` O-35, with its "No row" list).
  **Landed: kiln commit 9744b3649** (kiln `spec/codegen.md` §5.5). For **C-linkage** entities (in C units, and
  `extern "C"` in C++ units), hash a canonical **C** type instead: an
  enumeration replaced by its compatible integer type (the one kiln
  chose), top-level parameter qualifiers dropped, C++'s builtin types
  mapped to C's for the target (`wchar_t` to the target's `wchar_t`
  typedef type, `char16_t`/`char32_t` to `uint_least16_t`/
  `uint_least32_t`, `char8_t` to `unsigned char`, `bool` = `_Bool`);
  unprototyped declarations get no row, and `f()` is prototyped only in
  C23 and later (O-33). The canonical C type is **conservative** where C
  and C++ spellings legitimately differ (O-33): every record type opaque
  (`R`), `va_list` one token (`V`; `std::__va_list` on AArch64 C++),
  `noexcept` dropped at every level including nested function-pointer
  types, references as pointers, `restrict` dropped, a transparent union
  as its first member's type, typedefs resolved; measured at zero
  differences on glibc's headers as C and as C++ (O-33). kiln already
  has `mangle_type` for a bare type, the enum's chosen integer type and
  the unprototyped flag. *Estimate:* 1 kiln package (kiln's estimate).
- **K-6 — modules.** **Its own design, after K-3, sharing one
  serializer** (not one hash): the comparison `kiln/spec/modules.md` §10
  item 11 skips today (definitions merged from global module fragments;
  the planned "ODR key / structural hash") needs hashes of everything a
  GMF declares (class definitions as declared, enums, uninstantiated
  templates) at import time, so the serializer must handle **dependent
  nodes**, which K-3 does not (they are K-6's work, not K-3's); kiln's
  expression profiler treats lambdas as unique and is not reusable as is.
  The BMI also stores the K-3 hashes of the module's definitions (O-10). Mortar measured that clang 19 and 22 do not
  diagnose two named modules whose global module fragments define one
  inline function differently, and that the linked program runs the first
  module's copy (`spec/odr.md` K19); kiln can diagnose it at import. Rows
  of GMF definitions carry flag `GMF`. *Estimate:* 2-3 kiln packages
  (kiln's).
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
  kiln's hod route cannot carry the records (kiln's llc route can): H-1
  is a **hard prerequisite of K-1 on the hod route**. hod confirmed
  (2026-11-15) that it writes one section per (name, flags, type, group)
  as llc does, never adds a group or `SHF_MERGE`, and that the size is
  the initializer's; the section's **position in the section table is
  not promised** to match llc's, and mortar does not need it: it finds the
  container by name and type only (`meta.md` MT-1). *Estimate:* half a
  day including tests against llc 19 and 22 on both targets and a link
  check with GNU ld, lld and mold (hod's estimate).
- **H-2 — opacity.** hod never interprets the section's bytes: its
  contents equal the IR constant's bytes (a test).
- **Status: H-1 and H-2 LANDED** (hod `a598555f`, hod's rule 9.7.4 and
  its record WP-273; measured by mortar's WP-099,
  `spec/landed/meta/WP-099.md`). The exact condition: the section gets
  `SHF_EXCLUDE` without `SHF_ALLOC` only when the variable has an explicit
  section, is not thread-local, not common, and its **first** `!exclude`
  node is an empty tuple. For `@__mason_meta = private constant [N x i8]
  c"…", section ".mason.meta", align 8, !exclude !{}` in
  `@llvm.compiler.used` (what `cargo xtask meta write --ll` writes) hod
  writes `.mason.meta`, `SHT_PROGBITS`, flags exactly `E`, alignment 8, no
  group, no relocation section, the initializer's bytes. Other flags join
  `E` as llc adds them: `R` (`SHF_GNU_RETAIN`) for a global in
  `@llvm.used`; `G` in a comdat; `l` for an x86-64 large-section name; a
  `.bss*`/`.tbss*`/`.tdata*` name loses the exclusion; an initializer
  with a pointer gets a `.rela` section; two excluded variables of one
  section name share one section (their initializers concatenated).
  **What mortar relies on**: the name, type and flags of MT-1/MT-6 and the
  bytes (H-2); nothing else. **What it does not**: the section's
  position, a symbol in it (hod writes none: neither the private global's
  nor a section symbol), the order of the other sections, flags beyond `E`
  (each makes the section not a container: `meta.md` MT-6, MT-2a, a
  warning or a drop, never records read). Equality: the one-definition-rule
  corpus through hod's container and through the test producer's gives
  the same answer on both 64-bit targets with every opt-in (WP-099,
  `cargo xtask mason producers`). hod does not target i386; i386 Mason
  objects do not exist.
- **H-3 — the relocation model, in a unit of hod's own** [M4; requested
  by WP-115 (`spec/landed/dyn/WP-110.md` §G.5); mortar's reader WP-129;
  sent to hod 2026-11-16]. *Context:* mortar
  refuses, in a shared object, non-PIC code that binds the library's own
  exported data (`spec/dyn.md` D-117). For GCC and Clang objects it
  infers this from relocations and an AArch64 register analysis, which
  leave a measured residual hole (3 of 306 bound definitions) and cost a
  decode of the code of objects that pass a pre-filter. hod knows the
  model it compiled under; one row makes the refusal exact for every hod
  object and needs no analysis. *Ask:* in every object hod writes, a
  `.mason.meta` unit of its own (`meta.md` MT-10: magic `MASN`, major 1,
  minor 0, `producer` 2, `flags` 0) with one kind-1 block (MT-11,
  `version` 1, `row_size` 16) holding one row (MT-26: `{u32 name; u32
  value; u32 flags; u32 reserved}`, both strings in the unit's string
  table, MT-12, which starts with the empty string): name `reloc-model`,
  value `static`, `pie` or `pic` (the model hod's code generation used:
  `-fno-pic`, `-fPIE`, `-fPIC`; `pic` exactly when the module's own
  default-visibility definitions are reached through the GOT, as llc's
  `pic` relocation model), flags 0 (not `ENFORCE`). It may be emitted as
  a second private excluded global in section `.mason.meta`, listed in
  `@llvm.compiler.used` and **not** `@llvm.used` (which adds `R`), in no
  comdat (which adds `G`): a section with any flag beyond `SHF_EXCLUDE`
  is not a container (MT-6, MT-2) and its units are never read. hod's
  H-1 lowering concatenates the initializers of one section name, so
  kiln's unit, when present, and hod's unit share one container (each a
  multiple of 8 bytes, MT-10); H-2 still holds for kiln's bytes. Or it
  may be written by the object API directly; either way the section is
  H-1's (`SHT_PROGBITS`, flags exactly `E`, alignment 8, no group, no
  relocation section, MT-2a). *What mortar does with it* (`dyn.md`
  D-117; the read is a check under MT-42, done only in a `Shared` link
  and only for objects that define an exported preemptible data
  definition): `Shared`: a unit saying `static` or `pie` in such an
  object refuses it, `non-pic-in-shared-object` (naming the object and
  the row; exact, no analysis); `pic`, or no unit, or no row: D-117's
  rules and analysis as for any object (the row never exempts). `Exec`,
  `Pie`, static: no effect (information). `-r`: units are concatenated
  unchanged (MT-10); a later `Shared` link refuses an object any of
  whose units says `static` or `pie`. *Why only hod:* a GCC or Clang
  object carries the model nowhere: `DW_AT_producer` exists only with
  `-g`; GCC's records the switches given on the line, so its default
  `-fPIE` is absent, and Clang records none unless
  `-grecord-command-line`; no ELF header or note field holds it.
  *Amendments* (R-MT11, applied as planned rules in `meta.md`): MT-10's
  "one per translation unit" and MT-20's "one unit per TU" become "one
  per translation unit and producer"; MT-26's table gains the row
  `reloc-model` (`static`/`pie`/`pic`, enforce no, "dyn.md D-117: exact
  refusal of non-PIC code in a shared object") marked [hod]. *Test:* for
  each target (x86-64, AArch64) and model, the object holds exactly one
  hod unit with the row, read back by `mortar_meta`'s MT-10 reader; with
  kiln's container present, both units parse in order; the rest of the
  object is unchanged (hod's irdiff against llc, `.mason.meta`
  excluded). *Estimate:* half a day (hod's to confirm).
- Nothing else beyond H-3: per-section content hashes are not needed
  (`spec/incr.md` IN-22); DWARF is unchanged.

### Landing order

kiln's order and estimates (peer review, 2026-11-15; kiln packages of
about 1000 changed lines). Starting any kiln or hod step is the
operator's decision. Each step is useful and testable alone:

1. **mortar** WP-071 (container reader, writer, test producer; its
   acceptance test is experiment 7) and WP-072 (W1-W3, switches): no other
   project needed.
2. **hod** H-1 (half a day): useful to any IR producer; tested by irdiff;
   needed before K-1 on the hod route. **Done** (hod `a598555f`, with
   H-2; mortar's side WP-099).
3. **kiln** K-1 + K-2 (1 package): units with kind 1 only.
4. **kiln** K-5 (1 package): kind 5, C++ and C linkage.
5. **kiln** K-4 (2 packages: the syntactic test, then the rows): kind 3.
   **Landed**: kiln main 55b7e0538 (class rows) and d854b23bc (K-4b:
   enumeration rows, kind 4 at `detail`); read back by mortar on 48
   units and `format`-heavy code (WP-134, `spec/landed/meta/WP-134.md`).
6. **kiln** K-3 (5-7 packages): kind 2; it carries the must-equal risk
   and a standing maintenance cost, so it comes after the cheap kinds that
   catch the memory-corrupting violations (K4, K14).
7. **kiln** K-6 (2-3 packages): its own design after K-3.

Each kind's mortar check runs under `--odr=strict` as soon as its kiln
step lands (WP-073 is written against the test producer) and turns on by
default after the oracle (O-46): kinds 5 and 3 can therefore move before
kind 2. **mortar** WP-075 (O-24's prototype, then tables and O-24) needs
kind 2 and follows M2's dynamic executable; `.so` outputs with M4.
WP-077 (the hook's prototype matrix) after WP-075; nothing ships from it
without O-58's gate and the operator's Q2 and Q3.

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
time (O-24, with the shapes of T-O-24); at load time, the prototype
matrix of O-54 (WP-077 only). Per producer: kiln
(records: the expected diagnostic, by id and places, under
`--odr=strict` and, once O-47 lists the check, by default) and, before
kiln emits, the test producer of `meta.md` §12 writing the rows kiln
would (the model's constructions are its first cases); GCC and Clang (no
records: the W outcome of §8.1, which is "not detected" for most kinds,
recorded as such so that a change is visible). Link tests use the `#@`
format (`spec/harness.md` "Link tests") with a new directive `#@ diag:
<id> <file>...` (amendment A-6).

### 12.2 The legal corpus

N1-N31 of §3 as link tests that must link with **no finding of any
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
on purpose, and a second reader is the point). The rules (amendment
A-7, applied as `spec/plumb.md` §13, P-M1-P-M3):

- an executable or shared object contains no `.mason.meta` unless
  `--keep-mason-meta` (MT-31, MT-32); a relocatable output's
  `.mason.meta` is the concatenation of the live inputs' units in input
  order (MT-30);
- a `.note.mason`, if present, lies in a `PT_NOTE` with `p_align` 8; its
  rows are sorted; each row's key is the FNV-1a 64 of its `.dynsym` name;
  each row's hash is the first 8 bytes of the kind 2 row of **the
  keeper's unit** for that name (O-51; the keeper is `comdat[G]`'s file
  for a vague definition, the defining file otherwise); the row set
  equals O-50's (exported definitions, protected and `-Bsymbolic` ones
  included, hidden ones not); each row's flags equal the keeper's row's,
  plus `NONPREEMPTIBLE` exactly for protected and locally bound
  definitions; each row's `.dynsym` entry carries the version node the
  output gives the name;
- a table adds no code: no `.init_array`/`.fini_array` entry and no
  input section that mortar did not read from the command line. If a
  runtime ever ships (O-58), a rule for its entries and bytes is added
  then.

Whether `mortar_plumb` should also re-run the comparison (a second
implementation of the verdict) is Q11.

### 12.6 Tests of the rules

| rule | test | what it shows |
|---|---|---|
| O-1, O-3, O-46, O-47 | T-O-1 | a CI check that every check O-47 lists as on has a recorded zero-finding oracle run in the landed record of the package that moved it; the legal corpus has no finding of a default-on check |
| O-2 | T-O-2 | `static` functions of one name in two TUs, with rows forced by the test producer: no comparison; extended to types: N31's unnamed-namespace type, `std::optional` of it, local class and closure write no kind 3 row and are never compared (model case and, with K-4, a kiln test) |
| O-4 | T-O-4 | anchored, unanchored (`--redefine-sym`), ambiguous (two names, one FNV-1a 64, constructed) and versioned (N6) rows |
| O-10..O-13 | T-O-10 (kiln) | the corpora of O-13, as kiln tests and as L11 links; the golden-hash test of O-11; two units with different schemes give only an `--odr-stats` count (X13); mortar's side of the two schemes (WP-151, `odr_records.rs::layout_schemes`): a layout difference between equal layout schemes and different `odr.scheme`, a hash difference not compared there, a unit without `odr.scheme` silent and counted, a unit without `odr.layout-scheme` compared on its `odr.scheme`, the shape exception across layout schemes, the definition note across them, both stats counts, and the legal pairs silent |
| O-14, O-15 | T-O-14 | rows per definition kind; K13's flags; N9; N22 (strong + `VAGUE`, equal); a closure's `operator()` and a generic lambda's specializations get rows, the static invoker none; a lambda in a declarator gives no row (X15); mortar's side (WP-151, `odr_records.rs`): `named_rows_with_and_without_symbols` (a named row anchored on its symbol, one without a symbol compared under strict only, N12 at `all` + strict reported and silent by default), `partial_rows` (two `PARTIAL` rows and a `PARTIAL` against a full row: warnings; equal: silent) |
| O-16 | T-O-16 | K12 under strict; nothing by default |
| O-17, O-18 | T-O-17 | K4 (also pointer-only, layout computed in both TUs), K7, K16a, N24 (no RTTI bit); the syntactic test: `std::optional<int>` and `ns::Widget<ns::Gadget*>` get rows, `std::optional<(anonymous)::S>`, a local class, a closure and an anonymous struct none (N31); a zero `hash` compares layout only |
| O-19 | T-O-19 | a kind 4 entry renders the first difference: per item kind (member, bit-field, base, vptr, vtable slot, enumerator, underlying type), both directions, an item in one entry only, one entry a prefix of the other, the same items in another order, equal entries, nested unnamed members by `width`, signed and unsigned enumerators, the slot help note, equal enumerations (`crates/mortar_odr/src/details.rs`; end to end `odr_records.rs::kind4_reader_notes`, WP-151); end to end, malformed entries and the cost counter (`crates/mortar/tests/odr_records.rs` `kind4_first_difference`); the reader (`crates/mortar_meta/tests/details.rs`); `tests/link/odr/k4-details.test` and `k4-details-default.test` (WP-145) |
| O-20, O-21 | T-O-21 | the optimisation and the normative rule report the same findings over the whole corpus, K13 included |
| O-22 | T-O-22 | kind 3 and 5 rows from 3 units grouped in `FileId` order |
| O-23 | T-O-23 | K17 with and without `--odr-check-archives` (WP-076: `tests/link/odr/k17-archive-{shadow,silent,thin}.test`: a differing member warns, an equal one and one without a container say nothing, thin archives, the option off; `crates/mortar/tests/odr_records.rs::archive_members_are_read_only_on_request`: the read counter in each mode with the option on and off, a member extracted by a later reference compared as a live unit, a non-ELF member; `partial_rows`: a member's `PARTIAL` row set aside below strict, compared under it, WP-151) |
| O-24 | T-O-24 | the shapes listed in O-24: (a) exported and hidden; (b) two libraries; `-Bsymbolic`, `-Bsymbolic-functions` and `protected` libraries (R4/R5, findings); an `--as-needed`-dropped library (none); one version node per library (R6, none) and a shared node (a finding); unversioned against versioned, both orders; (c) K18b; a malformed table; libraries without tables and the default mode (none). `tests/link/dyn/odr/o24-*.test` (WP-075) |
| O-25 | T-O-25 | K16c with two vtable copies; F5 silent |
| O-30, O-31 | T-O-30 | K16a/K16e/K16d notes under strict; `abi.rtti` never a finding |
| O-32 | T-O-32 | `mixed-std-system-row` → a stats count; `mixed-std-user-body` → a finding |
| O-33 | T-O-33 | N27, N28 (strict only), K14b; the canonical C type: glibc's declarations as C and as C++ `extern "C"` give zero differences (`peer/run3.sh`), and a real parameter-type difference (`int` vs `long`) still differs |
| O-34 | T-O-34 | kind 5 against a foreign definition's size |
| O-35 | T-O-35 | K14, N15; the place lines "declared in" / "declared differently in" and a C/C++ pair's notes without `lang`/`std` (WP-151, `odr_records.rs::declared_place_lines_and_linkage_pair_notes`) |
| O-40, O-41 | T-O-41 | each mode's severities; `--odr=off` output byte-identical; `--detect-odr-violations` a no-op that prints nothing |
| O-42 | T-O-42 | `meta.md` T-MT-42: bytes read counted |
| O-44 | T-O-44 | the stats line's fields on a corpus link |
| O-45 | T-O-45 | the bench lane's comparison |
| O-50, O-51 | T-O-50 | `=full` (WP-076, `tests/link/dyn/odr/o50-full{,-input}.test`, `odr_dynamic.rs::full_table_in_a_dynamic_executable`, `mortar_meta::table`'s mutation test of the second note): hidden vague definitions and types in the second note, the first note unchanged; a `.so` input with a `=full` table read by O-24 as with `=exported`. The row set: default, protected, `-Bsymbolic` and `-Bsymbolic-functions` definitions have rows (the last three `NONPREEMPTIBLE`), hidden and version-script `local:` ones do not; versioned names; the keeper's hash after a warned link. In an executable (WP-075, `tests/link/dyn/odr/o50-table.test`, `crates/mortar/tests/odr_dynamic.rs`): default and protected rows, every one `NONPREEMPTIBLE`, none for hidden; identical bytes on two links; none without the option |
| O-52 | T-O-52 | `note-final`'s tolerance matrix |
| O-53, O-54, O-56 | T-O-54 | WP-077's prototype matrix (O-58) on three targets; not a release test until a runtime ships |
| O-55, O-57 | T-O-57 | K18c's two outcomes unchanged by the table |
| O-58 | T-O-58 | no output carries a runtime in M6; no table without `--odr-table` |
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
| WP-071 | container reader and writer; MT-6 classification; `meta-malformed`/`meta-version`; the test producer; the `--odr=off` path and A-8 (MT-42, O-42). **Acceptance test = experiment 7**: real units written by the test producer at the row counts of the heavy TU's object and of protoc's 167 objects (kind 2 per vague symbol, kind 5 per defined/undefined global, kind 3 per DWARF type); unit sizes by kind; bytes read with `--odr=off` and in the default mode under the staged store (zero container bytes for every object, small ones included); the cost of reading and grouping the rows (O-22) on the bench lane against O-45. MT-41's gate is set from these numbers in its landed record | `mortar_meta`; `mortar_input` (A-8); `mortar_resolve` (A-1's row); `xtask` (producer) | A-1, A-2, A-8 applied | `.s` units assembled by GNU as | 4-5 days [estimate] | WP-072 |
| WP-072 | W1-W3 (W1, W2 on; W3, W4 strict); the diagnostic kinds; `--odr=`, `--no-odr-check`, `--odr-stats`, `--detect-odr-violations` no-op; O-47's register as code | `mortar_odr`, `mortar_diag`, `mortar_cli` | A-4, A-5; `comdat_copies` (M1) | GCC/Clang objects of the catalogue and the legal corpus, experiment 1's corpora | 4-5 days | WP-071 |
| WP-073 | record checks, all `--odr=strict`, in kiln's landing order so each is ready when its kind lands: kind 1 (notes, O-30, O-31), kind 5 (O-35, O-33's canonical rows), kind 3 (O-17, O-18), then kind 2 (O-4-O-16, O-25); the L11 static corpus (the model's 13 cases first) | `mortar_odr`, `tests/link/odr/` | WP-071, WP-072 | the test producer (no kiln) | 1.5-2 weeks | kiln K-1..K-5 |
| WP-074 | the Mason L11 lane and the false-positive oracle (§12.3, the C corpus of O-33); moves checks per O-46, per kind as kiln lands it: kind 1 and kind 5 after K-1/K-2/K-5, kind 3 after K-4, kind 2 after K-3 (so kinds 5 and 3 can run, and turn on, before kind 2) | `xtask`, `tests/mason/` | WP-073; kiln K-1 + K-2 to start, then K-5, K-4, K-3; hod H-1 for the hod route | kiln objects | 1-1.5 weeks, spread over kiln's landings | WP-075 |
| WP-075 | the O-24 check against `.so` inputs, **starting with its prototype** (a script over test-producer `.so`s with tables, the shapes of T-O-24: R4/R5, `--as-needed`, R6), then the loadable table (O-50-O-52) for executables and the check in mortar; `[M4]` the same for `.so` outputs | `mortar_odr`, `mortar_synth`, `mortar_layout` | WP-073; M2's dynamic executable; A-3; M4 for `.so` | test-producer units; `note-final` | 1.5-2 weeks | WP-074 |
| WP-076 | opt-ins: `--odr-check-archives`, `--odr-table=full`, `--keep-mason-meta`, `-r` pass-through (MT-30, with M4). Landed without `-r` (`spec/landed/meta/WP-076.md`) | `mortar_odr`, `mortar_meta` | WP-073 | | 4 days | WP-075 |
| WP-077 | **prototype package** for the load-time hook (§10.3): evaluate the candidate mechanisms of O-54 (recorded answers via the module's own GOT with `-z now`; glibc's audit interface; initial global scope only; versioned keys with `dlvsym`) and deliver the matrix of O-58 on three targets, the quadratic conflict path at 300 plugins and K = 10, and a recommendation. Deliverable: the matrix, the measurements and a spec proposal, **not shipped code**; it reports and never aborts | `.build`-style prototype under `xtask` or `tests/hook/` only | WP-075 (tables); no operator decision needed to prototype | `hook2`, `hook-rebind`, `hook-quadratic` | 1.5-2 weeks | WP-078 |
| WP-078 | verifier rules (§12.5) | `mortar_plumb` | A-7; WP-071 (format only) | the outputs of WP-073/075 | 3-4 days | WP-077 |
| WP-079 | apply the amendments of §14 | specs | this draft accepted | — | 1 day | first |

WP-071 and WP-072 need no other project and can start the day this
draft is accepted. M6's "done when" (`docs/ROADMAP.md`) is WP-074's
oracle green with WP-075 landed; WP-077 is not on M6's path, and a
package that ships a runtime is cut only after O-58's gate and the
operator's Q2 and Q3.

**What an implementer may start on today, without any open decision:**
the container reader and writer with its classification, the
`--odr=off` path and A-8's byte rule, and the test producer (WP-071);
the verifier's container rules (the `.mason.meta`-absent and `-r`
concatenation rules of §12.5, part of WP-078); W1 and W2, on by default
as warnings, with the switches and diagnostics (WP-072); and the record
comparison engine behind `--odr=strict` (WP-073: kinds 1, 2, 3, 5, O-25,
O-33, against test-producer units). None of these depends on Q1-Q16,
on kiln's hasher, or on the hook.

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
   (`spec/odr.md` O-42)"; and the whole-file row for objects of at most
   64 KiB gains "when `--odr` is off, or no check that needs the
   container is on, such a file's containers are left out of its
   resident ranges (read with the file, dropped)" (`meta.md` MT-42, as
   ruled 2026-11-15). A test-only byte counter per file range for
   T-MT-42.
3. **A-2 `spec/model.md`** — `SectionKind::Meta`; per file, the list of
   its `Meta` sections (so that `mortar_odr` finds units without a
   rescan); in §8, phase 40's `seq` for row diagnostics without a symbol:
   "`2^40 +` the row's index in its unit". `comdat_copies` stays as it is.
4. **A-3 `spec/dyn.md`** — §14 "M6: ODR data" becomes: "`.note.mason`
   (`spec/odr.md` O-52) is an allocated note of alignment 8, placed with
   the other notes (layout L-60 rank 2), written only with
   `--odr-table`; O-24 reads the tables of shared-object inputs that
   stay in `DT_NEEDED`, identifying rows by (name, version node)". No
   runtime is added in M6 (O-58). D-21 unchanged at M2; Q8's class, if
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

**Applied (WP-079, 2026-11-15).** Those specs number by section, except
`dyn.md` (D-*n*) and `plumb.md` (P-*n*), so ids are new only in
`plumb.md`:

- A-1: `resolve.md` §10, the container row before `SHF_EXCLUDE` and a
  sentence after the table; §6.7, one sentence; §13, "per-file summary".
- A-8: `input.md` §11.1, the two rows and the test-only byte counter;
  §11.9 (A), the small-file exception (a fit: the read plan states the
  whole-file rule a second time).
- A-2: `model.md` §3.2 `files` (the `Meta` list); §3.3 `kind`: `Meta`
  replaces `Metadata` and moves to the kinds without output (a fit); §8,
  phase 40's `seq`.
- A-3: `dyn.md` §14, the "M6: ODR data" bullet replaced; the D-21 bullet
  gains the Q8 sentence. No D rule changed.
- A-5: `cli.md` §3.9, the `odr` field; §4.2, the M6 rows and the
  packages that add them (`rows.rs`; `docs/status/options.md`
  regenerated, not edited).
- A-4: `diag.md` §3.1, the area and 13 rows; §3.2, one note.
- A-6: `harness.md` "The test file", the `diag` directive; "Planned
  workloads", the Mason workloads; "The corpus", the ODR cases and the
  L11 lane.
- A-7: `plumb.md` §1, stage 4; §13, P-M1-P-M3, not implemented (WP-078).

## 15. Open questions

The operator's decisions (Q1, Q2, Q3, Q6) are written out with options
and costs in the WP-070 record (`spec/landed/meta/WP-070.md`, "Operator
decisions"); their recommendations are repeated here.

- **Q1. Default severity.** *Recommend:* `--odr=warn` until each check
  has been on at zero for a release, then the operator decides the move
  to error per check (O-40, O-46).
- **Q2. The table and the hook.** **DECIDED (operator, 2026-11-17): a
  load-time check is wanted, with modes chosen by options: at least
  reporting, and a fatal mode (the load fails or the process aborts on a
  mismatch); fatal is not the only mode.** The table stays opt-in
  (`--odr-table`). What ships and when is still gated by O-58: no mode
  that kills a process before every must-not-report case of §10.3's
  matrix is green. WP-077 (the prototype) proposes the mechanism, the
  modes and their options as rules.
- **Q3. How the runtime's bytes are made.** **DECIDED (operator,
  2026-11-17): the runtime's object code comes from hod, through kiln**
  (the toolchain's own code generator, not a foreign compiler). The
  check code is the same for every program, so it is generated when
  mortar is built, not at each link: source in mortar's tree (C++ in
  the freestanding subset kiln and hod compile at `-O0`; the prototype
  runtime of experiment 5 compiles for AArch64 and x86-64, byte-identical
  on a repeat), the object bytes per target embedded in mortar, CI
  rebuilding them with the pinned kiln and hod and requiring identical
  bytes (O-53). i386: hod has an i386 back end too (`hod_x86` on its
  main; the route to it for this source is being settled with hod's
  agent), so the runtime is built for the three targets; a target
  whose bytes cannot be built by the toolchain has no runtime (the
  option refused by name there), never bytes from a foreign compiler
  shipped silently. Mortar stays `forbid(unsafe_code)`: the runtime is bytes it
  links.
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
- **Q15.** Withdrawn by the coordinator's ruling: A-8 applies to small
  files too; with `--odr=off` no container byte is kept (MT-42, O-42;
  the reads as ruled 2026-11-15, WP-071).
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
| `n31` | `rev/n31.sh` (runs `adv/kind3-tulocal.sh`, then `wcheck.py`) | N31: exit 0 under g++ and clang++ 19 × GNU ld, lld, mold; W1-W3 zero (g++, clang, mixed) |
| peer review, K-1(c) | `peer/run.sh`, `peer/run2.sh` over `peer/m.ll`, `peer/mx.ll` | `opt` 19/22 `-O1..-Os`, `llc -O2`, `clang` 19/22 `-O2 -c` × AArch64, x86-64, i686: the global survives with `@llvm.compiler.used` (deleted without it); `.mason.meta` flags `E`, align 8, bytes verbatim, no relocations, no group; section index 2 or 3 |
| peer review, O-33 | `peer/run3.sh` + `peer/ccanon.py` over `peer/hdrs.h` (24 glibc headers) | the canonical C type of C17 vs C++20 `extern "C"` declarations, clang 19/22 × 3 targets: 0 differences (8 without the transparent-union step) |
| re-review, REQUIRED for WP-077 | `adv/hook-rebind.sh` (R1m, R2m, R3/R3m, R4/R5, R6 `vplain`/`vhook`/`vmhook`/`vu`, R8/R8m, `LD_PRELOAD`), `adv/hook-quadratic.sh` (N plugins, K keys) | the open problem of O-54 and the matrix of O-58; not required to pass now |

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
- Every cost marked `[estimate]`: O-4, O-21, O-22, O-30, the W
  checks' cost in mortar (only Python prototypes ran); the hook's cost on
  real x86 hardware.
- Everything that needs kiln's hasher: the false-positive rates of the
  record checks, of the odr-use analysis and of the C canonical types;
  the model (§12.7) checks the rules, not the hasher.
- O-24 under musl's loader, and glibc's choice for an unversioned
  reference against an earlier versioned library with no definition at
  index 2 (excluded, "Not compared"). O-24 for shared-object outputs
  (`[M4]`).
- O-24's cost in a link of real libraries with tables (none exist before
  M4): measured on generated tables of 4,000, 13,000 and 15,000 rows
  only (`spec/landed/meta/WP-075.md`).
- The hook as a whole: the open problem of O-54 (R1m, R2m, R3m, R6 fail
  on prototype 2), versioned lookup, the quadratic path at K = 10, its
  `secure_getenv` and `write`-only output: WP-077's matrix.
- TU-locality (O-17) on kiln's AST: N31 is shown with `typeid`
  encodings, not with rows; O-17's syntactic test is not prototyped.
- O-33's canonical C type on kiln's types (measured on clang's AST dump
  of glibc headers only; zlib, SQLite and Lua are not in the container);
  GCC's i386 `wchar_t` (`long int`) vs clang's (`int`) at the record level.
- K-3's cost against its 3% criterion (kiln measures it).
- `opt`/`clang` survival with LTO (a named non-goal).
