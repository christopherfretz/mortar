# spec: mortar_diag

Status: normative (WP-007, crate `crates/mortar_diag`; the relocation
kinds, `undefined-symbol` as the scan reports it and `internal-error` by
WP-020c, for `spec/arch.md` §7 and §10; the kinds of the input phase and
of resolution, and the wording policy of §8.3, by WP-020d, for
`spec/input.md` §13 and `spec/resolve.md` §14). "MUST" and "SHOULD"
carry their RFC 2119 meanings. Every test in `crates/mortar_diag` names the
section it checks with a `// spec: diag.md §N` comment; §11 maps sections
to tests.

## 1. Scope

`mortar_diag` owns

- the diagnostic model: severity, kind (with a stable id), place, context
  lines (§2, §3);
- the vocabulary of refusals: how mortar says "not implemented" by name
  (§4; `docs/ARCHITECTURE.md`, principle 1);
- the text every diagnostic is printed as (§5);
- the collector: deterministic order under parallel emission, the error
  limit, the warning switches, and "has the link failed?" (§6, §7).

It depends on no crate, mortar's included, and contains no `unsafe`. Every
other crate reports through it, so no other crate forms the text of a
diagnostic: a caller says *what* (a kind and its fields) and *where*
(places), and §5 decides the characters.

Not here: demangling (the hook of §5.4 is where it attaches; the demangler
is a later work package), parsing `--error-limit`, `--fatal-warnings`,
`-w`, `--color-diagnostics` (`spec/cli.md` sets the values of §5.1 and
§6.3), deciding whether a terminal takes color, translation of messages,
machine-readable output (§5.7), and source positions from debug information
(§9).

## 2. The model

### 2.1 Severity

`Severity` is `Warning` or `Error`, and `Warning < Error`.

- An **error** means the link fails (§7).
- A **warning** means the link goes on and its output is valid.

There is no "fatal" severity and no "note" severity. A problem that makes
going on pointless is an error and the phase that found it returns; the
link stops at the next poll of §7. What other tools print as a note is a
context line of the diagnostic it belongs to (§2.4), so that a diagnostic
is one value and its lines cannot be interleaved with another's. A bug of
mortar's is a panic, not a diagnostic (`docs/AGENTS.md`, rule 5), with one
exception: a table that a check finds in violation of its own validity
rules (`spec/model.md` §4.4, `Layout::check`) is reported as
`internal-error` (§3.2). The check runs on data, in production as well as
in tests, so its finding is a value to report, and the link then fails
like any other: status 1, no output file (§7).

Incremental mode's `incr-full-link` report (`spec/incr.md` IN-17) is a
line of the report, written to standard error or the report file, not a
diagnostic: it has no kind, no severity and no id, and `--fatal-warnings`
does not see it.

### 2.2 Kind and id

`Kind` says what the diagnostic states. Each kind has

- a **stable id**: lower-case ASCII words joined by `-`
  (`undefined-symbol`). Tests assert on ids, never on message text outside
  this crate's own tests. An id MUST NOT change or be reused once merged.
  The text form does not print ids;
- fields that carry the names involved, as data;
- exactly one message wording, fixed in §3.1.

The severity is not part of the kind: `undefined-symbol` is an error by
default and a warning under `--warn-unresolved-symbols`. The exception is
`refused`, always an error (§4.3).

### 2.3 Places

A `Place` says where something is. Callers build places from parts; they
never concatenate a place into a string. The forms and their text:

| form | fields | text |
|---|---|---|
| file | path | `foo.o` |
| file (archive member) | archive path, member name | `libx.a(y.o)` |
| section | file, section name, byte offset in the section | `foo.o:(.text+0x10)` |
| symbol | file, symbol-table name | `foo.o:(main)` |
| argument | the argument's text | `argument '--entry=main'` |
| script | path, line, column | `libc.so:5:9` |
| record | file, symbol-table name (optional), kept, source path and line (optional) | `foo.o:(f) (kept), at h.h:3`; `foo.o, at h.h:3` |

- A "file" inside the section and symbol forms is either file form, so
  `libx.a(y.o):(.text+0x10)`.
- A path is the path as the user, a search directory or a script gave it:
  not made absolute, not canonical. `FileRef::from_path` replaces bytes
  that are not UTF-8 with U+FFFD.
- The offset is printed as `0x` and lower-case hexadecimal digits without
  leading zeros (`0x0` for zero).
- The symbol form's name is the symbol table's (mangled) and goes through
  the hook of §5.4.
- The argument's text is what identifies it to the user: one argument
  (`--entry=main`) or an option and its separate value (`-e main`).
- Script lines and columns start at 1. A column counts bytes.
- The record form is a Mason metadata record's (`spec/odr.md` §9.1,
  WP-073): `(kept)` when the file's copy is the one the link keeps, and
  `, at {path}:{line}` from the record's own source position (`:{line}`
  left out when the record gives line 0). Its name goes through the hook
  of §5.4 like the symbol form's.

`Place` is `#[non_exhaustive]`: §9 lists the forms that are planned.

### 2.4 Context lines

A diagnostic carries zero or more context lines, printed in the order the
caller added them. A context line is one of

- a **relation** and a place: `>>> referenced by foo.o:(.text+0x10)`;
- a **note**, one line of text that states a fact which is not a place:
  `>>> expected one of: sysv, gnu, both`.

The relations and their words:

| relation | words | use |
|---|---|---|
| `DefinedDifferentlyIn` | `defined differently in` | where a definition of the diagnostic's symbol is that differs from the one before it (`spec/odr.md` §9.1) |
| `DefinedIn` | `defined in` | where a definition of the diagnostic's symbol is |
| `In` | `in` | what contains the construct, when the head line has no place |
| `NamedBy` | `named by` | the argument or script position that brought a file into the link |
| `ReferencedBy` | `referenced by` | where a reference to the diagnostic's symbol is |

A relation is added to the table and to `Relation` (sorted by its words)
with a test of its text; its words are lower-case and contain no colon.

A note is the one place where a caller supplies prose. It SHOULD start in
lower case, have no final period, and not restate a place that a relation
could carry.

### 2.5 The diagnostic value

A `Diagnostic` is a severity, a kind, an optional place and the context
lines. It is built by value with `Diagnostic::error`, `::warning` or
`::refused` and the builder methods `.at(place)` (the place the head line
starts with), `.with(relation, place)` and `.note(text)`, and read back
through accessors (`severity`, `kind`, `id`, `refusal`, `place`,
`context`).

`Diagnostic` MUST be `Send` and `Sync` and own all its data (no lifetime):
it is created on a worker thread and outlives the inputs it describes.

## 3. Kinds

### 3.1 The table

`{x}` is the field `x` after §5.5; `{name}` of the kinds that name a
symbol (every `{name}` of the area "symbols", `relocation-incompatible`,
`relocation-out-of-range`) goes through §5.4, also where it appears
twice. The areas, in order: command line (`conflicting-options` to
`unknown-option`), files (`ambiguous-sysroot-path` to
`written-file-clash`), file formats, symbols (`archive-backref`
to `version-reassigned`), one-definition rule (`meta-malformed` to
`odr-vtable-rtti-mismatch`, M6), relocations, layout (`executable-stack` to
`section-conflict`), incremental linking (`incremental-not-reproducible`
to `incremental-file-clash`), refusals, internal. The wording column is the message of §5.2, complete: nothing
is added before or after it.

| id | variant | message | wording from |
|---|---|---|---|
| `conflicting-options` | `ConflictingOptions { first, second }` | `{first} and {second} cannot be used together` | own (GNU ld: `-r and -shared may not be used together`) |
| `invalid-option-value` | `InvalidOptionValue { option, value }` | `{option}: invalid value '{value}'` | own (§8.2) |
| `missing-argument` | `MissingArgument { option }` | `{option}: missing argument` | lld |
| `no-input-files` | `NoInputFiles` | `no input files` | GNU ld, lld |
| `response-file-limit` | `ResponseFileLimit { limit }` | `too many response files: more than {limit} expanded` | own (GNU ld: `too many @-files encountered`) |
| `unbalanced-option` | `UnbalancedOption { option, counterpart }` | `{option} without a matching {counterpart}` | own (lld: `stray --end-group`, `unbalanced --push-state/--pop-state`) |
| `unknown-option` | `UnknownOption { option }` | `unknown argument '{option}'` | lld |
| `ambiguous-sysroot-path` | `AmbiguousSysrootPath { spelling }` | `ambiguous sysroot path '{spelling}'` | own |
| `cannot-open-file` | `CannotOpenFile { path, reason }` | `cannot open {path}: {reason}` | lld, mold |
| `cannot-open-output` | `CannotOpenOutput { path, reason }` | `cannot open output file {path}: {reason}` | GNU ld, lld |
| `cannot-write-output` | `CannotWriteOutput { path, reason }` | `cannot write output file {path}: {reason}` | own |
| `duplicate-soname` | `DuplicateSoname { soname }` | `DT_SONAME '{soname}' is already loaded; this file is not used` | own |
| `ignored-member` | `IgnoredMember { what }` | `archive member is not a relocatable object: {what}` | own (lld: `archive member 'x' is neither ET_REL nor LLVM bitcode`) |
| `incompatible-file` | `IncompatibleFile { expected, found }` | `incompatible file: {expected} is expected but got {found}` | mold |
| `input-changed` | `InputChanged` | `file changed while it was read` | own |
| `library-not-found` | `LibraryNotFound { name }` | `cannot find -l{name}: No such file or directory` | GNU ld (§8.3) |
| `needed-not-found` | `NeededNotFound { name, needed_by }` | `{name}, needed by {needed_by}, not found (try using -rpath or -rpath-link)` | GNU ld (§8.3) |
| `no-target` | `NoTarget` | `target emulation unknown: -m or at least one .o file required` | lld |
| `script-cycle` | `ScriptCycle` | `linker script names itself` | own |
| `skipped-incompatible` | `SkippedIncompatible { path, search }` | `skipping incompatible {path} when searching for {search}` | GNU ld (§8.3) |
| `static-link-of-shared-object` | `StaticLinkOfSharedObject { path }` | `attempted static link of dynamic object {path}` | GNU ld, lld (§8.3) |
| `written-file-clash` | `WrittenFileClash { written, what }` | `{written} names {what}; nothing is written to it` | own (`spec/cli.md` §3.14; GNU ld: `input file 'a.o' is the same as output file`) |
| `cet-report` | `CetReport { path, property }` | `{path}: missing {property} property (-z cet-report)` | own (GNU ld: `an.o: error: missing IBT and SHSTK properties`); severity from `-z cet-report` (`spec/x64.md` X-43) |
| `malformed-input` | `Malformed { what }` | `{what}` | lld's shape |
| `unknown-file-type` | `UnknownFileType` | `file not recognized: file format not recognized` | GNU ld (§8.3) |
| `unknown-property` | `UnknownProperty { path, pr_type }` | `{path}: unsupported GNU_PROPERTY_TYPE {pr_type:#x}; property dropped` | GNU ld's phrase; a warning (`spec/synth.md` S-95, R-D2) |
| `archive-backref` | `ArchiveBackref { name }` | `backward reference to an archive member: {name}` | own (lld: `backward reference detected: x in b refers to a`) |
| `archive-choice` | `ArchiveChoice { name }` | `GNU ld would choose another definition of {name}` | own |
| `comdat-mismatch` | `ComdatMismatch { name, signature }` | `symbol {name} is defined only in a discarded copy of COMDAT group {signature}` | own |
| `common-alignment` | `CommonAlignment { name }` | `definition of {name} is less aligned than its common symbols` | own |
| `common-larger-than-definition` | `CommonLargerThanDefinition { name }` | `common symbol {name} is larger than its definition` | own |
| `common-overridden` | `CommonOverridden { name }` | `common symbol {name} is overridden by a definition` | own |
| `discarded-section-reference` | `DiscardedSectionReference { name }` | `relocation refers to a symbol in a discarded section: {name}` | lld |
| `duplicate-symbol` | `DuplicateSymbol { name }` | ``duplicate symbol: {name} (multiple definition of `{name}')`` | lld, with GNU ld's phrase (§8.3) |
| `entry-not-found` | `EntryNotFound { name }` | `cannot find entry symbol {name}` | GNU ld, lld (their lines go on: `; defaulting to …`, `; not setting start address`) |
| `gnu-warning` | `GnuWarning { text }` | `{text}` | GNU ld (the section's text) |
| `linker-symbol-redefined` | `LinkerSymbolRedefined { name }` | `cannot redefine linker-defined symbol: {name}` | lld's (`cannot redefine linker defined symbol`) |
| `multiple-common` | `MultipleCommon { name }` | `multiple common symbols: {name}` | own |
| `non-exported-symbol` | `NonExportedSymbol { name }` | `non-exported symbol referenced by a shared object: {name}` | own |
| `required-symbol` | `RequiredSymbol { name }` | `required symbol not defined: {name}` | own (GNU ld: ``required symbol `x' not defined``) |
| `tls-mismatch` | `TlsMismatch { name }` | `TLS attribute mismatch: {name}` | lld |
| `undefined-symbol` | `UndefinedSymbol { name }` | ``undefined symbol: {name} (undefined reference to `{name}')`` | lld, with GNU ld's phrase (§8.3) |
| `undefined-version` | `UndefinedVersion { name, version }` | `version {version} of {name} is not defined` | own |
| `version-reassigned` | `VersionReassigned { name, first, second }` | `{name} is assigned to version {first} and {second}; {first} is kept` | own |
| `versioned-export` | `VersionedExport { name, version }` | `versioned definition {name}@@{version} cannot be exported (version definitions are not supported)` | own (`spec/dyn.md` D-27: GNU ld writes a verdef, lld and mold export it unversioned; M4) |
| `meta-malformed` | `MetaMalformed { reason }` | `malformed Mason metadata: {reason}` | own |
| `meta-version` | `MetaVersion { major }` | `Mason metadata version {major} is not supported; its records are ignored` | own |
| `odr-abi-mismatch` | `OdrAbiMismatch { option, first, second }` | `translation units compiled with different {option}: {first} and {second}` | own (WP-073) |
| `odr-contents-mismatch` | `OdrContentsMismatch { name }` | `{name} has different contents in different files` | own (`spec/odr.md` §8.1 W4; WP-072) |
| `odr-declaration-mismatch` | `OdrDeclarationMismatch { name }` | `{name} is declared with a type that does not match its definition` | own (WP-073) |
| `odr-internal-reference` | `OdrInternalReference { name }` | `{name} refers to an entity with internal linkage, which differs in each translation unit` | own (WP-073) |
| `odr-isa-mismatch` | `OdrIsaMismatch { name }` | `the kept copy of {name} was compiled for other target features than a translation unit that uses it` | own (WP-073) |
| `odr-size-mismatch` | `OdrSizeMismatch { name }` | `{name} has different sizes in different files` | own |
| `odr-strong-and-comdat` | `OdrStrongAndComdat { name }` | `{name} has a non-inline definition and an inline copy` | own |
| `odr-symbol-type-mismatch` | `OdrSymbolTypeMismatch { name }` | `{name} is a function in one file and data in another` | own |
| `odr-type-mismatch` | `OdrTypeMismatch { name }` | `type {name} violates the one-definition rule` | own (GCC `-Wodr`'s shape; WP-073) |
| `odr-violation` | `OdrViolation { name }` | `{name} violates the one-definition rule` | own (GCC `-Wodr`'s shape; WP-073) |
| `odr-vtable-rtti-mismatch` | `OdrVtableRttiMismatch { name }` | `the vtable of {name} is emitted by translation units compiled with and without RTTI` | own (WP-073) |
| `canonical-plt-in-pie` | `CanonicalPltInPie { r_type, name }` | ``relocation {r_type} against symbol `{name}' cannot be used in a position-independent executable; recompile with -fPIE, or link with -no-pie`` | GNU ld (`spec/dyn.md` D-44) |
| `copy-relocation-against-protected` | `CopyRelocationAgainstProtected { name, file }` | ``copy relocation against non-copyable protected symbol `{name}' in {file}`` | GNU ld (`spec/dyn.md` D-18) |
| `copy-relocation-of-zero-size` | `CopyRelocationOfZeroSize { name }` | `cannot create a copy relocation for symbol {name}` | lld (`spec/dyn.md` D-42) |
| `relocation-incompatible` | `RelocationIncompatible { r_type, name, reason }` | `relocation {r_type} cannot be used against symbol '{name}'; {reason}` | lld's shape, with a reason (§3.2) |
| `relocation-misaligned` | `RelocationMisaligned { r_type, value, align }` | `improper alignment for relocation {r_type}: {value} is not aligned to {align} bytes` | lld |
| `relocation-out-of-range` | `RelocationOutOfRange { r_type, value, min, max, name }` | `relocation truncated to fit: {r_type} against '{name}': {value} is not in [{min}, {max}]` | lld's value and bounds; GNU ld's phrase |
| `text-relocation` | `TextRelocation { r_type, name }` | ``relocation {r_type} against symbol `{name}' can not be used when making a PIE object; recompile with -fPIE`` | GNU ld (`spec/dyn.md` D-47) |
| `weak-undefined-pc-in-pie` | `WeakUndefinedPcInPie { r_type, name }` | ``relocation {r_type} against undefined weak symbol `{name}' cannot be used in a position-independent executable; recompile with -fPIE`` | GNU ld (`spec/dyn.md` D-25; `Pie` only, once per key) |
| `executable-stack` | `ExecutableStack { file }` | `{file}: requires executable stack (because the .note.GNU-stack section is executable)` | GNU ld |
| `output-too-large` | `OutputTooLarge { what, limit }` | `output too large: {what} exceeds {limit}` | own (lld: `output file too large`) |
| `section-conflict` | `SectionConflict { name, first, second }` | `section {name}: {first} and {second} inputs cannot be combined` | own (lld: `section type mismatch for {name}`) |
| `incremental-not-reproducible` | `IncrementalNotReproducible` | `SOURCE_DATE_EPOCH is set, but an incremental link's output depends on its edit history` | own (`spec/incr.md` IN-7) |
| `incremental-unsupported` | `IncrementalUnsupported { option }` | `--incremental is not supported with {option}; linking without a state` | own (`spec/incr.md` IN-12) |
| `incremental-update-failed` | `IncrementalUpdateFailed { reason }` | `--incremental=update: cannot update: {reason}` | own (`spec/incr.md` IN-10) |
| `incremental-file-clash` | `IncrementalFileClash { option, path, what }` | `{option}={path} names {what}; nothing is written to it` | own (`spec/incr.md` IN-17); no longer emitted since WP-087, which made `written-file-clash` cover the state and the report (the id stays, never reused) |
| `refused` | `Refused { class, name, not_recognized }` | `unsupported {noun of class}: {name}` (§4), then `; file not recognized: file format not recognized` when `not_recognized` (§8.3) | own (§8.2) |
| `internal-error` | `InternalError { what }` | `internal error in mortar: {what}` | own |

The kind of M6 (the one-definition rule, `spec/odr.md` §9.1) **not yet
in the code**. Its row moves into the table above, at the place of its
area in the order stated before the table, with the package that
implements it (§3.3): WP-076, with `--odr-check-archives` (WP-071 moved
the two `meta-` kinds, WP-072 the kinds of the checks on foreign objects,
WP-073 those of the record checks). The table above stays the list of
`Kind::IDS`, which a test compares with the code.

| id | variant | message | wording from |
|---|---|---|---|
| `odr-archive-shadow` | `OdrArchiveShadow { name }` | `{name} in an archive member that is not linked is defined differently` | own |

### 3.2 Notes on the kinds

- `cannot-open-output` and `cannot-write-output` name the output path as
  the user gave it, never the temporary file (`spec/image.md` §7); key
  `(105, 0, 0)`. `cannot-write-output` was added by WP-026.
- `unknown-option` is for an argument that is not an option of GNU ld's
  command line as `spec/cli.md` lists it (a typing error, most likely). An
  option that is listed and not implemented is `refused` with class
  `option`: the user is told that the limit is mortar's.
- `conflicting-options`, `response-file-limit` and `unbalanced-option`
  were added by WP-008 (`spec/cli.md` §3.2, §2.6, §3.3): `-r` with
  `-shared` or `-pie`; GNU ld's limit on `@file` expansions; a
  `--pop-state`, `--end-group` or `--end-lib` with nothing to close, or a
  nested `--start-lib`.
- `{option}` is spelled as the user wrote it (`-o`, `--hash-style`,
  `-z stack-size`), so that the line can be found from the build log.
- `cannot-open-file` covers failing to open an input and failing to read
  its bytes (opening a directory succeeds; reading it does not), as lld.
- `{reason}` is the operating system's message. `os_reason(&io::Error)`
  gives it: Rust's `Display` text without the ` (os error N)` it appends,
  which makes the line the one lld, mold and GNU ld print
  (`No such file or directory`). mortar does not call `setlocale`, so the
  text is the C locale's.
- `library-not-found`: `{name}` is the text after `-l`, so `-l:libx.a`
  gives `cannot find -l:libx.a: No such file or directory`, GNU ld's line
  (§8.3). The directories searched are notes (`spec/input.md` §4.4).
- The other kinds of the files area are the input phase's
  (`spec/input.md` §13, phase 20, the key of its §12.2), each `.at` the
  file it is about (an archive member as `archive(member)`), with the
  `>>> named by` lines of §2.4 for a file a script or a closure edge
  brought in:
  - `ambiguous-sysroot-path`: `{spelling}` is the path as written (`-L`,
    or a script entry), and two notes give the two readings
    (`spec/input.md` §4.3);
  - `skipped-incompatible`, a warning: `{search}` is `-l{name}` for a
    library, or the name as written for a script entry or a `DT_NEEDED`
    edge (`spec/input.md` §4.4);
  - `incompatible-file`: `{expected}` and `{found}` are target names
    (`aarch64`, `x86_64`, `i386`), with a note saying what set the
    target (`spec/input.md` §3.3);
  - `no-target`: no `-m` and no ELF input (`spec/input.md` §3.1); no
    place;
  - `ignored-member`: a warning for a lazy member, an error under
    `--whole-archive`; `{what}` says what the member is
    (`spec/input.md` §7.2);
  - `input-changed`, at the lowest mention of the file
    (`spec/input.md` §11.5);
  - `script-cycle`, at the script position that names a script already
    being read, with the chain as `named by` lines (`spec/input.md` §5.3);
  - `duplicate-soname`, a warning, with the note `loaded from {path}`
    (`spec/input.md` §8.3);
  - `static-link-of-shared-object` (`spec/input.md` §8.4) and
    `needed-not-found`, a warning whose notes are the directories
    searched (`spec/input.md` §8.6): GNU ld's phrases (§8.3).
- `malformed-input` and `unknown-file-type` are emitted with a place
  (`.at`): the file, or the section and offset when the reader knows it.
  `{what}` is the reader's own statement (`mortar_elf`'s error values
  render to it): lower-case, no final period, naming the field and the
  offending value. `unknown-file-type` is for bytes mortar cannot name, and
  its line at the file is GNU ld's
  (`trunc.o: file not recognized: file format not recognized`, §8.3); a
  format it can name and does not read is `refused` with class
  `file-format`, built with `Diagnostic::refused_unrecognized_format` when
  GNU ld does not read it either (§8.3).
- `duplicate-symbol` and `undefined-symbol` carry the symbol-table name,
  shown twice: after lld's head and inside GNU ld's phrase, quoted as GNU
  ld quotes it (`` `name' ``, §8.3). For `duplicate-symbol`, where the
  definitions are is context (`DefinedIn`); how many of them a diagnostic
  lists is decided by `spec/resolve.md` (§3.8, §14: the key of the second
  definition, phase 30).
- `undefined-symbol` is **one diagnostic per key** (`spec/resolve.md`
  §5.3): its references are context, `>>> referenced by
  file:(section+0xoff)` for the first three surviving references in order
  of `(FileId, section index, relocation index)`, then the note
  `referenced N more times`. Its key is that of the least surviving
  reference: phase 70, `(70, FileId, section index × 2^32 + relocation
  index)` (`spec/model.md` §8), so the order of the report is the order of
  first references. It is emitted by the relocation scan
  (`spec/arch.md` §3.4), after sequence recognition, from `Resolution`'s
  `def`; the undefined references of a needed shared object are emitted by
  resolution (phase 30, unit = the shared object, seq = its `.dynsym`
  index). So `ld: error: undefined symbol: missing (undefined reference
  to `missing')` / `>>> referenced by undef.o:(.text+0xc)`. It is an
  error, or a warning where `spec/resolve.md` §5.2 says so
  (`--warn-unresolved-symbols`); a hidden undefined symbol with a
  surviving reference is always an error.
- The other kinds of the symbols area are resolution's (`spec/resolve.md`
  §14 gives each its severity and key), except `comdat-mismatch` and
  `discarded-section-reference`, which the scan emits per key like
  `undefined-symbol`, and `gnu-warning` per symbol at the scan (phase 70)
  or per file at resolution (phase 30). `archive-backref` and
  `archive-choice` are emitted only under `--warn-backrefs`
  (`spec/resolve.md` §4.6); `multiple-common` and `common-overridden` only
  under `--warn-common`. `entry-not-found`, `common-*`, `archive-*` and
  `version-reassigned` are warnings, the others errors. `gnu-warning`'s
  `{text}` is the section's text, as written, after §5.5; it names no
  symbol for §5.4.
- `cet-report` (`spec/x64.md` X-43, R-11): `{property}` is `IBT` or
  `SHSTK`, one diagnostic per (live input object, missing property),
  `.at` nothing (the path is in the message, as `unknown-property`). Its
  severity is the option's: a warning under `-z cet-report=warning`, an
  error under `=error`; none is emitted under `=none`.
- The **relocation kinds** are reported with `.at` the relocation's place
  (`file:(section+0xoff)`); for a synthetic chunk (a PLT entry, a thunk, a
  patch) the place is the output section and offset. `{r_type}` is the
  type's ABI name (`R_AARCH64_CALL26`), or `0x…` for a number without one.
  `{name}` is the target's symbol-table name; for a section symbol it is
  the section and offset (`.text+0x40`); for a branch through the PLT it is
  the symbol's name. `{value}`, `{min}`, `{max}` (`i64`) and `{align}`
  (`u64`) are printed in decimal, negative values with `-`, as lld prints
  them; for ELFCLASS32 the value is the 32-bit-reduced one
  (`spec/arch.md` §2.3). Who emits which, with which key, is
  `spec/arch.md` §7's: `relocation-incompatible` at scan (phase 70), the
  other two at fill (phase 100).
- `relocation-incompatible`'s `{reason}` is one of a closed list,
  `IncompatibleReason`, which a test compares instead of the text:

  | variant | words |
  |---|---|
  | `RecompileFpic` | `recompile with -fPIC` |
  | `RecompileFpie` | `recompile with -fPIE` |
  | `ReadOnlySection` | `it is in a read-only section` |
  | `AddendMustBeZero` | `the addend must be 0` |
  | `NotThreadLocal` | `it is not thread-local` |
  | `ThreadLocal` | `it is thread-local` |
  | `LocalExecInShared` | `local-exec TLS in a shared object` |
  | `InstructionMismatch` | `the instruction does not match the relocation` |
  | `NonAlloc` | `not allowed in a non-allocated section` |

  A reason is added like a refusal class (§4.1): the variant, the row, the
  test.
- An incremental link that falls back to a full link (`spec/incr.md`
  §7) is not a diagnostic: §2.1 has no note severity, and a warning
  would fail `--fatal-warnings` builds on every fall-back, which is not a
  problem with the link. It is one line on standard error in the
  report's words, `incremental: full (<reason>)` (IN-10's
  `incr-full-link`; with `--incremental-report` the report line instead),
  and the link's status is unchanged. Under `--incremental=update` the
  same reason is the error `incremental-update-failed`.
- `internal-error` is a bug of mortar's that a check of its own data
  found (§2.1): today, a layout mortar computed that `Layout::check`
  rejects (`spec/model.md` §4.4). `{what}` names the table, the rule and
  what is wrong (`invalid layout: §4.4 rule 1: a live section has no
  place`); the rows involved are notes. Like `refused` it is always an
  error (`Diagnostic::warning` of it is an error, and `-w` cannot hide it):
  an output written from data mortar knows to be wrong would be a wrong
  link. It has no place: the input is not at fault. A layout *loaded from a
  link state* that fails the same check is not an internal error but
  untrusted input that is not reused (phase 15, `spec/incr.md`); the
  incremental phase reports it in its own terms.

- The one-definition rule's kinds (M6, `spec/odr.md` §9.1; added by
  WP-072 and WP-073 as each check lands, per §3.3) are warnings in the
  default mode `--odr=warn`; `odr-violation`, `odr-type-mismatch`
  (layout differences) and `odr-declaration-mismatch` are errors under
  `--odr=error` and `--odr=strict`; `meta-malformed` and `meta-version`
  of a container's contents are errors reported under `--odr=strict`
  only, the one mode that reads containers, and `meta-malformed` for a
  section of the name that is not a container is a warning in every
  mode (`spec/odr.md` O-40, O-41, O-42, O-47). Which checks are on
  by default is `spec/odr.md` O-47's register; `odr-internal-reference`
  `odr-strong-and-comdat` and `odr-contents-mismatch` are reported only
  under `--odr=strict`,
  `odr-archive-shadow` only with `--odr-check-archives`. Every `{name}`
  goes through §5.4. Their context lines (§2.4) and their key (phase 40,
  `spec/model.md` §8; `2^40 +` the row's index for a row without a
  symbol) are `spec/odr.md` §9.1's. Never a `refused`: a finding is not a
  limit of mortar's.

### 3.3 Adding a kind

A work package that needs a new kind adds, in one change: the variant, its
id in `Kind::id` and `Kind::IDS`, its message in the renderer, its row in
§3.1, and a test of its exact text in `tests/kinds.rs`.

- Variants, rows and tests are grouped by area (command line, files, file
  formats, symbols, relocations, layout, refusals, internal) and sorted by id
  inside an area. A new one is
  **inserted at its place, not appended**: two branches that add different
  kinds then touch different lines.
- The wording follows §8.2 and the wording policy of §8.3: GNU ld's phrase
  verbatim where it has one for the condition, otherwise an existing
  linker's wording where one fits the shape; lower-case, no final period,
  names after a colon or in single quotes.
- A field that holds a symbol name MUST be returned by `Kind::symbol` so
  that it reaches the demangling hook.
- Refusing a construct never needs a new kind (§4).

`Kind` is `#[non_exhaustive]`: adding a kind does not break a crate that
matches on kinds.

## 4. Refusals

mortar links a construct correctly or stops with an error that names it. A
refusal is that error: kind `Refused { class, name }`, id `refused`.

```text
<prog>: error: [<place>: ]unsupported <noun>: <name>
```

### 4.1 Classes

`RefusalClass` says what sort of construct was refused. It has a stable id
(for tests and machine formats) and the noun of the message.

| id | variant | noun | a name looks like |
|---|---|---|---|
| `option` | `Option` | `option` | `--plugin` |
| `option-value` | `OptionValue` | `option value` | `--hash-style=sysv`, `-z nognustack` |
| `emulation` | `Emulation` | `emulation` | `elf_i386` |
| `file-format` | `FileFormat` | `file format` | `ELF32`, `big-endian ELF`, `LLVM bitcode` |
| `machine` | `Machine` | `machine` | `EM_RISCV` |
| `object-type` | `ObjectType` | `object type` | `ET_EXEC` |
| `section-type` | `SectionType` | `section type` | `SHT_GNU_verdef` |
| `section-flag` | `SectionFlag` | `section flag` | `SHF_GNU_RETAIN` |
| `relocation-type` | `RelocationType` | `relocation type` | `R_AARCH64_TLSDESC_CALL` |
| `symbol-type` | `SymbolType` | `symbol type` | `STT_GNU_IFUNC` |
| `symbol-binding` | `SymbolBinding` | `symbol binding` | `STB_GNU_UNIQUE` |
| `compression-type` | `CompressionType` | `compression type` | `ELFCOMPRESS_ZSTD` |
| `script-command` | `ScriptCommand` | `linker script command` | `PHDRS` |
| `feature` | `Feature` | `feature` | `relocatable output (-r)` |

A class is added (rarely) like a relation: the variant, the row, the test.
`feature` exists so that nothing is ever refused without a diagnostic for
want of a class; a construct with a class of its own uses it.

### 4.2 The name

`name` identifies the construct to someone who will search for it:

- its spelling in the document that defines it (the gABI's `SHT_`/`SHF_`/
  `STT_` names, the processor supplement's `R_` names, GNU ld's manual for
  options and script commands), exactly, with nothing appended;
- or, when the value has no name mortar knows, the value in hexadecimal
  with `0x` (`0x999`; for a flag, the mask of the unknown bits).

Detail (why, what to use instead, the milestone that brings it) goes in a
note, not in the name: tests compare the name
(`docs/VALIDATION.md`, "A test of a refusal checks the name").

A refusal SHOULD carry the place of the construct: `.at` the section and
offset for a relocation, the file for a section or a header field, the
script position for a command, and nothing for an option (the name is the
place).

### 4.3 Always an error

A refusal MUST be an error: `Diagnostic::new(Severity::Warning,
Kind::Refused { .. })` and `Diagnostic::warning` of a refusal produce an
error. A refusal that lets the link go on is the silent acceptance
principle 1 forbids. Consequently `-w` never hides a refusal.

## 5. Text

### 5.1 Style

The renderer is given a `Style`:

- `prog`: the basename mortar was invoked as (`mortar`, `ld`,
  `ld.mortar`), which the caller takes from `argv[0]`;
- `color`: whether to color (§5.6). The renderer does not look at the
  terminal or the environment: `--color-diagnostics=auto|always|never` is
  resolved by the command-line crate to this boolean;
- `demangle`: the hook of §5.4.

`Style::plain(prog)` is no color and no demangling.

### 5.2 Grammar

```text
report      = *diagnostic [limit-line]
diagnostic  = head *context
head        = prog ": " severity ": " [place ": "] message LF
severity    = "error" / "warning"
context     = ">>> " ( "mangled name: " name          ; §5.4, first if present
                     / relation " " place             ; §2.4
                     / note ) LF
place       = file
            / file ":(" section "+0x" 1*hex ")"
            / file ":(" name ")"
            / "argument '" text "'"
            / path ":" line ":" column
file        = path [ "(" member ")" ]
limit-line  = prog ": error: too many errors emitted, stopping now"
              " (use --error-limit=0 to see all errors)" LF
```

`message` is the wording of §3.1 for the kind. The rendering MUST have
these properties, whatever the fields contain:

1. Every line starts with `<prog>: error: `, `<prog>: warning: ` or
   `>>> `, and ends with LF. There are no blank lines, also not between
   diagnostics.
2. A diagnostic is exactly one head line followed by its context lines:
   the number of lines that do not start with `>>> ` is the number of
   diagnostics (plus one for the limit line).
3. The same model value renders to the same bytes under the same style.

The text form is for people and for line-oriented tools; it is not
injective (a file may be named `a.o:(x)`), so a tool that needs the fields
uses the model, not the text (§5.7).

### 5.3 Places in the head line

The head line starts with the diagnostic's place when it has one
(`.at`): `ld: error: foo.o:(.rela.text+0x18): unsupported relocation type:
0x999`. The relocation kinds are one diagnostic per relocation and start
with its place (§3.2). `undefined-symbol` (one per key) and
`duplicate-symbol` name their places in context lines instead, one per
reference or definition.

### 5.4 Symbol names and the demangling hook

The hook is a function from a symbol-table name to `Some(shown)` or
`None`: `&(dyn Fn(&str) -> Option<String> + Sync)`. It is called for the
symbol of a diagnostic (`Kind::symbol`) and for the name of a symbol place,
once per occurrence, at rendering time only; a link that reports nothing
never calls it.

- `None`, or `Some` of the name itself: the name is printed as it is.
- `Some(shown)`: `shown` is printed where the name would be. If the name
  is the diagnostic's symbol, the first context line is
  `>>> mangled name: <name>`. Symbol places get no such line.

```text
ld: error: undefined symbol: ns::f(int, char) (undefined reference to `ns::f(int, char)')
>>> mangled name: _ZN2ns1fEic
>>> referenced by cxx.o:(.text+0x18)
```

`no_demangling` is the hook that always answers `None`; it is what
`--no-demangle` selects.

### 5.5 Control characters

Every piece of text that comes from outside the renderer (the program
name, paths, member, section and symbol names, argument text, reasons,
`what`, notes, and what the hook returns) is printed with each control
character (Unicode category Cc: U+0000 to U+001F and U+007F to U+009F)
replaced by `\u{` + its code point in lower-case hexadecimal without
leading zeros + `}`. So a line feed is `\u{a}` and an escape is `\u{1b}`.
Nothing else is changed; a backslash is not doubled.

This is what makes §5.2's properties hold for hostile input: a symbol name
in an object file cannot start a line that looks like another diagnostic,
and cannot send a terminal an escape sequence.

### 5.6 Color

With `color` false the output contains no escape sequence. With `color`
true the severity word and its colon are wrapped, and nothing else is:

| | bytes before | text | bytes after |
|---|---|---|---|
| error (and the limit line) | `ESC [ 0 ; 3 1 m` | `error:` | `ESC [ 0 m` |
| warning | `ESC [ 0 ; 3 5 m` | `warning:` | `ESC [ 0 m` |

so ``ld: \x1b[0;31merror:\x1b[0m undefined symbol: f (undefined reference to `f')``. The sequences are
lld's (red, magenta). Removing them from a colored rendering MUST give the
uncolored rendering.

### 5.7 Where another renderer attaches

`Report::diagnostics()` and the accessors of §2.5 expose everything the
text is made from: severity, id, the kind's fields, the refusal's class id
and name, places by form and field, relations. A machine format (JSON,
SARIF) is a second function over `&Report` beside `Report::render`, in this
crate, selected by a field added to the command line's options; it needs no
change to the model and none to the callers. It is not part of WP-007.

## 6. Collection

### 6.1 Keys

A `Collector` is shared by reference among the threads of a link. A thread
reports with `emit(key, diagnostic)`, and `Key` is

| field | type | meaning |
|---|---|---|
| `phase` | `u16` | ordinal of the pipeline phase, in pipeline order |
| `unit` | `u32` | the unit of work inside the phase: an input file's ordinal in command-line order, an argument's index |
| `seq` | `u64` | position inside the unit: a byte offset, a table index, or a count kept by the unit's own sequential work |

Keys compare by `phase`, then `unit`, then `seq`. `spec/model.md` assigns
the phase ordinals and says, per phase, what `unit` and `seq` are.

Obligations of the caller, which the collector cannot check:

- The key and the diagnostic MUST be functions of the inputs, their order
  and the options: never of a thread's identity, of timing, of an address,
  or of the iteration order of a hash table.
- *Whether* a diagnostic is emitted MUST be such a function too. In
  particular a parallel phase does not stop emitting because another thread
  has reported an error or because the limit is reached: `has_errors` is
  polled between phases and between the sequential steps of a phase (§7).
- Keys SHOULD be distinct for distinct diagnostics; §6.2 orders ties, at
  the cost of rendering them.

### 6.2 Order

`finish` yields the diagnostics in **report order**, which MUST depend only
on the multiset of `(key, diagnostic)` pairs emitted, not on which thread
emitted them or when:

1. by key (§6.1);
2. then by the diagnostic's text under `Style::plain` (§5; no color,
   identity hook; the program name is common and does not matter),
   compared as bytes (so at one key an error precedes a warning);
3. then, for diagnostics with the same key and the same text and different
   fields, by a total order on the fields that the implementation fixes.

Diagnostics equal in all three are the same value. Both are kept:
duplicates are not merged.

### 6.3 Warning switches

`Policy` carries the switches; the command line sets them.

- `no_warnings` (`--no-warnings`, `-w`): a warning is dropped at `emit`,
  not counted and not shown.
- `fatal_warnings` (`--fatal-warnings`): a warning becomes an error at
  `emit`: it is rendered as `error:`, counts toward the error limit, and
  fails the link. Its kind and id do not change.
- With both set, `no_warnings` wins: warnings are dropped and the link is
  not failed by them. (GNU ld 2.44 and lld 19 do this; §8.1.)

### 6.4 The error limit

`Policy::error_limit` is the number of errors a report shows; `0` means
all. The default is `DEFAULT_ERROR_LIMIT`, 20, as lld's `--error-limit`.

With limit `N > 0` and `E` errors emitted (promoted warnings included):

- `E <= N`: the report holds every diagnostic.
- `E > N`: the report holds the diagnostics in report order up to and
  including the `N`th error, and nothing after it (later warnings are cut
  too); `Report::truncated()` is true and the text ends with the limit line
  of §5.2. `Report::error_count()` is still `E`.

Warnings do not count toward the limit and are not limited.

The limit line's wording is lld's, including the option name: it tells the
user the switch that shows the rest. It is not a diagnostic (it has no id
and is not in `diagnostics()`).

Because of §6.1 the errors shown are the first `N` in report order, the
same on every run. The collector MUST NOT hold more than a number of
errors proportional to `N` (the implementation: at most `2N + 64`), so a
link with a million undefined symbols does not pay for a million
diagnostics.

### 6.5 The report

`finish(self) -> Report`. A `Report` gives

- `diagnostics()`: what to show, in report order;
- `failed()`: whether any error was emitted (§7);
- `error_count()`, `warning_count()`: how many were emitted, shown or not
  (a promoted warning is an error; a dropped warning is neither);
- `truncated()`: whether §6.4 cut it;
- `render(&Style) -> String`: the text of §5.2, for standard error.

A link that succeeds with warnings has a report too; the caller prints it
and exits with success.

### 6.6 Threads

`Collector` MUST be `Send` and `Sync`; `emit` and `has_errors` take
`&self`, so scoped threads share `&Collector`. `Report` is `Send` and
`Sync`. `emit` takes a lock; it is on no path of a link that reports
nothing.

## 7. Failure

An error means the link fails and leaves no output file.

- `Collector::has_errors()` MUST be true from the moment `emit` has
  returned for an error (or for a warning under `fatal_warnings`), and
  false while only warnings have been reported. It is one atomic load, so
  phases poll it freely.
- The pipeline polls it after each phase and stops at the first poll that
  is true: later phases would report consequences, not causes.
- The phase that creates the output file (`spec/image.md`) creates it only
  when `has_errors()` is false, and removes it if an error is reported
  afterwards. A failure at any phase after the command line also removes
  an existing regular output file that is not an input (`spec/image.md`
  I-66).
- `Report::failed()` is true exactly when an error was emitted.
- The facade (`mortar::link`) returns `Outcome::Failed(report)` for a
  failed link; the `mortar` executable then writes `report.render(..)` to
  standard error, nothing to standard output, and exits with status 1.

## 8. Survey and choices

### 8.1 What the reference linkers print

GNU ld 2.44, lld 19.1.7 and mold 2.37.1 on AArch64 Linux, objects built
with `gcc -c` (no `-g` unless said), each linker run as `<linker> <inputs>
-o out`. Lines are as printed, with `/abs` for the build directory and
`...` where a line is cut short; ` / ` separates the lines of one
diagnostic.

| case | GNU ld | lld | mold |
|---|---|---|---|
| undefined symbol | ``ld.bfd: undef.o: in function `_start':`` / ``undef.c:(.text+0xc): undefined reference to `missing'`` | `ld.lld: error: undefined symbol: missing` / `>>> referenced by undef.c` / `>>>               undef.o:(_start)` | as lld, with `mold:` |
| the same, object built with `-g` | `ld.bfd: /abs/undef.c:3:(.text+0xc): undefined reference to ...` | `>>> referenced by undef.c:3 (/abs/undef.c:3)` | as without `-g` |
| the same, in an archive member | `ld.bfd: libundef.a(undef.o): in function ...` | `>>>               undef.o:(_start) in archive libundef.a` | `>>>               libundef.a(undef.o):(_start)` |
| undefined C++ symbol | ``undefined reference to `ns::f(int, char)'`` | `error: undefined symbol: ns::f(int, char)` | as lld |
| the same, `--no-demangle` | ``undefined reference to `_ZN2ns1fEic'`` | `error: undefined symbol: _ZN2ns1fEic` | as lld |
| duplicate symbol | ``dup2.c:(.text+0x0): multiple definition of `twice'; dup1.o:dup1.c:(.text+0x0): first defined here`` | `ld.lld: error: duplicate symbol: twice` / `>>> defined at dup1.c` / `>>>            dup1.o:(twice)` / `>>> defined at dup2.c` / `>>>            dup2.o:(.text+0x0)` | `mold: error: duplicate symbol: dup2.o: dup1.o: twice` |
| unknown option | `ld.bfd: unrecognized option '--frobnicate'` / `ld.bfd: use the --help option for usage information` | `ld.lld: error: unknown argument '--frobnicate'` | `mold: fatal: unknown command line option: --frobnicate` |
| unknown `-z` keyword | `ld.bfd: warning: -z frobnicate ignored` (status 0) | `ld.lld: warning: unknown -z value: frobnicate` (status 0) | `mold: warning: unknown command line option: -z frobnicate` (status 0) |
| option without its value | `ld.bfd: unrecognized option '-o'` | `ld.lld: error: -o: missing argument` | `mold: fatal: option -o: argument missing` |
| bad option value | ``ld.bfd: invalid hash style `frob'`` | `ld.lld: error: unknown --hash-style: frob`; `ld.lld: error: --error-limit=x: number expected, but got 'x'` | `mold: fatal: invalid --hash-style argument: frob` |
| unknown emulation | `ld.bfd: unrecognised emulation mode: elf_frob` / `Supported emulations: ...` | `ld.lld: error: unknown emulation: elf_frob` | `mold: fatal: unknown -m argument: elf_frob` |
| no inputs | `ld.bfd: no input files` | `ld.lld: error: no input files` | `mold: fatal: -m option is missing` |
| missing file | `ld.bfd: cannot find nosuch.o: No such file or directory` | `ld.lld: error: cannot open nosuch.o: No such file or directory` | `mold: fatal: cannot open nosuch.o: No such file or directory` |
| missing library | `ld.bfd: cannot find -lnosuch: No such file or directory` | `ld.lld: error: unable to find library -lnosuch` | `mold: fatal: library not found: nosuch` |
| output cannot be created | `ld.bfd: cannot open output file /nonexistent/dir/out: No such file or directory` | `ld.lld: error: cannot open output file /nonexistent/dir/out: No such file or directory` | `mold: fatal: cannot open /nonexistent/dir/.out.7001: No such file or directory` |
| a text file that is not a script | `ld.bfd:garbage.o: file format not recognized; treating as linker script` / `ld.bfd:garbage.o:1: syntax error` | `ld.lld: error: garbage.o:1: unknown directive: not` / `>>> not an object` / `>>> ^` | `mold: fatal: garbage.o:1: not an object` / `                          ^ unknown linker script token` |
| an object cut at 100 bytes | `trunc.o: file not recognized: file format not recognized` | `ld.lld: error: trunc.o: section header table goes past the end of the file: e_shoff = 0x2e8` | `mold: fatal: trunc.o: e_shoff or e_shnum corrupted: 100 12` |
| a broken archive | as the text file | `ld.lld: error: bad.a: failed to parse archive: truncated or malformed archive (...)` | nothing: status 0 |
| an executable as input | `ld.bfd: cannot use executable file 'exe.out' as input to a link` (after reading its symbols) | `ld.lld: error: exe.out: unknown file type` | `mold: fatal: exe.out: unknown file type` |
| an object of another machine | `ld.bfd: x64.o: Relocations in generic ELF (EM: 62)` / `ld.bfd: x64.o: error adding symbols: file in wrong format` | `ld.lld: error: x64.o is incompatible with dup1.o` | `mold: fatal: x64.o: incompatible file type: arm64 is expected but got x86_64` |
| unknown relocation type (0x999) | `ld.bfd: badrel.o: unsupported relocation type 0x999` (five lines, one of them `can not size stub section: bad value`) | `ld.lld: error: badrel.o:(.text+0x0): unknown relocation (2457) against symbol foo` | `mold: error: badrel.o:(.text): unknown relocation: unknown (0x999)` |
| unknown section type, unknown section flag, unknown symbol type | nothing: status 0 | nothing: status 0 | nothing: status 0 |
| unknown script command | `ld.bfd:bad.ld:1: syntax error` | `ld.lld: error: bad.ld:1: unknown directive: FROB` / `>>> SECTIONS { .text : { *(.text) } } FROB(1)` / `>>>                                   ^` | `mold: fatal: bad.ld:1: SECTIONS ... FROB(1)` / `                       ^ unknown linker script token` |
| 25 undefined symbols | 25 diagnostics | 20, then `ld.lld: error: too many errors emitted, stopping now (use --error-limit=0 to see all errors)` | 25, not in the order of the references (`u7`, `u18`, `u10`, ...) |
| exactly as many errors as the limit | - | all of them, no limit line | - |
| `--warn-unresolved-symbols` | ``undef.c:(.text+0xc): warning: undefined reference to `missing'`` | `ld.lld: warning: undefined symbol: missing` | `mold: warning: undefined symbol: missing` |
| the same, `--fatal-warnings` | still `warning:`, status 1 | `ld.lld: error: undefined symbol: missing`, status 1 | `mold: error: ...`, status 1 |
| the same, `--fatal-warnings -w` | nothing, status 0 | nothing, status 0 | (`-w` is not an option of mold's) |
| separation of diagnostics | none | a blank line before a diagnostic that follows one with context lines | none |
| color (`--color-diagnostics=always`) | not an option | `ld.lld: ESC[0;31merror: ESC[0m...`; warnings `ESC[0;35m` | `mold: ESC[0;1;31merror:ESC[0m ...` |

### 8.2 Choices

The shape is lld's and mold's: `<prog>: error: <message>` and `>>> `
context lines. One head line per diagnostic can be counted and searched;
the continuation lines are a form that build tools and IDEs already meet
from lld and mold; and no message needs debug information to be formed
(GNU ld's `file.c:(.text+0xc)` prefix reads the object's `STT_FILE` symbol
and, with `-g`, its line table).

The wording is lld's wherever lld has a wording that fits, because the
shape is: a tool that matches `error: duplicate symbol: ` was written
against lld or mold. Where GNU ld has a phrase for the condition, the
message also contains that phrase (§8.3), which wins over lld's wording
when both cannot be had. The table of §3.1 names the source of each. Where
mortar differs from lld:

| what | lld | mortar | why |
|---|---|---|---|
| place of an archive member | `y.o:(.text+0x10) in archive libx.a` | `libx.a(y.o):(.text+0x10)` | One form for a file wherever it appears (§2.3). `archive(member)` is GNU ld's and mold's form and keeps the place a single token. |
| undefined symbol | `error: undefined symbol: missing`, then the references as context | ``error: undefined symbol: missing (undefined reference to `missing')``, then the references as context | lld's shape (one diagnostic per key, `spec/resolve.md` §5.3), so that a key referenced a thousand times is one line and the error limit counts keys; GNU ld's phrase inside it, by the policy of §8.3. A search for lld's `undefined symbol: ` and one for GNU ld's ``undefined reference to `missing'`` both match. |
| context lines | two lines per reference (`>>> referenced by undef.c` / `>>>               undef.o:(_start)`), the first from the `STT_FILE` symbol or the line table | one line per place: `>>> referenced by undef.o:(.text+0xc)` | Every `>>> ` line is self-contained and has one grammar; no alignment by spaces; no dependence on `STT_FILE` or DWARF. Source positions are a planned place form (§9). |
| duplicate symbol | `>>> defined at dup1.c` / `>>>            dup1.o:(twice)` | `>>> defined in dup1.o:(.text+0x0)` | `defined in` for a place in a file; `defined at` is kept for the source-position form of §9. |
| blank line between diagnostics | after a diagnostic with context lines | never | Property 1 of §5.2: every line is a head line or a context line. mold prints none either. |
| demangled symbol | the demangled name only | the demangled name, then `>>> mangled name: _ZN2ns1fEic` | The mangled name is what `nm`, a version script and the other linkers' messages are searched for; without the line the user links again with `--no-demangle`. An extra `>>> ` line is ignored by tools that read lld's (lld prints others itself: `>>> did you mean: helper_function`). |
| bad option value | several wordings (`unknown --hash-style: frob`, `--error-limit=x: number expected, but got 'x'`) | `--hash-style: invalid value 'frob'`, detail in a note | One wording for one kind, in the form of `missing-argument` (the option first). No two of the three linkers agree here, so no tool can rely on one. |
| unknown `-z` keyword, unknown section type or flag | a warning, or nothing | `refused`: an error | Principle 1. Whether a given keyword is refused or accepted without effect is `spec/cli.md`'s. |
| refusal | `unknown relocation (2457) against symbol foo`, `unknown emulation: x`, `unknown directive: X`, each its own wording | `unsupported <noun>: <name>` | One wording for every refusal, so that "what does mortar not implement here" is one search (`: unsupported `), and "unsupported" is true both of what nobody defines and of what mortar has not implemented yet. GNU ld says `unsupported relocation type 0x999` and `unsupported option: -z frobnicate`. |
| control characters in names | printed raw | `\u{..}` (§5.5) | A name from an input file must not be able to forge a line or drive a terminal. |
| color | `ESC[0;31m` + `error: ` + `ESC[0m` (the space is inside) | `ESC[0;31m` + `error:` + `ESC[0m` + space | The same colors; the colored span is the token. |
| `--fatal-warnings` | `error:` | `error:` (as lld and mold; GNU ld prints `warning:` and fails) | A failed link whose log has no `error:` line misleads people and the tools that count them. |
| error limit | stops the link at the limit | every phase that started reports fully; the report shows the first N in report order | Determinism (§6.1): which N errors are shown must not depend on scheduling. The memory bound of §6.4 keeps the cost of the rest small. |

### 8.3 The wording policy

Where GNU ld has a phrase for a condition that configure scripts and build
tools match, mortar's message for the same condition **contains that
phrase verbatim** (the operator's ruling, 2026-11-14). The head line stays
mortar's (`ld: error: ...`, §5.1); the phrase is inside the message, so an
unanchored search for it matches mortar's output as it matches GNU ld's.
Where lld's wording fits too, the message is lld's with GNU ld's phrase
after it (`undefined-symbol`, `duplicate-symbol`), so both searches match.

| GNU ld phrase | kinds that carry it | how |
|---|---|---|
| ``undefined reference to `{name}'`` | `undefined-symbol` | ``undefined symbol: {name} (undefined reference to `{name}')`` |
| ``multiple definition of `{name}'`` | `duplicate-symbol` | ``duplicate symbol: {name} (multiple definition of `{name}')`` |
| `cannot find -l{name}` (and `cannot find -l:{file}`) | `library-not-found` | the whole message: `cannot find -l{name}: No such file or directory` |
| `skipping incompatible {path} when searching for {search}` | `skipped-incompatible` | the whole message |
| `file not recognized: file format not recognized` | `unknown-file-type`; `refused` of a format GNU ld does not recognize either (`not_recognized`); `cannot-open-file` for a directory (`file format not recognized` in its reason) | `unknown-file-type`: the whole message, `.at` the file. `refused`: after its own text, `unsupported file format: Mach-O; file not recognized: file format not recognized` (`Diagnostic::refused_unrecognized_format`). A directory: the reason `Is a directory; file format not recognized` (`spec/input.md` §13) |
| `No such file or directory` | `cannot-open-file` (the reason, `strerror(ENOENT)`, §3.2), `library-not-found` | the reason; the end of `library-not-found`'s message |
| `attempted static link of dynamic object` | `static-link-of-shared-object` | the whole message |
| `needed by {path}, not found (try using -rpath or -rpath-link)` | `needed-not-found` | the whole message |
| `cannot find entry symbol` | `entry-not-found` | the whole message (GNU ld's line goes on with `; defaulting to …`, which mortar cannot promise: the address is layout's) |
| `relocation truncated to fit: {r_type} against` | `relocation-out-of-range` | the start of the message, then lld's value and bounds (`spec/x64.md` X-34) |

Every kind in the list holds the phrase; `tests/kinds.rs` checks each
message whole, and `tests/kinds.rs` `wording_policy_phrases` checks the
table: for each row, a message of each kind it names contains the phrase.
A new condition GNU ld has a phrase for gets a row here with its kind.

`library-not-found` was lld's `unable to find library -l<x>` before this
policy; the argument for lld's wording (a tool reading `ld: error:` lines
was written against lld) is answered by the head line, which stays lld's
shape.

## 9. Planned extensions

Recorded so that the model is not grown by accident; none is implemented.

- **Source positions.** A place form `file.c:12` (and `file.c:12:7`) found
  through the line table of the referencing object, rendered as its own
  context line (`>>> referenced at file.c:12`, `>>> defined at ...`) beside
  the section place, never instead of it. It needs a DWARF line-table
  reader and is looked up only when a diagnostic is rendered.
- **The enclosing function** of a section offset (lld's `foo.o:(main)`,
  GNU ld's ``in function `main'``): the symbol place exists; choosing it
  needs the symbol table at the point of the diagnostic
  (`spec/resolve.md`).
- **A byte offset in a file** (`foo.o+0x40`) for format errors outside any
  section.
- **A machine format** (§5.7).
- **Asking before building**: a query on the collector that tells a phase
  an error with a given key would be cut by the limit, so that the phase
  need not build its context lines.
- **Demangling** (Itanium C++, Rust v0) behind the hook of §5.4, in this
  crate, with `--demangle`/`--no-demangle` in `spec/cli.md`.

## 10. API

```rust
pub enum Severity { Warning, Error }
#[non_exhaustive] pub enum RefusalClass { Option, OptionValue, Emulation, FileFormat, Machine,
    ObjectType, SectionType, SectionFlag, RelocationType, SymbolType, SymbolBinding,
    CompressionType, ScriptCommand, Feature }
impl RefusalClass { pub const ALL: [Self; 14]; pub const fn id(self) -> &'static str;
    pub const fn noun(self) -> &'static str; }
#[non_exhaustive] pub enum Kind { /* §3.1 */ }
impl Kind { pub const IDS: [&'static str; 45]; pub const fn id(&self) -> &'static str;
    pub fn symbol(&self) -> Option<&str>; }
#[non_exhaustive] pub enum IncompatibleReason { RecompileFpic, RecompileFpie, ReadOnlySection,
    AddendMustBeZero, NotThreadLocal, ThreadLocal, LocalExecInShared, InstructionMismatch,
    NonAlloc }
impl IncompatibleReason { pub const ALL: [Self; 9]; pub const fn text(self) -> &'static str; }

pub struct FileRef { /* path, member */ }
impl FileRef { pub fn new(path: impl Into<String>) -> Self;
    pub fn in_archive(archive: impl Into<String>, member: impl Into<String>) -> Self;
    pub fn from_path(path: &Path) -> Self;
    pub fn path(&self) -> &str; pub fn member(&self) -> Option<&str>; }
#[non_exhaustive] pub enum Place { File(FileRef),
    Section { file: FileRef, section: String, offset: u64 },
    Symbol { file: FileRef, name: String }, Argument(String),
    Script { path: String, line: u32, column: u32 } }
impl Place { pub fn file(path: impl Into<String>) -> Self;
    pub fn section(file: FileRef, section: impl Into<String>, offset: u64) -> Self;
    pub fn symbol(file: FileRef, name: impl Into<String>) -> Self;
    pub fn argument(text: impl Into<String>) -> Self;
    pub fn script(path: impl Into<String>, line: u32, column: u32) -> Self; }
impl From<FileRef> for Place;
#[non_exhaustive] pub enum Relation { DefinedIn, In, NamedBy, ReferencedBy }
impl Relation { pub const ALL: [Self; 4]; pub const fn phrase(self) -> &'static str; }
pub enum Context { At(Relation, Place), Note(String) }

pub struct Diagnostic { /* severity, kind, place, context */ }
impl Diagnostic {
    pub fn new(severity: Severity, kind: Kind) -> Self;
    pub fn error(kind: Kind) -> Self;
    pub fn warning(kind: Kind) -> Self;
    pub fn refused(class: RefusalClass, name: impl Into<String>) -> Self;
    pub fn refused_unrecognized_format(name: impl Into<String>) -> Self;   // §8.3
    pub fn at(self, place: impl Into<Place>) -> Self;
    pub fn with(self, relation: Relation, place: impl Into<Place>) -> Self;
    pub fn note(self, text: impl Into<String>) -> Self;
    pub fn severity(&self) -> Severity; pub fn kind(&self) -> &Kind;
    pub fn id(&self) -> &'static str; pub fn refusal(&self) -> Option<(RefusalClass, &str)>;
    pub fn place(&self) -> Option<&Place>; pub fn context(&self) -> &[Context];
    pub fn render(&self, style: &Style<'_>) -> String;
}

pub type Demangle<'a> = &'a (dyn Fn(&str) -> Option<String> + Sync);
pub fn no_demangling(name: &str) -> Option<String>;
pub struct Style<'a> { pub prog: &'a str, pub color: bool, pub demangle: Demangle<'a> }
impl<'a> Style<'a> { pub fn plain(prog: &'a str) -> Self; }
pub fn os_reason(error: &std::io::Error) -> String;
pub const NOT_RECOGNIZED: &str = "file not recognized: file format not recognized";   // §8.3

pub const DEFAULT_ERROR_LIMIT: usize = 20;
pub struct Key { pub phase: u16, pub unit: u32, pub seq: u64 }
impl Key { pub const fn new(phase: u16, unit: u32, seq: u64) -> Self; }
pub struct Policy { pub error_limit: usize, pub fatal_warnings: bool, pub no_warnings: bool }
pub struct Collector { /* .. */ }
impl Collector {
    pub fn new(policy: Policy) -> Self;
    pub fn policy(&self) -> Policy;
    pub fn emit(&self, key: Key, diagnostic: Diagnostic);
    pub fn has_errors(&self) -> bool;
    pub fn finish(self) -> Report;
}
pub struct Report { /* .. */ }
impl Report {
    pub fn failed(&self) -> bool;
    pub fn diagnostics(&self) -> &[Diagnostic];
    pub fn error_count(&self) -> usize; pub fn warning_count(&self) -> usize;
    pub fn truncated(&self) -> bool;
    pub fn render(&self, style: &Style<'_>) -> String;
}
```

The model types derive `Debug`, `Clone`, `PartialEq`, `Eq`, `PartialOrd`,
`Ord` and `Hash`; `Policy::default()` is limit 20 and both switches off;
`Collector::default()` is `Collector::new(Policy::default())`.

## 11. Tests

Files are under `crates/mortar_diag/` unless a crate is named.

| section | what | test |
|---|---|---|
| §2.1 | severity order and words | `tests/model.rs` `severity_words_and_order` |
| §2.2, §3.1 | ids are kebab-case, unique, and the table of §3.1 is `Kind::IDS`; every variant's id is in the list | `tests/spec_tables.rs` `kind_ids_match_the_spec_table`, `src/model.rs` `every_variant_is_in_ids` |
| §2.3 | each place form, exact text | `tests/render.rs` `place_forms` |
| §2.3 | non-UTF-8 path | `tests/render.rs` `path_that_is_not_utf8` |
| §2.4 | each relation, exact text, order of context lines; the table | `tests/render.rs` `relations_and_notes_in_the_order_given`, `tests/spec_tables.rs` `relations_match_the_spec_table` |
| §2.5 | builder and accessors; `Send`/`Sync`/`'static` | `tests/model.rs` `builder_and_accessors`, `types_are_send_and_sync` |
| §3.1 | every kind, exact text, as error and as warning | `tests/kinds.rs` (one test per kind) |
| §3.2 | `os_reason` | `tests/kinds.rs` `os_reason_is_the_system_message` |
| §3.2 | `undefined-symbol` one per key with its references as context; the relocation kinds' fields; every reason's words and the table | `tests/kinds.rs` `undefined_symbol`, `relocation_*`, `tests/spec_tables.rs` `incompatible_reasons_match_the_spec_table` |
| §2.1, §3.2 | `internal-error` is always an error | `tests/kinds.rs` `internal_error` |
| §3.3 | symbol fields reach the hook | `tests/kinds.rs` `symbol_kinds_report_their_symbol` |
| §4, §4.1 | every class: id, noun, exact text; the table | `tests/refusal.rs` `every_class_renders_uniformly`, `tests/spec_tables.rs` `refusal_classes_match_the_spec_table` |
| §4.2 | class and name are readable by tests; place in the head | `tests/refusal.rs` `class_and_name_are_data`, `refusal_with_a_place` |
| §8.3 | every phrase of the wording policy is in the message of each kind it names; a refusal of a format GNU ld does not recognize carries the phrase; a directory's reason | `tests/kinds.rs` `wording_policy_phrases`, `cannot_open_file_for_a_directory`, `tests/refusal.rs` `a_format_gnu_ld_does_not_recognize_carries_its_phrase` |
| §4.3 | a refusal is always an error, also under `-w` | `tests/refusal.rs` `a_refusal_is_never_a_warning`, `no_warnings_does_not_hide_a_refusal` |
| §5.2 | line properties 1 to 3 over every sample, hostile fields included | `tests/render.rs` `every_line_is_a_head_or_a_context_line` |
| §5.1, §5.3 | the program name; place in the head line | `tests/render.rs` `head_line_place`; `crates/mortar/src/lib.rs` `program_name_is_the_basename`, `crates/mortar/tests/cli.rs` `diagnostics_start_with_the_invoked_name` |
| §5.4 | hook: `None`, same name, another name; symbol places; called only at rendering | `tests/render.rs` `demangling_*` |
| §5.5 | control characters in every kind of field | `tests/render.rs` `control_characters_are_made_visible` |
| §5.6 | color on and off, exact bytes, stripping | `tests/render.rs` `color_*` |
| §6.1, §6.2 | order by key; ties by text, then fields; duplicates kept | `tests/collect.rs` `report_order_*` |
| §6.1 | the caller's obligations | not checkable here: the determinism gate of `docs/VALIDATION.md` (one thread against many) checks them on whole links |
| §6.2, §6.6 | the same report from one thread and from two to eight, in any emission order | `tests/collect.rs` `order_does_not_depend_on_threads` |
| §6.3 | `no_warnings`, `fatal_warnings`, both | `tests/collect.rs` `no_warnings_*`, `fatal_warnings_*` |
| §6.4 | limit: below, at, above, `0`, default, warnings around the cut, promoted warnings count, threads | `tests/collect.rs` `limit_*` |
| §6.4 | the bound on errors held | `src/collect.rs` `limit_bounds_the_errors_held`, `no_limit_holds_everything` |
| §6.5 | counts, empty report, warnings only, the report's text | `tests/collect.rs` `report_of_nothing_and_of_warnings_only`, `report_renders_each_diagnostic_with_the_style` |
| §7 | `has_errors` | `tests/collect.rs` `has_errors_*` |
| §7 | the facade and the executable: standard error only, status 1, no output file | `crates/mortar/src/lib.rs` `a_link_is_refused_not_faked`, `crates/mortar/tests/cli.rs` `a_failed_link_reports_on_stderr_and_leaves_no_output` |
