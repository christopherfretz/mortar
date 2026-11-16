import sys
p = sys.argv[1]; s = open(p).read()
def rep(old, new):
    global s
    if s.count(old) != 1: raise SystemExit('count %d: %r' % (s.count(old), old[:70]))
    s = s.replace(old, new)

rep('''`--dynamic-list`, `--exclude-libs`, `-z nodelete`/`nodlopen`/`initfirst`/`interpose`/`origin`, `--allow-shlib-undefined` supported; `-Bsymbolic-non-weak*`, `-z notext`, `-f`/`-F`, `--spare-dynamic-tags` refused; the `-Bsymbolic` and `-z defs` no-op rows written for executables keep their meaning there |''',
'''`--dynamic-list`, `-Bno-symbolic`, `--exclude-libs`, `-z nodelete`/`nodlopen`/`initfirst`/`interpose`/`origin`, `--allow-shlib-undefined` supported; `-Bsymbolic-functions` and `-z now` in the first package (S1a: Python's `LDSHARED` line and rustc's `cdylib` line carry them, record §A.4); `-Bsymbolic-non-weak*`, `--dynamic-list-data`, `--dynamic-list-cpp-new`, `--dynamic-list-cpp-typeinfo`, `-z notext`, `-f`/`-F`, `--spare-dynamic-tags` refused (class `option`, D-112); the `-Bsymbolic` and `-z defs` no-op rows written for executables keep their meaning there |''')
rep('''| R-O10 | odr O-24, O-50, O-51 | a row's version is its `.dynsym` version node (D-115); `NONPREEMPTIBLE` is D-112's set; a `COPY` of a locally bound definition is refused in an executable (D-126 item 2) |''',
'''| R-O10 | odr O-24, O-50, O-51 | a row's version is its `.dynsym` version node (D-115); `NONPREEMPTIBLE` is D-112's bound set, truthful but for D-117's gap (D-126 item 1); a consumer of a row (odr.md's cross-library checks) reads `NONPREEMPTIBLE` as R-D43 does (D-126 item 2) |''')

rep('''### 18.3 Requests to `plumb.md` (stage 3 for a shared object)

plumb.md §12 (P-D1) accepts only executables. A shared object cannot be
judged "resolves as the program assumed" from the library alone: what a
preemptible reference binds to depends on the process (D-112). The
verifier checks instead that the library's records say exactly what
D-112 decided, structurally:

| id | request |
|---|---|
| P-R1 | P-D1 accepts `ET_DYN` without `DF_1_PIE` and without `PT_INTERP` as `Shared`; P-D2-P-D13 apply with this section's changes |
| P-R2 | every use of a key the verifier computes as preemptible (from the inputs and the options: D-110, D-112, recomputed independently) goes through a record naming that key (`GLOB_DAT`, `JUMP_SLOT`, symbolic, TLS); no word holds its link-time address. DF_SYMBOLIC is not trusted as evidence (mold omits it); the options are |
| P-R3 | no use of a non-preemptible key has a record naming it (`RELATIVE`/`IRELATIVE`/`r_sym` 0 TLS only) |
| P-R4 | `.dynsym`'s defined set equals the verifier's export set (D-110), with each entry's binding, type, visibility and version (D-115) |
| P-R5 | no `COPY`, no canonical PLT, no `DT_TEXTREL`, no LE TLS form in the output's code (D-117, D-119, D-122) |
| P-R6 | each `PT_LOAD` has `p_align` the max page size and `p_offset ≡ p_vaddr (mod p_align)` (glibc checks only the page, D-123) |
| P-R7 | `DT_INIT`/`DT_FINI` point at defined code; `DT_SONAME`, `DT_VERDEF*` are consistent with `.gnu.version_d` |
| P-R8 | mutations: each of W19-W24, W31-W39 written into a correct mortar library is reported by one of P-R2-P-R7 |''',
'''### 18.3 Requests to `plumb.md` (stage 3 for a shared object)

plumb.md §12 (P-D1) accepts only executables. A shared object cannot be
judged "resolves as the program assumed" from the library alone: what a
preemptible reference binds to depends on the process (D-112).

**What stands in for the host.** P-D9's loader model runs with a
synthetic first module, the **interposer**, in place of an executable:
it defines every key the verifier computes as preemptible (D-110,
D-112, recomputed from the inputs and the options, never from the
output's `.dynsym` or `DF_SYMBOLIC`) at a fresh address (a function at
a fresh code address, data at a fresh allocation of its `st_size`, a
TLS key at a fresh offset in a second module's block), as a preloaded
library would. Two views: (A) no interposer, every key resolved to the
library's own definition; (B) the interposer first. Each input
relocation's place must hold, in both views, the value the input
computes with `S` the run-time address of the definition the view
binds: the library's own for a bound key in both views, the
interposer's for a preemptible key in view B. This is P-D9's view 3
generalised from `WeakDyn` keys to every preemptible key, and it is
what turns P-D9's equality into a check of D-112.

| id | request |
|---|---|
| P-R1 | P-D1 accepts `ET_DYN` without `DF_1_PIE` and without `PT_INTERP` as `Shared`; P-D2-P-D13 apply with this section's changes; P-D9 runs the two views above in place of its views 1 and 3 (view 2, a second load bias, stays) |
| P-R2 | for every relocation of an `SHF_ALLOC` place of a live input (`.eh_frame` and the LSDA included: they are checked by value, not by the address rule) the views agree with the input. Plus D-117's address-level clause: a use whose named address (D-117's rule, computed by the verifier's own decoder from the input) lies in a preemptible data definition is reported even when both views agree on its value. Not checked: non-alloc places (debug sections hold link-time addresses by design) |
| P-R3 | every use of one IFUNC key, in every place and both views, yields one address: the IPLT entry for a bound key, the interposer's or the resolver's record for a preemptible one (W21); an i386 `Shared` output has no `STT_GNU_IFUNC` and no `IRELATIVE` (D-130) |
| P-R4 | `.dynsym`'s defined set equals the verifier's export set (D-110, `__start_`/`__stop_` `PROTECTED` included), with each entry's binding, type, visibility and version (D-115) |
| P-R5 | no `COPY`, no canonical PLT, no `DT_TEXTREL`, no dynamic record whose place is in a `PT_LOAD` without `PF_W` (outside `PT_GNU_RELRO`: such a record would write read-only text), no LE TLS form in the output's code (D-117, D-119, D-122), no output section `.ctors` or `.dtors` (their input goes to `.init_array`/`.fini_array`, D-124). i386: every place the output resolves to a PLT entry or GOT slot comes from an input relocation whose form sets `%ebx` (`PLT32`, `GOT32X` with a base register; D-130) |
| P-R6 | each `PT_LOAD` has `p_align` the max page size and `p_offset ≡ p_vaddr (mod p_align)` (glibc checks only the page, D-123) |
| P-R7 | `DT_SYMBOLIC` and `DF_SYMBOLIC` present iff the line's last `-Bsymbolic` form is `-Bsymbolic` and it has no `--dynamic-list` (D-112, D-102, D-103); `DT_INIT`/`DT_FINI` point at defined, live code (D-124); `DT_SONAME`, `DT_VERDEF*` consistent with `.gnu.version_d`; `GNU_PROPERTY_1_NEEDED_INDIRECT_EXTERN_ACCESS` present iff an input has it (D-112) |
| P-R8 | mutations: each wrong output of §17.7 written into a correct mortar library (or, where it concerns the executable, a host) is reported as the table below says, and its test says so |
| P-R9 | (executables, with R-D43) P-D11 gains: a `COPY` of a definition its library binds locally (`DT_SYMBOLIC`/`DF_SYMBOLIC`, or a `NONPREEMPTIBLE` row) whose `st_value` lies in a writable `PT_LOAD` of that library outside `PT_GNU_RELRO`, or in none, is reported |

P-R8's table (the P-D rules are main's plumb.md §12, applied through P-R1):

| wrong output | reported by |
|---|---|
| W19 (LE TLS against own TLS, AArch64) | P-R5 (LE form in a shared object's code); view B (P-R1) gives another block |
| W20 (exported IFUNC bound) | P-R2, view B; P-R3 |
| W21 (two addresses of one IFUNC) | P-R3 |
| W22 (`DT_INIT = 0`, the function collected) | P-R7 |
| W23 (`__start_` undefined, section dropped) | P-R4; P-D7's undefined-entry check |
| W24 (`.ctors` kept as an output section) | P-R5 |
| W25 (text relocations in `.eh_frame`, i386) | P-R5 (record in a non-writable `PT_LOAD`; `DT_TEXTREL`) |
| W26 (`%ebx`-relative IPLT entry as canonical address, i386) | P-R3 (no IFUNC in i386 `Shared`) |
| W27 (hidden IFUNC called through an `%ebx` stub, i386) | P-R3 |
| W28 (`-shared -static` C++ without `--eh-frame-hdr`, i386) | not applicable to the image: the driver's line omits `--eh-frame-hdr` and the output is what the line asks; D-125 makes mortar warn on the line (no image check can know the line was wrong) |
| W29 (`-shared -static` C library crashes, mold i386) | NOT caught: the cause was not isolated (record §B); needs S7 (i386) to reduce it to a record or a PLT form before a rule can name it |
| W30 (a `COPY` of writable locally bound data) | P-R9 (executables) |
| W31 (`DF_SYMBOLIC` with `--dynamic-list`) | P-R7; view B (P-R2) |
| W32 (`ADR_PREL_LO21`, `LD_PREL_LO19`, `PREL32` without a record, AArch64) | P-R2, view B |
| W33 (`PREL32` in `.data` bound; weak undefined non-zero) | P-R2, view B; P-D9 (P-R1) for the weak key |
| W34 (`PC32` dynamic record truncated, x86-64) | P-R2: view A with P-D9's second bias gives a truncated value |
| W35 (`PC32` call through an `%ebx` PLT, i386) | P-R5 (i386 clause) |
| W36 (`GOT32X` without a base register, i386) | P-R5 (i386 clause) |
| W37 (`GOTOFF` to a preemptible key, i386) | P-R2, view B |
| W38 (`&f` of an import is a PLT stub, AArch64) | P-R2: view A's value is the import's definition, the place holds the stub |
| W39 (`R_386_TLS_IE` in `.text` without `DT_TEXTREL`) | P-R5 (record in a non-writable `PT_LOAD`) |
| W40 (`extern "C++" {*}` exports nothing) | P-R4 |
| W41 (`NEEDED_INDIRECT_EXTERN_ACCESS` dropped) | P-R7 |
| W42 (versioned reference unversioned, `DT_NEEDED` dropped) | P-D6, P-D8 (P-R1) |
| W43 (TLS GD pair half bound, x86-64) | P-R2, view B (the interposer's block differs); P-D12 |
| W44 (anchor+immediate bound use, AArch64) | NOT caught: needs the verifier to decode the instruction immediate that follows the anchor's `ADRP`/`ADD` (an AArch64 load/store offset decoder over the input's code and its def-use within the block); both views agree, because the input's own relocation is to `.data + 0` and its value is right. This is the class of mislink no P-R catches: **a bound use whose relocation names neither the key nor an address inside it** (Q21) |

### 18.4 Requests to `diag.md` and to this spec

| id | target | request |
|---|---|---|
| R-D10 | diag §3.1 (symbols and relocations areas), §3.2 | new kinds: `non-pic-in-shared-object` (`NonPicInSharedObject { r_type, name, file }`, error, ``relocation {r_type} against symbol `{name}' which may bind externally can not be used when making a shared object; recompile with -fPIC``, GNU ld's; for D-117's address-level rule `{name}` is the definition and a note names the relocation's own symbol and place, and `-fdata-sections` at a boundary); `duplicate-version-pattern` (`DuplicateVersionPattern { pattern, first, second }`, error, `{pattern} is listed as global in version {first} and local in version {second}`, own; GNU ld: `duplicate expression`); `versioned-definition-hidden` (`VersionedDefinitionHidden { name, version }`, warning, `{name}@{version} is made local by version {version}'s local: entries; executables linked against {name}@{version} will not load`, own); `copy-of-locally-bound-definition` (`CopyOfLocallyBoundDefinition { name, file }`, error, `copy relocation against {name}, which {file} binds to itself; the program and {file} would use two different objects`, own; warning when R-D43 says so, with the note `{name} is read-only in {file}: two equal copies`); `version-script-undefined-name` (`VersionScriptUndefinedName { name, version }`, warning, `version script assigns {name} to {version}, but no file defines it`, own). `version-reassigned` exists (§3.1) and is used by resolve §9.7 rule 7 |
| R-D43 | this spec, D-43 (and D-18's neighbourhood) | the copy rule of D-126 item 2 for `Exec` and `Pie`: refuse `copy-of-locally-bound-definition` when the copied definition's library binds it locally and its `st_value` lies in a `PF_W` `PT_LOAD` outside `PT_GNU_RELRO` or in none; warn when it lies in a read-only `PT_LOAD` or in `PT_GNU_RELRO`. "Binds locally": `DT_SYMBOLIC`/`DF_SYMBOLIC`, or a `.note.mason` row with `NONPREEMPTIBLE`. Decided from the library's program headers and `.dynamic` (the symbol count from `DT_GNU_HASH` or `DT_HASH`), never its section headers. Its own package (record §D, S-R43); plumb request P-R9 |''')
open(p, 'w').write(s)
