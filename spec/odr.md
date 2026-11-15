# spec: one-definition-rule checks (`mortar_odr`)

**Status: draft (WP-070; awaiting review).** Rules are `O-n`. The
container they read is `spec/meta.md` (`MT-n`). Every statement about
what a compiler, linker or loader does names its experiment (Appendix A;
scripts in the WP-070 review directory, record in
`spec/landed/meta/WP-070.md`); statements about C++ cite the standard
from knowledge and are marked `[std, unverified clause]` where the clause
number was not checked against the text. `[unverified]` and `[estimate]`
mark the rest. Tags `[M6]` (default) and `[M4]` (shared-object output).

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
(§10) when that is on.

Evidence comes in two strengths:

- **records** (`meta.md` kinds 1-5) written by kiln for each TU: a
  definition hash per entity, layout per type, the declared type of
  every external reference. They make a check a proof;
- **foreign facts** that any object has: symbol sizes, types, bindings
  and COMDAT shape (§8). They catch a few kinds and prove only what they
  see.

**The guarantee (proven part).** Assume

- **A1** every TU that defines or declares entity `E` was compiled by
  kiln with `-fmason-metadata` at the default level or above, with one
  `odr.scheme` (O-11);
- **A2** the objects were changed after compilation only by `ar`,
  `strip`/`objcopy` (binutils or llvm) without symbol renames, and `-r`
  by any linker (`meta.md` §2: these keep the container byte for byte;
  `eu-strip` drops it, which voids A1 for that object);
- **A3** kiln's hasher meets O-13 (its must-equal and must-differ tests);
- **A4** the link runs with `--odr` other than `off`.

Then every violation of kinds K1, K2, K4-K9, K11-K14b, K16a, K16b, K16c
of §2 between two **live** files of a static link, and between the
executable and a mortar-made shared object it is linked against
(O-55), is reported, with O-40's severity (an error once O-46 allows
it, Q1), except that a definition difference in system headers between
units of different `-std` is a warning (O-32). A violation is missed only if two different definitions
have equal 128-bit hashes (O-12; about 2^-128 per pair, no adversary
assumed). K16d and K16e are reported as warnings (O-30, O-31). K17 is
reported only on request (O-23). K18 is reported at load time by the
hook of §10 for exported vague-linkage definitions. K19 (modules) is
kiln's to diagnose (§11, K-6).

**Best effort (no proof).** Objects without records: W1-W3 of §8 catch
K5, K6 and K13 and nothing else measured. Mixed links (one side with
records): O-34.

## 2. The catalogue (measured)

Each case is two TUs `a.cpp` (defines `fa`) and `b.cpp` (defines `fb`)
sharing a header, compiled with g++ 14.2 and clang++ 19.1.7, linked with
GNU ld 2.44, lld 19.1.7 and mold 2.37.1 in the orders `a,b` and `b,a`,
and run (`catalogue`, `catalogue2`; AArch64 host, x86 cases under qemu).
"Weak" is what the foreign checks of §8 see on the same objects
(`weak-catalogue`). "Today" holds for all three linkers and both
compilers unless stated: no linker printed anything for any case.

| K | kind | today (every linker, both orders) | weak checks | record that proves it |
|---|---|---|---|---|
| K1 | inline function, body differs by `-D` | silently the first object's copy for all callers: `fa=fb=1` (`a,b`), `fa=fb=2` (`b,a`) | no (code bytes differ legitimately, B1) | kind 2 hash |
| K2 | inline function with `assert`, `NDEBUG` in one TU | `a,b` runs; `b,a`: the assertion fires in the TU that disabled it, abort (134) | no | kind 2 hash |
| K4 | class with an extra member in one TU | wrong values: `fb=` garbage (`a,b`), `fa=0` (`b,a`) | no: no symbol carries a size | kind 3 layout |
| K5 | class with a different set of virtual functions | ran correctly by luck; `_ZTV1B` 48 vs 40 bytes | **W1** (size), W4 | kind 3 layout (vtable shape) |
| K6 | class with different base classes | clang `a,b`: SIGBUS (135) under all three; g++ ran; `_ZTI1D` 56 vs 24, `_ZTV1D` 72 vs 40 | **W1**, W4 | kind 3 layout |
| K7 | enum with different enumerators | wrong results, order-dependent (`fa=1 fb=0` / `fa=0 fb=1`) | no | kind 3 (enum values) |
| K8 | inline function, default argument differs | `fa=1 fb=2` in every order: each call site uses its TU's default; no visible failure | no | kind 2 hash (O-10: default arguments) |
| K9 | inline variable, initializer differs | the variable takes the first object's value (`11`/`21`) while constant-folded uses keep each TU's value | W4 only (off, §8) | kind 2 hash |
| K11 | lambda in an inline function, body differs | the first object's closure for all callers | no | kind 2 (the enclosing function and the closure's `operator()`) |
| K12 | unnamed namespace used by an inline function in a header | one counter for both TUs (`fa=11 fb=12`): the kept copy's | no (identical bytes) | kind 2 flag `USES_INTERNAL` (O-16) |
| K13 | explicit specialization in one TU, implicit instantiation in another (IF-NDR) | the strong specialization wins for all callers (`fa=fb=2`) | **W3** | kind 2 flags (O-15) |
| K14 | `extern` C++ variable declared with another type | wrong values (`fa=0`) | no: an undefined symbol has no size | kind 5 |
| K14b | `extern "C"` function declared with another signature | wrong results (`fa=2`, expected 6) | no | kind 5 |
| K16a | `-fshort-enums` in one TU | garbage values | no | kind 3 (enum size) + kind 1 `abi.short-enums` |
| K16b | `_GLIBCXX_DEBUG` in one TU (g++ only) | SIGSEGV in every order | no (10 symbols checked, none flagged) | kind 3 (user types containing containers) + kind 1 `abi.stdlib-debug` |
| K16c | `-fno-rtti` in one TU, polymorphic class | SIGSEGV in every order | no | kind 3 (the layout hash covers the RTTI slot) + kind 1 `abi.rtti` |
| K16d | inline function compiled with `-mavx2` in one TU (x86-64) | `a,b`: every linker keeps the AVX2 copy (5 `ymm` instructions in the linked `sum`) for every caller, the baseline TU's included; `b,a`: 0. A CPU without AVX2 would fault [unverified: qemu-user executes AVX2 whatever `-cpu` says] | no | kind 1 `isa` (O-31) |
| K16e | `-freg-struct-return` in one TU (i386) | SIGSEGV under all three | no | kind 1 `abi.struct-return` (O-30) |
| K17 | the same symbol in two archives / an object and an archive | link order picks silently: `liba,libb` 1, `libb,liba` 2, `x.o libb.a` 1 | yes, if unextracted members are read | kind 2, opt-in (O-23); legal by archive semantics |
| K18 | inline function in two shared objects, different bodies | default visibility: the first in lookup order wins for both libraries (`-lA -lB`: 1,1; `-lB -lA`: 2,2); with `-fvisibility-inlines-hidden` each keeps its own (1,2) | no | the loadable table (§10) |
| K18b | copy-relocated array whose size grows in the shared object | glibc at run time: ``Symbol `arr' has different size in shared object, consider re-linking``, then wrong values (`last=0`) | the sizes, at static link time only | kind 5 size vs `st_size` (O-55) |
| K18c | template static data member in a library loaded with `dlopen(RTLD_LOCAL)` | g++ binds it `STB_GNU_UNIQUE`, clang `STB_WEAK`; the library gets its own instance (sees 1) unless the executable is linked `-rdynamic` (sees 2) | — | not a source violation: one entity, two objects (`spec/dyn.md` §14; Q8) |
| K19 | modules: a definition merged from two global module fragments | [not built: kiln does not compare GMF definitions, `kiln/spec/modules.md` §10 item 11] | — | kiln's own check with the same hash (K-6) |

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
| clang modules' ODR hash | not verified: compares definitions merged from modules only | |
| MSVC `/ODR`-style checks, `abidiff` | not verified: not available | |

## 3. What must not be flagged

**O-1 (the bar).** A check that can reject a legal program someone can
construct is off by default or a warning (§9), and the program is in the
legal corpus (§12). Measured where the experiment says so (`legal`):

| N | legal program | why it looks like a violation | what keeps it unflagged |
|---|---|---|---|
| N1 | one inline function at `-O0` and `-O2` | different bytes (g++: 136 vs 36; clang: 140 vs 152) | the hash is of the definition before code generation (O-10); code bytes are never compared |
| N2 | `-g` and no `-g` | (g++ `-O0`: identical bytes, B1) | O-10 excludes debug information |
| N3 | GCC and Clang objects of one source | different bytes; `_ZTS*` and `_ZSt19piecewise_construct` aligned 8 by GCC, 1 by Clang (4 findings); a libstdc++ vtable whose contents differ (D1/D2 destructor aliases, 1 finding) — 5 of 268 checked symbols in a mixed static link | alignment (W5) is not checked; contents (W4) are not checked by default; hashes of different schemes are never compared (O-11) |
| N4 | `-fexceptions` and `-fno-exceptions` TUs mixed | flags differ | `abi.exceptions` is not enforced (`meta.md` MT-26) |
| N5 | a weak default overridden by a strong definition; `malloc`/`free` replaced | two definitions of one name (measured: not flagged) | rows are compared only for vague-linkage definitions and template specializations (O-14); interposing strong definitions are not ODR entities of the program's C++ |
| N6 | symbol versioning: `foo@V1` and `foo@@V2` | one name | the key is the full name with its version (O-2) [not built] |
| N7 | `gnu_inline` / `extern inline` with an out-of-line definition | two definitions (measured: not flagged) | `gnu_inline` definitions are not vague linkage; kiln writes no row for them (K-3) |
| N8 | C tentative definitions with `-fcommon` | several definitions (measured: not flagged) | commons are resolution's (`spec/resolve.md` §3.3) |
| N9 | explicit instantiation declaration + definition, with an implicit copy elsewhere | a strong-looking definition and a COMDAT copy | g++ and clang emit the explicit instantiation `STB_WEAK` in a COMDAT group (measured), so W3 does not fire; kind 2 flags `EXPLICIT_INST` and `IMPLICIT_INST` are compatible (O-15) |
| N10 | implicitly defined special members under different `-O` | different bytes | the hash is of the synthesized definition (O-10) |
| N11 | libstdc++'s own duplicates between `libstdc++.so` and `libstdc++.a`/`libsupc++`, `libgcc.a` and `libgcc_s.so` | same names, foreign objects | no records; W1-W3 found nothing on a 12-TU static link against `libstdc++.a` with 563 extracted members (measured) |
| N12 | a vendored copy renamed with `objcopy --redefine-sym` or a prefix macro | the records still name the old symbol | anchoring (O-4): a row is compared only if its object defines the symbol it names |
| N13 | `static` functions or unnamed-namespace entities with one name in many TUs | one name | internal linkage is never a key (O-2) |
| N14 | the same symbol in an unextracted archive member (K17) | two definitions in the inputs | not part of the program; checked only on request (O-23) |
| N15 | `extern int a[];` in one TU, `int a[4];` defining it | types differ | [basic.link] allows an array of unknown bound (`[std, unverified clause]`): kind 5 types are bound-erased and size 0 means unknown (O-35) |
| N16 | definitions that differ in comments, whitespace, macro names that expand alike, `[[likely]]`, `[[deprecated]]`, `[[nodiscard]]`, `[[maybe_unused]]` | different source text | O-10 hashes the AST after expansion and lists the attributes it ignores |
| N17 | an inline function in a header that reads a namespace-scope `const int` (internal linkage) without odr-using it | refers to an internal entity | [basic.def.odr] allows it (`[std, unverified clause]`); `USES_INTERNAL` is set only on an odr-use (O-16) |
| N18 | `assert` in a header's inline function, the header reached as `h.h` and `./h.h` | `__FILE__` differs (clang's message said `./h.h`, g++'s `h.h`: `catalogue` K2) | O-10: `__FILE__`, `__LINE__`, `__DATE__`, `__TIME__`, `__COUNTER__`, `__builtin_FILE`, `std::source_location` hash as placeholders |
| N19 | `-fPIC` and not, `-ffunction-sections` and not | different code and sections | O-10 |

**O-2 (keys never compared).** Local symbols, internal-linkage entities,
and names of a file's `STB_LOCAL` symbols are never keys.

**O-3 (measuring the bar).** Before any check is on by default, and on
every release after: the false-positive oracle of §12.3 runs and must
report **zero** findings of that check, or each finding is a real
violation recorded in the corpus with its reason. "Zero" is measured, not
argued.

## 4. The entity key

**O-4 (key, anchoring).** An entity's key is the pair (name, class):

- the **name** is the mangled name as it appears in the symbol table,
  including ABI tags (`B5cxx11`), module attachment (`W`, Itanium modules
  mangling as kiln writes it) and a symbol version if the name has one;
  for a type, the Itanium `<type>` encoding of the class or enum (what
  follows `_ZTS`);
- the **class** is function, variable or type: two rows with one name
  and different classes are W2's finding (§8), not a definition
  comparison.

Rows are grouped by the row's `key` (FNV-1a 64 of the name, MT-13) and
grouped again by name bytes, so that a key collision cannot pair
different names. An **anchored** row (kind 2, 5) has no name of its own:
its name is that of a symbol its file defines (or, for kind 5, mentions)
whose FNV-1a 64 is the key; a row whose file has no such symbol is
**unanchored** and is not compared unless `--odr=strict` (it describes a
renamed symbol, N12, or a definition the compiler inlined away). Two
symbols of one file with the same FNV-1a 64 and different names make
the rows ambiguous: they are not compared, and `--odr-stats` counts
them [estimate: never seen; FNV-1a 64 over a file's few thousand names].

Cost: the anchor check needs FNV-1a 64 of every global name of a file
with a unit, once per file: about 1 ns a byte [estimate], 78 KB of names
in the heavy TU, so ~80 µs per such file, parallel per file. With
`--odr=off` nothing is hashed.

## 5. The definition hash

**O-10 (what it covers) [kiln].** The hash of a definition is computed
by kiln over the definition after semantic analysis and template
instantiation (kiln's suggestion, adopted), serialized canonically:

- node kinds, operators, literal values as written (after macro
  expansion), the declarator (return and parameter types, `noexcept`,
  `constexpr`/`consteval`, cv- and ref-qualifiers, storage, the
  attributes that change meaning: `alignas`, `packed`, `no_unique_address`,
  calling-convention attributes, `visibility`);
- the **accumulated default arguments** of the function at the end of
  the TU, and the default arguments used by calls inside the definition
  (K8: [dcl.fct.default] requires them to agree for inline functions,
  `[std, unverified clause]`);
- each referenced entity by its **name**: an external entity by its
  mangled name, a local entity (parameter, variable, label, structured
  binding) by its declaration index in the definition, never by its
  spelling; an internal-linkage entity by its name and the flag of O-16;
- each type by its mangled encoding (the type's own definition is kind
  3's business, not hashed into every user);
- a lambda by its closure type's mangled name and its body; a local class
  likewise;
- implicitly defined members as kiln synthesizes them (N10).

It is **insensitive** to: whitespace, comments, macro names, `-O`, `-g`,
`-fPIC`, `-ffunction-sections`, the order of unrelated declarations,
source paths and line numbers, the expansions of `__FILE__`, `__LINE__`,
`__DATE__`, `__TIME__`, `__COUNTER__`, `__builtin_FILE`/`_LINE`/
`_FUNCTION`/`_COLUMN` and `std::source_location::current` (each hashes as
its own placeholder), and the attributes `likely`, `unlikely`,
`deprecated`, `nodiscard`, `maybe_unused`, `fallthrough`,
`no_sanitize*`, `gnu::used`, `gnu::cold`, `gnu::hot` (they change
neither meaning nor layout).

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

**O-13 (hash tests) [kiln].** kiln keeps two corpora: **must-equal** (N1,
N2, N10, N16, N18, N19 and the same definition reached through different
include paths, macro spellings, `-I` orders, `-ffile-prefix-map`) and
**must-differ** (K1, K2, K8, K9, K11, K12, K13, a changed
`alignas`/`packed`, a changed default member initializer). A must-equal
failure is a false positive in every user's link: it blocks release.

**O-14 (which definitions have rows).** Kind 2 rows exist for
definitions that may legally appear in more than one TU, and for those
that clash with them: inline functions and variables, template
instantiations (implicit or explicit), static data members of class
templates, explicit specializations, and the out-of-line members of
class templates. Not for ordinary strong functions and variables (two of
them are already `duplicate-symbol`, `spec/resolve.md` §3.8; their
declarations are kind 5's), not for `gnu_inline` (N7), not for weak
definitions that are not vague linkage (N5).

**O-15 (flags).** A kind 2 row is

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
not visible, [temp.expl.spec] `[std, unverified clause]`); `EXPLICIT_INST`
and `IMPLICIT_INST` are compatible (N9).

**O-16 (`USES_INTERNAL`) [kiln].** Set when the definition odr-uses an
entity with internal linkage that is not one of [basic.def.odr]'s
exceptions (a `const` object of literal type whose value is used and
whose address is not; `[std, unverified clause]`). Two rows of one key
with `USES_INTERNAL` from different units refer to different entities
even with equal hashes: reported as `odr-internal-reference`, a
**warning** by default (K12; the exception analysis is kiln's and its
false-positive rate is not measured: Q5).

## 6. Layout records

**O-17 (kind 3 row).** A second, independent line of evidence: one row
per class, struct, union or enumeration with external linkage whose
definition the TU contains and that an emitted entity's definition or
declarator reaches (kiln's suggestion: types of members, bases,
parameters, returns, locals and `new`/`sizeof` operands of emitted
definitions, transitively through members and bases).

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
name of the final overrider's declaration (not of its body), and whether
the RTTI slot holds a type_info pointer (K16c); for an enumeration, the
underlying type and the (name, value) list. From kiln's `RecordLayout`
(`kiln_ast/src/types/layout.rs`) and `VTableLayout`
(`kiln_ast/src/class.rs`).

**O-19 (kind 4, layout details).** Optional (`-fmason-metadata=detail`):
per type, the list O-18 hashes, in readable form (member names, offsets,
type strings), so that a diagnostic can say *what* differs. Mortar reads
kind 4 only to render a diagnostic that kind 3 already decided. Its exact
encoding is fixed by WP-073 with kiln (a list of `{u32 name; u32 type;
u64 bit_offset; u32 width; u32 flags}` entries is the proposal).

## 7. Comparison points

**O-20 (what is compared).** The rows of every unit of every live file
(`meta.md` MT-21), whatever the fate of the sections they describe.
Grouped by key and name (O-4), each group is partitioned by hash (and,
for types, by layout); more than one class is a finding.

**O-21 (the COMDAT keeper).** For a vague-linkage entity the copies are
`comdat_copies` (`spec/resolve.md` §6.7: `(ComdatId, FileId)` pairs, the
keeper first). The diagnostic names the keeper's file as "kept" so that
the user knows which body runs. Cost: the check is driven by the groups
with two or more live copies: for each, a binary search of each copy's
kind 2 block (sorted, MT-11): O(copies × log rows) [estimate: a link of
1,000 heavy TUs has ~600,000 copies; ~10 probes each, ~60 ms of one
core, ~6 ms on 12; 29 MB of rows touched].

**O-22 (types and declarations).** Kind 3 and kind 5 rows have no group
to drive them: every row of every live unit is read and grouped by key
(a parallel hash-partition by key, merged in `FileId` order).
[estimate: 334 type rows and 661 declaration rows a TU; 1,000 TUs: 1
million rows, 45 MB, ~50 ns a row → ~50 ms of one core.] This is the
check's largest cost and what `--odr=off` saves; WP-072 measures it on
the bench lane (§12.4) against the 3 % budget of O-45.

**O-23 (archives not extracted) — opt-in.** With `--odr-check-archives`,
the units of archive members that were **not** extracted are read too, and
a member whose kind 2 row disagrees with a live definition of the same
key is reported as the warning `odr-archive-shadow` (K17). Off by
default: such a member is not part of the program (N14), and reading it
costs a parse of every member's section headers [estimate: linear in
archive size; for a 100 MB archive, ~20 ms].

**O-24 (shared objects).** §10.5.

## 8. Checks without records

### 8.1 The foreign checks (W)

These read only what resolution already holds: for each live COMDAT
group with two or more copies, the defined symbols of each copy. Rules
and what they measured (`weak-catalogue`, `legal`):

| rule | finding | catches (measured) | false positives measured | default |
|---|---|---|---|---|
| W1 | a data symbol (`STT_OBJECT`/`STT_TLS`) defined in two copies of a group with different `st_size` | K5 (`_ZTV1B` 48/40), K6 (`_ZTI1D` 56/24, `_ZTV1D` 72/40) | 0 of 268 symbols in the mixed GCC/Clang static link | warning `odr-size-mismatch` |
| W2 | one key defined as a function in one live file and as data in another | (none in the catalogue) | 0 | warning `odr-symbol-type-mismatch` |
| W3 | a strong non-COMDAT definition and a COMDAT copy of the same key | K13 | 0; explicit instantiations are weak in COMDAT (N9) | warning `odr-strong-and-comdat` |
| W4 | data contents (bytes and symbol-normalized relocations) differ between copies | K5, K6, K9 | **1**: a libstdc++ vtable (D1/D2 aliases differ between GCC and Clang) | not a check: `--odr=strict` only, as a warning |
| W5 | section alignment differs | — | **4**: `_ZTS*` 8 (GCC) vs 1 (Clang) | not a check |
| F | function code bytes differ | K1, K2, K11 … | everything at different `-O` (B1) | never |

W1 and W3 are warnings although they found no false positive, because
the corpus that measured it is small (O-1); they become errors under
`--odr=strict` (O-41) and are proposed as errors once the
lanes of §12.3 have run them at zero (Q12). Cost: one pass over the
copies resolution keeps, comparing two integers per symbol [estimate:
under 1 ms per 100,000 copies; the Python prototype checked 268 objects
in 82 ms].

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
a declaration or between two declarations. That is
`odr-declaration-mismatch`, an **error** (K14, K14b; [basic.link] "the
types ... shall be identical", `[std, unverified clause]`), except that
a difference only in `NOEXCEPT` is a warning [unverified: whether the
two compilers mangle and treat it alike in every mode].

### 8.3 Mixed links

**O-34.** When one side of a comparison has records and the other has
not: a kind 5 row with a size is compared with the foreign definition's
`st_size` (`odr-size-mismatch`, warning: the foreign definition's
declared type is unknown); kind 3 is compared with nothing (foreign
objects carry no layout without DWARF, Q11); kind 2 with nothing. The
W checks run regardless of records.

## 9. Diagnostics and switches

### 9.1 Kinds

Proposed for `spec/diag.md` §3.1 (amendment A-4), a new area
"one-definition rule" between "symbols" and "relocations". Messages follow
the wording policy (no reference linker has these conditions; GCC's
`-Wodr` text is the model where it fits):

| id | variant | message | default |
|---|---|---|---|
| `odr-abi-mismatch` | `OdrAbiMismatch { option, first, second }` | `translation units compiled with different {option}: {first} and {second}` | warning |
| `odr-archive-shadow` | `OdrArchiveShadow { name }` | `{name} in an archive member that is not linked is defined differently` | warning (opt-in) |
| `odr-declaration-mismatch` | `OdrDeclarationMismatch { name }` | `{name} is declared with a type that does not match its definition` | error |
| `odr-internal-reference` | `OdrInternalReference { name }` | `{name} refers to an entity with internal linkage, which differs in each translation unit` | warning |
| `odr-isa-mismatch` | `OdrIsaMismatch { name }` | `the kept copy of {name} was compiled for other target features than a translation unit that uses it` | warning |
| `odr-size-mismatch` | `OdrSizeMismatch { name }` | `{name} has different sizes in different files` | warning |
| `odr-strong-and-comdat` | `OdrStrongAndComdat { name }` | `{name} has a non-inline definition and an inline copy` | warning |
| `odr-symbol-type-mismatch` | `OdrSymbolTypeMismatch { name }` | `{name} is a function in one file and data in another` | warning |
| `odr-type-mismatch` | `OdrTypeMismatch { name }` | `type {name} violates the one-definition rule` | error (layout differs); warning (only the definition hash differs) |
| `odr-violation` | `OdrViolation { name }` | `{name} violates the one-definition rule` | error |
| `meta-malformed` | `MetaMalformed { reason }` | `malformed Mason metadata: {reason}` | warning |
| `meta-version` | `MetaVersion { major }` | `Mason metadata version {major} is not supported; its records are ignored` | warning |

Every `{name}` goes through the demangling hook (`spec/diag.md` §5.4).
Context lines (`spec/diag.md` §2.4), e.g. for K1 linked `a.o b.o`:

```
error: f(int) violates the one-definition rule
>>> defined in a.o (kept), at h.h:3
>>> defined differently in b.o, at h.h:3
>>> the definitions differ; a.o and b.o were compiled with different -D options? (compare their command lines)
```

- One diagnostic per entity: the copies are partitioned by hash; the
  first class (the keeper's) is "defined in", each other class's first
  file "defined differently in", with at most 3 files a class and a count
  of the rest.
- Notes that explain, from kind 1: every `ENFORCE` key and `std` that
  differ between the two units are listed (`>>> a.o: abi.short-enums=on;
  b.o: off`). The question in the K1 example is printed only when no
  kind 1 key differs.
- Types: with kind 4, the first difference (`>>> member 'y' at offset 8
  in a.o only`); without, the sizes and alignments.
- Key (`spec/model.md` §8): phase 40, unit = the `FileId` of the first
  file of the second class, seq = its symbol-table index; for a row
  without a symbol (types, unanchored rows) `2^40 + ` the row's index in
  its unit (amendment A-2).

### 9.2 Severity and switches

**O-40 (defaults).** Record-based findings whose rows are anchored, of
one scheme, without `PARTIAL`: `odr-violation`, `odr-type-mismatch`
(layout) and `odr-declaration-mismatch` are **errors**; everything else
is a warning, as the table says. `--fatal-warnings` and `-w` act as on
every warning (`spec/diag.md` §6.3).

**O-41 (switches)** (amendment A-5 to `spec/cli.md`):

- `--odr=error` (default): O-40.
- `--odr=warn`: every finding is a warning.
- `--odr=strict`: O-40, plus heuristic findings (W1-W3,
  `odr-abi-mismatch`, `odr-internal-reference`, `odr-isa-mismatch`,
  definition-only type mismatches) as errors, plus unanchored, `PARTIAL`
  and `INLINED_ONLY` rows and W4 as warnings, plus `meta-malformed` and
  `meta-version` as errors.
- `--odr=off`, also spelled **`--no-odr-check`**: no check runs, no
  container is read (MT-42), the W checks do not run, and no loadable
  table is written (§10). What a link pays then is the classification
  of one more section name per Mason object (a byte comparison it does
  for every section anyway): **nothing measurable**. The output is
  byte-identical to the default link's when that link succeeds and the
  table is off (T-O-41).
- `--odr-check-archives` (O-23), `--odr-table=off|exported|full` (O-50),
  `--odr-stats` (O-44).
- GNU gold's `--detect-odr-violations`: accepted as `--odr=warn`
  (gold-only spelling; [unverified: gold's semantics compare DWARF line
  numbers, not definitions]).

**O-44 (`--odr-stats`).** One line to standard error: units read, rows by
kind, schemes seen and the unit pairs not compared because of them,
ambiguous anchors (O-4), unanchored rows, time spent. For tuning and for
the oracle of §12.3.

**O-45 (cost budget).** With the default `--odr=error`, the checks of
§7-§8 cost at most 3 % of the link's wall time on the bench lane's
Mason workloads (`spec/harness.md` "bench", amendment A-6), measured by
WP-072 and WP-073; above it, the check is a performance bug. The
operator's frame (2026-11-15): diagnostics may cost what mold does not
pay, and `--no-odr-check` must make mortar competitive with mold:
O-41's "nothing measurable" is that clause.

**O-46 (turning a check on).** A check moves from warning to error by
default, or from off to on, only after O-3's oracle reports zero on it,
and the move is recorded in the landed record of the package that makes
it.

### 9.3 ABI and ISA summaries

**O-30 (`ENFORCE` keys).** Two live units whose kind 1 rows for the same
name both carry `ENFORCE` with different values: `odr-abi-mismatch`, one
diagnostic per (name, value pair), naming the first unit of each value.
A **warning**: mixing is legal when no affected type or call crosses
the two (an `-fno-rtti` library linked with RTTI code is common), so the
mismatch alone proves nothing. Where it does cross, kind 3 sees it (K16a
enum size, K16c RTTI slot) and the error names the key in a note. K16e
(i386 struct return) has no kind 3 witness: it stays a warning
(`--odr=strict` makes it an error).

**O-31 (`isa`).** For a COMDAT group whose copies come from units with
different `isa` strings, when the keeper's `isa` differs from that of a
unit with another copy: `odr-isa-mismatch` (K16d). Mortar does not order
feature sets (strings are opaque); it reports the difference. Q7 asks
whether mortar should instead keep the copy of the least-featured unit.

**O-32 (`-std`).** When a kind 2 or kind 3 difference is between units
whose `std` (kind 1, a non-enforced key kiln writes: `c++17`,
`c++20`, …) differs and whose rows carry `SYSTEM`, the finding is a
warning with the note `>>> the translation units use different -std`:
library headers legitimately specialize bodies on `__cplusplus`, and
mixing standards is supported practice. Q6.

## 10. The dynamic side

### 10.1 What the table holds

**O-50 (which rows).** With `--odr-table=exported` (the proposed default
when any live unit has kind 2 rows and the output is dynamic; Q2), a
mortar-made executable `[M6]` or shared object `[M4]` carries one row
per definition it **exports** (in `.dynsym`, `STV_DEFAULT` or
`STV_PROTECTED`, defined) that has a kind 2 row with `VAGUE` or
`EXPLICIT_SPEC`. With `=full`, also its hidden vague definitions and its
kind 3 types (strict use only: two libraries with hidden copies never
bind each other, K18's second row). With `=off`, no table and no
runtime.

**O-51 (row).** `{u64 key; u64 hash; u32 dynsym; u32 flags}` (24 bytes),
sorted by key: the key of MT-13, the first 8 bytes of the kind 2 hash
(O-12), the `.dynsym` index of the symbol (the name, for the message,
comes from `.dynsym`/`.dynstr`, so the table carries no strings), the
flags of O-15's low byte. Ties (several units with one key, equal hashes
by then: a static violation would have failed the link) give one row.
Header `{u32 version = 1; u32 count; u32 row_size = 24; u32 reserved}`.

Size: 24 bytes per exported vague definition. Measured counts of
exported weak or unique definitions in system libraries (`nm -D`):
`libstdc++.so.6` 3,991, `libclang-cpp.so.19.1` 12,929,
`libLLVM.so.19.1` 15,129: 96 KB, 310 KB and 363 KB of table, in pages
that are read once at load and never written.

### 10.2 Section and segment

**O-52 (encoding).** An allocated `SHT_NOTE` section `.note.mason`,
`sh_addralign` 8, holding one note: owner `MasonMeta` (`n_namesz` 10),
`n_type` 1 (`NT_MASON_ODR`), descriptor = O-51's header and rows. It is
in a `PT_NOTE` of its own with `p_align` 8 (the gABI's rule for notes
of 8-byte alignment, as GNU ld does for `.note.gnu.property`); with
`n_namesz` 10 the descriptor starts at offset 24 under both the 4-byte
and the 8-byte note layouts, so readers of either kind agree. The
prototype used a 4-aligned note with owner `Mason`, which `gcc` put in
the same `PT_NOTE` as `.note.gnu.build-id` (`dynamic`): the reason for a
segment of its own.

Why a note, measured against the alternatives (`dynamic`; the same table
in a `PT_NOTE`, in a `PT_LOOS`-range segment `0x6d736e00`, and found
through a `DT_` tag `0x6d736e00`; executable and shared object):

| | note `PT_NOTE` | segment `PT_LOOS+…` | `DT_` tag |
|---|---|---|---|
| `ld.so` runs it | yes | yes | yes |
| `strip`, `llvm-strip` 19, `eu-strip` | kept, program runs | kept, runs | kept (tag present), runs |
| gdb 16.3 backtrace | 2 frames, normal | normal | normal |
| lldb 19 backtrace | normal | normal | normal |
| `eu-elflint` | error: unknown note type with owner `Mason` | error: unknown program header type | (the measured file also had the note; the tag alone is not isolated) |
| readelf | `readelf -n` prints it | `LOOS+0xd736e00` | `Operating System specific: 6d736e00` |
| found by the runtime | `dl_iterate_phdr`, standard type | `dl_iterate_phdr`, private type | needs the dynamic section; absent in static executables |
| patchelf | not verified (not installed) | | |

All three are tolerated by everything that runs or debugs; none is clean
under `eu-elflint`. The note wins on discovery (a standard segment type
any tool lists), on inspection (`readelf -n`), and it is what dyn.md §14
anticipated. The brief's "custom ELF segments" is satisfied by a
`PT_NOTE` segment that is Mason's alone; `PT_LOOS+0xd736e00`
(`PT_MASON`) stays unused, recorded here so that nobody takes it, and
would be used only if a later table must be found without walking notes.

### 10.3 The load-time hook

**O-53 (the runtime).** A small object, `mason_rt.o`, one per target,
shipped with mortar (`<prefix>/lib/mortar/<triple>/mason_rt.o`) and
added by mortar as an ordinary input — like a start file, first among
the inputs so that its array entry runs first in its module — whenever
the output gets a table. Recommended in C (about 150 lines; the
prototype `hook/mason_rt.c` is 97), built per target by the Mason build
with the cross compilers the harness already uses; the alternative is
`no_std` Rust, which needs `unsafe` to read loader memory (Q3, the
operator decides anything `unsafe`). mortar itself stays
`forbid(unsafe_code)`: the runtime is a file it links, not code it runs.
Synthesizing machine code in mortar per architecture was rejected: three
hand-written encoders that the verifier could not judge as code.

**O-54 (behavior).**

- **Executable:** an entry in `.preinit_array` runs the check over every
  module loaded at start-up, **before any constructor** — measured: the
  executable's `.preinit_array` runs before its libraries'
  constructors, PIE and non-PIE (`dynamic`). It merges all tables at
  once (a k-way merge by key over the modules found by
  `dl_iterate_phdr`).
- **Shared object `[M4]`:** an entry in `.init_array` compares its own
  table with each module listed before it by `dl_iterate_phdr`, so a
  library loaded by `dlopen` is checked against everything loaded
  before it, and every pair is compared once whatever the constructor
  order (the prototype's rule). At start-up under a Mason executable,
  this repeats the preinit check's pairs: at most twice the cost below.
- Modules without a table (foreign) are skipped; a table with an
  unknown version is skipped.
- A key that two modules carry with different hashes, once the names
  read from both `.dynsym`s are equal: the runtime writes to file
  descriptor 2 (with `write`, no allocation, no stdio) `mason:
  one-definition-rule violation: <name> differs between <module> and
  <module>` per key, then calls `abort()`. `MASON_ODR=warn` writes and
  continues; `MASON_ODR=off` skips the check (read with
  `secure_getenv`, so a set-user-ID program always checks and aborts).
- No allocation, no global state, no lock: `dl_iterate_phdr` holds the
  loader's lock while it walks; nothing persists after the constructor,
  so `dlclose`, `fork` and threads need nothing.
- **Static executables** (and static PIE) get no table and no runtime:
  the static check saw every definition (MT-33). A static executable
  that `dlopen`s is not covered (recorded, not refused).

Measured on the prototype (aarch64, glibc 2.41; four modules, the
executable and three libraries, each with `n` shared keys and `n/4`
private ones; in-process time of all four constructors, median of 50
runs; `dynamic`):

| rows per module | table bytes per module | check time | process start with hook / without (wall, noisy) |
|---|---|---|---|
| 1,250 | 20 KB | **34 µs** (37 µs in a second run) | 524 / 704 µs |
| 12,500 | 200 KB | **321 µs** (381) | 992 / 607 µs |
| 125,000 | 2 MB | **3,880 µs** (4,485) | 4,337 / 631 µs |

About 2.3 ns per row visited, linear. A clang-sized process
(`libLLVM` 15,129 + `libclang-cpp` 12,929 + `libstdc++` 3,991 + an
executable) is ~35,000 rows: ~0.3 ms with the pairwise prototype
[estimate], less with O-54's k-way merge. Detection, measured: a
violating library linked at start-up aborts with the message (exit
134); `MASON_ODR=warn` runs to exit 0; a violating library loaded with
`dlopen` aborts in its constructor.

**O-55 (what the static link proves).** At static link time mortar
compares the executable's kind 2 rows with the table of every
mortar-made shared object it links against (O-52, read from the `.so`
input): a difference is `odr-violation` with the library as the other
place, an error, before anything runs. Kind 5 sizes of variables the
executable references are compared with the `.so`'s `st_size`
(`odr-size-mismatch`, a warning; K18b's copy relocation would truncate).
What only load time can see: a different build of the library at run
time (K18b's v2), libraries loaded by `dlopen`, and libraries the link
never named.

**O-56 (compatibility).** A mortar-made executable with foreign
libraries: they have no table and are not checked. A mortar-made shared
object in a foreign executable: its constructor checks it against
whatever tables are loaded. Intentional interposition of a vague
definition (`LD_PRELOAD` of a library that redefines an inline function
differently) aborts: `MASON_ODR=warn` is the escape; interposing strong
definitions (`malloc`) is not in any table (N5).

**O-57 (the D-21 boundary).** K18c shows that a library loaded with
`RTLD_LOCAL` gets its own instance of a vague definition the executable
does not export (`spec/dyn.md` §14). The table does not change that; Q8
asks whether an M6 export class should.

## 11. Requests to kiln and hod

Ready to send; the supervisor relays them. Estimates are the designer's,
for the receiving agent to correct.

### kiln

- **K-1 — the flag and the emission.** `-fmason-metadata[=min|default|detail|all]`
  (`min`: kinds 1, 2; `default`: 1, 2, 3, 5; `detail`: + 4; `all`: +
  `INLINED_ONLY` rows), and `-fno-mason-metadata`. The unit is one IR
  global per module: `@__mason_meta = private constant [N x i8] c"…",
  section ".mason.meta", align 8, !exclude !0`, listed in
  `@llvm.compiler.used`: the form llc 19/22 already lower to `meta.md`
  MT-1 (measured), so kiln's opt+llc route works today and its hod route
  as soon as H-1 lands. Where: at the end of module emission, after the
  `kiln_llvm::Module` is final (`kiln_codegen/src/module.rs`, `Cg`): the
  final set of globals with linkage and comdat is the list of emitted
  definitions. A writer of `meta.md` §4 (~300 lines). *Why:* nothing
  else reaches mortar. *Estimate:* 2-3 days.
- **K-2 — kind 1.** The keys of MT-26 from the driver's options and the
  target. *Estimate:* 1 day.
- **K-3 — the definition hasher (kind 2).** A pass over the sema AST
  after instantiation, keyed by mangled name, computed for emitted
  definitions (O-14), serializing O-10 canonically, plus O-15's flags
  and O-16's odr-use analysis. Traps (kiln's: default arguments,
  lambdas in inline functions, attributes; and these): local entities by
  index not spelling; `__FILE__` & co. as placeholders (N18);
  `__COUNTER__`; implicitly defined members synthesized before hashing;
  constant-evaluated initializers hashed as expressions, not values (K9:
  `inline int v = N;` must differ when `N` does); explicit specialization
  vs implicit instantiation flags; `gnu_inline` excluded; deterministic
  (no pointer values, no hash-map order). The two corpora of O-13 as
  kiln tests. *Estimate:* 2-3 weeks with the corpora.
- **K-4 — kinds 3 and 4.** From `RecordLayout` and `VTableLayout`
  (O-17-O-19), enums included; the reachability walk of O-17.
  *Estimate:* 1 week.
- **K-5 — kind 5.** Defined and referenced external functions and
  variables, type encodings from `kiln_abi`'s mangler, bound-erased.
  *Estimate:* 3-4 days.
- **K-6 — modules.** Use the K-3 hash for the comparison
  `kiln/spec/modules.md` §10 item 11 skips today (definitions merged from
  global module fragments, WP-460; the planned "ODR key / structural
  hash", follow-up S5, WP-1067): one hash, two consumers. Rows of GMF
  definitions carry flag `GMF`. *Estimate:* kiln's.
- **K-7 — default.** On in the dev profile when kiln has one, off
  otherwise until O-3's oracle is green.
- **K-8 — corpora.** kiln's must-equal/must-differ tests (O-13) emit
  objects that mortar's L11 lane links (§12).

### hod

- **H-1 — `!exclude`.** Lower `!exclude` on a global to `SHF_EXCLUDE`
  (today: `unsupported construct 'global-metadata:exclude'`), at llc
  parity: a private constant in a named section `.mason.meta`, align 8,
  `SHT_PROGBITS`, flags exactly `SHF_EXCLUDE`, not in a group, no
  relocations, kept because it is in `@llvm.compiler.used`. hod's object
  API already has `SectionKind::Elf{…}` with `SHF_EXCLUDE`. Testable
  against `llc -O0` by hod's irdiff oracle. *Estimate:* 1-2 days.
- **H-2 — opacity.** hod never interprets the bytes; the section's
  contents equal the IR constant's bytes (a test).
- Nothing else: per-section content hashes are not needed
  (`spec/incr.md` IN-22); DWARF is unchanged.

### Landing order

Each step is useful and testable alone:

1. **mortar** WP-071 (container reader, test producer) and WP-072 (W1-W3,
   switches): no other project needed.
2. **hod** H-1: useful to any IR producer; tested by irdiff.
3. **kiln** K-1 + K-2: units with kind 1 only; mortar reports
   `odr-abi-mismatch` (WP-073 reads them).
4. **kiln** K-3, then K-5, then K-4: each kind turns on its mortar check
   (WP-073 was written against the test producer, so only the lane
   changes).
5. **mortar** WP-075 (table, runtime, hook) after M2's dynamic
   executable; `.so` rows with M4.
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
time; at load time (start-up and `dlopen`). Per producer: kiln (records:
the expected diagnostic, by id and places) and, before kiln emits, the
test producer of `meta.md` §12 writing the rows kiln would; GCC and
Clang (no records: the W outcome of §8.1, which is "not detected" for
most kinds, recorded as such so that a change is visible). Link tests
use the `#@` format (`spec/harness.md` "Link tests") with a new
directive `#@ diag: <id> <file>...` (amendment A-6).

### 12.2 The legal corpus

N1-N19 of §3 as link tests that must link with no diagnostic at
`--odr=strict` except where §3 says a warning is the design (none
today). The mixed GCC/Clang static link against `libstdc++.a`
(`legal`) is one of them.

### 12.3 The false-positive oracle

- **libc++'s test suite**, built by kiln with `-fmason-metadata=all`
  against a libc++ built the same way, every test linked by mortar with
  `--odr=strict --odr-stats`: zero findings (O-3). Its `-std` matrix is
  the evidence for O-32.
- **The open-source lanes** (`docs/VALIDATION.md`): built with GCC and
  Clang (the W checks; and once per lane with GCC and Clang alternating
  per TU, which is where N3 lives), and with kiln where kiln builds them.
- A finding is either a real violation (added to the corpus with its
  reason and an allowlist entry for that lane) or a false positive (the
  check is fixed or demoted, and the program joins §12.2).

### 12.4 Mutation tests and cost

Mutations of kiln's objects and of test-producer units, each with its
expected outcome: one hash byte flipped → exactly one `odr-violation`;
two rows' hashes swapped → two; a unit truncated → one `meta-malformed`,
the neighbours read; rows unsorted → `meta-malformed`; the scheme
renamed in one unit → no comparison, counted by `--odr-stats`; a symbol
renamed by `objcopy --redefine-sym` → no report (N12); the container
removed by `eu-strip` → no report and no warning; the rows of an output's
`.note.mason` reordered → the verifier's failure (12.5). The cost of
O-45 is measured by the bench lane on Mason workloads, `--odr=error`
against `--odr=off`.

### 12.5 The verifier

`mortar_plumb` depends on `mortar_elf` and `mortar_diag` only, so it
re-reads `meta.md`'s format itself (about 150 lines: the format is
small on purpose, and a second reader is the point). Proposed rules
(amendment A-7 to `spec/plumb.md`):

- an executable or shared object contains no `.mason.meta` unless
  `--keep-mason-meta` (MT-31, MT-32); a relocatable output's
  `.mason.meta` is the concatenation of the live inputs' units in input
  order (MT-30);
- a `.note.mason`, if present, is in a `PT_NOTE` of its own with
  `p_align` 8, its rows sorted, each row's key the FNV-1a 64 of its
  `.dynsym` name, each row's hash the first 8 bytes of a kind 2 row of a
  live input defining that name, and the row set equal to O-50's;
- with a table, a `.preinit_array` (executable) or `.init_array`
  (shared object) entry points into the runtime object's input section.

Whether `mortar_plumb` should also re-run the comparison (a second
implementation of the ODR verdict) is Q11.

## 13. Work packages

| WP | what | crates / files | depends on | tests alone against | estimate | parallel with |
|---|---|---|---|---|---|---|
| WP-071 | container reader and writer; MT-6 classification; `meta-malformed`/`meta-version`; the test producer; `--odr=off` path (MT-42) | `mortar_meta`; `mortar_resolve` (one row of §10); `xtask` (producer) | A-1, A-2 applied | `.s` units assembled by GNU as | 3-4 days [estimate] | WP-072 |
| WP-072 | W1-W3; the diagnostic kinds; `--odr=`, `--no-odr-check`, `--odr-stats`; cost measurement | `mortar_odr`, `mortar_diag`, `mortar_cli` | A-4, A-5; `comdat_copies` (M1) | GCC/Clang objects of the catalogue and the legal corpus | 4-5 days | WP-071 |
| WP-073 | kinds 1, 2, 3, 5 checks (O-4-O-35); notes from kind 1; L11 static corpus | `mortar_odr`, `tests/link/odr/` | WP-071, WP-072 | the test producer (no kiln) | 1.5-2 weeks | kiln K-1..K-5 |
| WP-074 | the Mason L11 lane and the false-positive oracle (§12.3) | `xtask`, `tests/mason/` | WP-073; kiln K-1-K-5; hod H-1 | kiln objects | 1 week | WP-075 |
| WP-075 | loadable table (O-50-O-52), the runtime `mason_rt` (C, per target), O-54, O-55 for executables | `mortar_odr`, `mortar_synth`, `mortar_layout`, `runtime/mason_rt/` | WP-073; M2's dynamic executable (WP-040's packages); A-3 | test-producer units; the prototype's measurements | 1.5 weeks | WP-074 |
| WP-076 `[M4]` | shared-object rows: `.init_array` entry, O-55 for `.so` inputs made by mortar | as WP-075 | WP-075; M4 `-shared` | | 1 week | |
| WP-077 | opt-ins: `--odr-check-archives`, `--odr-table=full`, `--keep-mason-meta`, `-r` pass-through (MT-30, with M4) | `mortar_odr`, `mortar_meta` | WP-073 | | 4 days | WP-075 |
| WP-078 | verifier rules (§12.5) | `mortar_plumb` | A-7; WP-071 (format only) | the outputs of WP-073/075 | 3-4 days | WP-075 |
| WP-079 | apply the amendments of §14 | specs | this draft accepted | — | 1 day | first |

WP-072 needs no other project and can start the day this draft is
accepted; WP-071 likewise. M6's "done when" (`docs/ROADMAP.md`) is
WP-074's oracle green with WP-075 landed.

## 14. Requests to other specs (ranked: cheapest now, dearest later)

1. **A-1 `spec/resolve.md` §10** — a row before the `SHF_EXCLUDE` row:
   "| name `.mason.meta` with `SHF_EXCLUDE` (`spec/meta.md` MT-1) | `Meta` |
   `Dropped(Consumed)`; `mortar_meta` reads it when `--odr` is not `off` |
   GNU ld, lld, mold drop it (`meta.md` §2) |". And §6.7: "`mortar_odr`
   (`spec/odr.md` O-21) reads `comdat_copies`; the symbol sets of the
   copies are W1-W3's input." And §13: "a file's units are part of its
   summary (I-file)". *Why first:* the classification table is in the
   hot path of M1/M2 code; one row now, a re-classification later.
2. **A-2 `spec/model.md`** — `SectionKind::Meta`; per file, the list of
   its `Meta` sections (so that `mortar_odr` finds units without a
   rescan); in §8, phase 40's `seq` for row diagnostics without a symbol:
   "`2^40 +` the row's index in its unit". `comdat_copies` stays as it is.
3. **A-3 `spec/dyn.md`** — §14 "M6: ODR data" becomes: "`.note.mason`
   (`spec/odr.md` O-52) is an allocated note of alignment 8 in a
   `PT_NOTE` of its own, placed with the other notes (layout L-60 rank
   2); the runtime object is an input mortar adds (O-53) whose
   `.preinit_array` entry (executables) or `.init_array` entry (shared
   objects) is ordinary". D-21 unchanged at M2; Q8's class, if accepted,
   is added there by amendment. *Why now:* M2 is fixing note placement
   and array ordering.
4. **A-5 `spec/cli.md`** — rows `--odr=error|warn|strict|off`,
   `--no-odr-check` (= `--odr=off`), `--odr-check-archives`,
   `--odr-table=off|exported|full`, `--odr-stats`, `--keep-mason-meta`
   (mortar spellings, no GNU flag); `--detect-odr-violations` (gold)
   accepted as `--odr=warn`; §3.9 gains `odr` in `plan.diagnostics`.
5. **A-4 `spec/diag.md`** — §3.1's area "one-definition rule" with the
   rows of §9.1; §3.2's notes on them.
6. **A-6 `spec/harness.md`** — "Link tests": the directive `#@ diag:`;
   "bench": the Mason workloads and the `--odr=off` comparison (O-45);
   the L11 lane and the oracle of §12.3.
7. **A-7 `spec/plumb.md`** — the rules of §12.5 (a stage after M2's).

## 15. Open questions (each with a recommendation)

- **Q1. Errors by default?** Record-based findings reject programs that
  link today. *Recommend:* warnings until O-3's oracle is green on libc++
  and the lanes, then errors (O-40); the operator decides.
- **Q2. The loadable table and hook on by default** for dynamic outputs
  with records? It aborts programs at load time. *Recommend:* on in the
  dev profile (`MASON_ODR=warn` as the escape), off otherwise; operator.
- **Q3. The runtime's language.** *Recommend:* C (no `unsafe` enters the
  Rust tree; ~150 lines; built per target with the harness's cross
  compilers); the alternative, `no_std` Rust, needs `unsafe` — operator.
- **Q4. Anchored rows vs own strings.** *Recommend:* anchored (MT-12):
  3-6 % of an object instead of ~20 % (`size`); the cost is that a row
  whose symbol was renamed or inlined away is not compared by default.
- **Q5. `odr-internal-reference` and definition-only type mismatches as
  warnings.** *Recommend:* yes until measured at zero.
- **Q6. `-std` mixing (O-32).** *Recommend:* warning for `SYSTEM` rows
  only; user code differing by `-std` stays an error.
- **Q7. An ISA-safe COMDAT keeper** (keep the copy of the unit with the
  smallest feature set). It changes `spec/resolve.md` §6.1's keeper
  (still deterministic). *Recommend:* not now; warn (O-31) and revisit
  with a measured need.
- **Q8. Export vague and unique definitions from executables** so that a
  `dlopen`ed library shares them (K18c; `spec/dyn.md` D-21). *Recommend:*
  not by default (it changes behavior relative to every other linker);
  an option later.
- **Q9. Unextracted archive members.** *Recommend:* opt-in only (O-23).
- **Q10. The hash function.** *Recommend:* kiln's choice under a scheme
  name; FNV-1a 128 (already in kiln) or SipHash-1-3-128.
- **Q11. A second implementation of the verdict in `mortar_plumb`.**
  *Recommend:* yes for kind 2 at L11 only (cheap: compare hashes per
  key), not in `--verify`.
- **Q12. W1 and W3 as errors.** *Recommend:* after the lanes run them at
  zero (O-46).
- **Q13. DWARF-based foreign checks** (type sizes and enumerators from
  `.debug_info` for GCC/Clang objects with `-g`). *Recommend:* defer:
  `.debug_info` is the largest part of a `-g` input, reading it breaks
  the "pay nothing when off" frame unless opt-in, and hod emits no type
  units to deduplicate by.
- **Q14. Type rows in the loadable table.** *Recommend:* `=full` only
  (O-50); two libraries with private same-named types are common and
  never bind each other.

## Appendix A. Experiments

Scripts in `/Volumes/AgentWork/mortar-helper/.build/review-meta/`
(uncommitted; `run-all.sh WORK` runs them all, ~10 minutes; the record
`spec/landed/meta/WP-070.md` lists their outputs). Tool versions: g++
14.2.0 (Debian 14.2.0-19, also as `x86_64-linux-gnu-` and
`i686-linux-gnu-`), clang 19.1.7 and 22.1.8, GNU ld 2.44, lld 19.1.7,
mold 2.37.1, gdb 16.3, lldb 19.1.7, elfutils 0.192, glibc 2.41, qemu
10.0.13; kiln and hod release builds of the workspace.

| name | script | what |
|---|---|---|
| `catalogue` | `catalogue.sh` | K1-K16c: build, link (3 linkers × 2 orders × 2 compilers), run |
| `catalogue2` | `catalogue2.sh` | K16d, K16e, K17, K18, K18b, K18c; GCC LTO `-Wodr`, `-Wlto-type-mismatch`; GCC ASan |
| `weak-catalogue` | `run-all.sh` + `weakcheck.py` | W1-W5 and F over the catalogue's objects |
| `legal` | `legal.sh` | B1-B3 bytes across options/compilers; mixed static link with every extracted `libstdc++.a`/`libc.a` member through W1-W5; legal shapes |
| `container` | `container.sh` | encodings P, N, U, K, G, R × 3 targets × (3 linkers final and `-r`, 7 strip/objcopy variants, `--redefine-sym`, `ar`), eu-elflint |
| `size` | `size.sh` | entity counts of a libc++-heavy TU: kiln+hod (libc++, libstdc++), clang++ 22 (`-O0`, `-O2`, `-g`), g++ |
| `dynamic` | `dynamic.sh`, `hook/mason_rt.c`, `hook/gentab.py`, `patchelf_mini.py` | note / segment / `DT_` tolerance (ld.so, 3 strips, eu-elflint, gdb, lldb); hook detection (start-up, `dlopen`, `MASON_ODR=warn`); start-up cost; `.preinit_array` order |

Also measured by hand and recorded in the WP-070 record: llc 19/22 emit
encoding P from `!exclude` IR; hod refuses it; exported weak/unique
counts of three system libraries.

## Appendix B. Not verified

- SIGILL of K16d on a CPU without AVX2 (qemu-user executes AVX2).
- Clang's ASan ODR checker (runtime not installed); gold
  `--detect-odr-violations`, `abidiff`, MSVC, patchelf (not installed);
  clang modules' ODR hash.
- K19 (modules), `#pragma pack`, point of instantiation, `-std` bodies:
  not built (§2).
- The standard's clause numbers marked `[std, unverified clause]`.
- `eu-elflint` on a `DT_` tag alone (the measured file also had the
  note).
- Whether `-r` by the references keeps the relative order of same-named
  sections (`meta.md` MT-16).
- What `strip` does to a non-allocated `.mason.meta` in an executable
  (MT-32).
- Every cost marked `[estimate]`: the static checks' cost (O-21, O-22)
  and the k-way merge were not prototyped; only the pairwise hook was.
- The false-positive rates of kiln's hasher and odr-use analysis: kiln
  has neither yet.
- Symbol versioning (N6) and `libgcc` duplicates (N11) not built.
