# `mortar_input`: from the plan to the inputs

Status: **draft** (WP-021, stage 1: the design; the code follows WP-020).

`mortar_input` turns the `Plan` (`spec/cli.md` §3) into `Inputs`
(`spec/model.md` §3.2) and the **store** that owns the bytes of every file
read. It finds files (search paths, `-l`, the sysroot), reads them, decides
what each one is from its bytes, expands the linker scripts that stand in
input position, opens archives, numbers files, interns symbol keys, and
reports every problem of these steps. It resolves no symbol and decides
no extraction (`spec/resolve.md`).

The behavior below was established by experiment on this machine (AArch64
host) with GNU ld 2.44 (native `ld.bfd`; the x86-64 and i386 cross
`ld.bfd`), lld 22.1.8 (`ld.lld-22`) and mold 2.37.1, on objects built by
GCC 14 and Clang 22. Each claim about a reference cites an experiment
`[En]` of `spec/landed/input/WP-021-spec.md`, which gives its command
lines and outputs; a claim without one is marked *not verified*. Where
the three agree, mortar agrees. Where they differ, the section says which
one mortar follows and why; §15 collects the differences.

Specs this one leans on, and does not restate: `spec/model.md` (ids,
tables, determinism, keys), `spec/cli.md` §3 (the `Plan`),
`spec/script.md` §2.3-§2.8 (implicit-script commands and how their file
names resolve), `spec/archive.md` (the archive reader), `spec/elf.md`
(the object reader; §6, shared objects, is WP-006's), `spec/diag.md` (the
form of diagnostics and refusals).

## 1. Scope and interface

In scope: every `Plan` input (`Path`, `Library`, `LibraryFile`,
`JustSymbols`), the implicit scripts they lead to, archives and their
members, shared objects and (M2) the shared objects their `DT_NEEDED`
entries name; the target when `-m` is absent; the store; `files`,
`sym_map`, `symbols`, `comdats`, `archives` and `target` of `Inputs`.

Out of scope: `-T`/`-dT` scripts (refused until M4, `spec/cli.md` §4),
version scripts and dynamic lists (`spec/script.md`, later stages),
which archive members and shared objects take part in the link
(`spec/resolve.md`), section classification (`spec/model.md` §3.3; it may
run here per file, I-file), `-z` page and stack sizes (`spec/layout.md`;
the forms the references accept are recorded in the landed file, [E24]).

Shape (informative):

```rust
pub fn load<'s>(plan: &Plan, env: &LoadEnv, store: &'s mut Store, diag: &Collector)
    -> Option<Inputs<'s>>;              // None: errors were emitted (spec/diag.md §7)
pub struct LoadEnv { pub cwd: PathBuf, pub threads: Threads }   // nothing else: §4.6
pub struct Store { /* §11 */ }
impl Store {
    pub fn file(&self, f: FileId) -> &[u8];                     // the bytes of a loaded file
    pub fn range(&self, f: FileId, r: Range<u64>) -> &[u8];
}
```

## 2. What a file is

### 2.1 Sniffing

What a file is, is decided from its bytes, never from its name or
extension, in this order; the first rule that matches decides:

| # | bytes | kind | §, or the diagnostic |
|---|---|---|---|
| 1 | length 0 | an empty linker script: contributes nothing | §2.3 |
| 2 | `\x7fELF` | ELF: `mortar_elf::FileHeader::parse` (elf.md §3.1 steps 1-5), then §2.2 | its errors: §2.4 |
| 3 | `!<arch>\n`, `!<thin>\n` | archive, regular or thin (`Archive::sniff`) | §7 |
| 4 | `BC\xC0\xDE`; `\xDE\xC0\x17\x0B` (the bitcode wrapper, 0x0B17C0DE little-endian) | LLVM bitcode | refused, `file-format` `LLVM bitcode` |
| 5 | `\xFE\xED\xFA\xCE`, `\xFE\xED\xFA\xCF`, `\xCE\xFA\xED\xFE`, `\xCF\xFA\xED\xFE`, `\xCA\xFE\xBA\xBE` | Mach-O (thin or universal) | refused, `file-format` `Mach-O` |
| 6 | `\0asm` | WebAssembly | refused, `file-format` `WebAssembly` |
| 7 | at least 20 bytes, starting `\x4C\x01`, `\x64\x86` or `\x64\xAA` | COFF (i386, AMD64, ARM64 machine fields) | refused, `file-format` `COFF` |
| 8 | `mortar_script::looks_like_script` is true | linker script | §5 |
| 9 | anything else | — | `unknown-file-type` at the file |

The references: GNU ld says `file format not recognized` for rows 4-7 and
9; lld and mold say `unknown file type` for rows 5-7 and an executable;
lld links LLVM bitcode itself and mold refuses it naming LTO [E1].
mortar names what it can name (`spec/diag.md` §3.2: a format it can name
and does not read is a refusal). Rules 4-7 come before rule 8 so that an
object of another format is reported by name and never as a script syntax
error. A text file can only be taken for COFF if it starts with `d`
followed by byte 0x86 or 0xAA, which no script does.

Rows 2-7 are decided on the first bytes only; row 8 reads the whole file
(script.md §3.4).

### 2.2 ELF files

After `FileHeader::parse`, `e_type` decides:

| `e_type` | what | rule |
|---|---|---|
| `ET_REL` | relocatable object | §6, after the LTO check below |
| `ET_DYN` | shared object | §8.1 (a position-independent executable is refused there) |
| `ET_EXEC` | executable | refused, `object-type` `ET_EXEC` (GNU ld: `cannot use executable file 'exe' as input to a link`, after reading its symbols; lld, mold: `unknown file type` [E1]) |
| `ET_CORE` | core file | refused, `object-type` `ET_CORE` (all three: not recognized [E1]) |
| other | — | refused, `object-type` `ET_NONE` or the value in hex (elf.md §3.1 step 8 names) |

**LTO objects.** A relocatable object is a GCC LTO object without code
("slim") when its symbol table has an entry named `__gnu_lto_slim` (GCC
writes a common symbol of that name; [E1]); it is refused, `file-format`
`GCC LTO object`, with a note that mortar does not perform link-time
optimization and that `-ffat-lto-objects` objects link without it. An
object that has `.gnu.lto_*` sections and no `__gnu_lto_slim` is a "fat"
LTO object: it is an ordinary object (its `.gnu.lto_*` sections carry
`SHF_EXCLUDE` and are dropped), as all three references link it [E1].
The references on a slim object: GNU ld without a plugin warns `plugin
needed to handle lto object` and links *without it*, exit 0; lld links it
silently as an object without code; mold refuses [E1]. Both successes are
links that are missing code; mortar refuses. (The `-plugin` options GCC
passes on every line have no effect, `spec/cli.md` §3.11.)

### 2.3 Empty files

An empty file in input position is an empty linker script and
contributes nothing (script.md §3.4; GNU ld and lld accept it, mold says
`unknown file type` [E1]). An empty *archive member* is not a script: §7.3.

### 2.4 Header errors

`FileHeader::parse` and the readers' errors become diagnostics:

| reader error (elf.md §3.1) | diagnostic |
|---|---|
| `BigEndian` | refused, `file-format` `big-endian ELF` |
| `UnsupportedMachine` | refused, `machine`, the `EM_` name or the number in hex |
| `UnsupportedClass` (x32, ILP32, ELF64 `EM_386`) | refused, `file-format` `x32`, `ILP32`, or `ELF64 EM_386` |
| `InvalidClass`, `InvalidEncoding`, `TruncatedHeader`, `UnsupportedVersion`, every later structural error | `malformed-input` at the file, the reader's text |

A machine mortar supports but that is not the link's target is §3.3, not
this table.

### 2.5 Files that are not regular files

The path is opened and `fstat`ed. A directory is `cannot-open-file` with
the reason `Is a directory` (lld's text [E1]; GNU ld: `cannot find dir:
file format not recognized`; mold: `mmap failed: No such device`). Any
other file type that is not a regular file (FIFO, device, socket) is
refused, `feature` `input that is not a regular file`: its size is not
known before reading, which the store needs (§11), and nothing in a
build passes one. *Not verified* for the references.

### 2.6 `-b` and `--format`

The positional `format` of the input (`spec/cli.md` §3.3):

| format | rule |
|---|---|
| `Detect` (`default`, lld's `elf`) | §2.1 |
| `Named(n)`, `n` the target's BFD name (`elf64-littleaarch64`, `elf64-x86-64`, `elf32-i386`) | §2.1, and the file must be an ELF file of the target (§3.3) |
| `Named(n)`, any other name | refused, `file-format`, `n` |
| `Binary` | refused, `file-format` `binary` (GNU ld and lld read raw bytes into a data section; mold refuses [E19]): open question §18.4 |

GNU ld rejects `-b elf` (`invalid BFD target`), lld rejects BFD names and
mold implements no `-b` but `binary`'s refusal [E19].

## 3. The target

### 3.1 Which file decides

The target (machine, class, relocation form: RELA for AArch64 and x86-64,
REL with implicit addends for i386) is:

1. the target of `-m`, when the line has one (`spec/cli.md` §2.8);
2. otherwise, the target of the first **ELF file** among the line's
   `Path` inputs, in command-line order, scripts not expanded;
3. otherwise, the target of the first ELF file in load order (§10),
   scripts expanded and `-l` resolved; a search made before the target is
   known takes its first candidate, without the compatibility test of
   §4.4;
4. otherwise `no-target`, an error: `target emulation unknown: -m or at
   least one .o file required` (lld's wording; GNU ld uses its built-in
   default, mold says `-m option is missing` [E1]).

An **ELF file** for this purpose is a relocatable object, a shared object,
or an archive's first member that is a relocatable object (in archive
order, whether or not it is ever extracted). Executables, refused files
and scripts do not count.

The references [E2]: GNU ld ignores the inputs (its target is the
emulation it was built with: the native one rejects x86-64 objects with
`Relocations in generic ELF (EM: 62)`). lld takes the first ELF file in
load order, shared objects, `-l` results and lazy archive members
included. mold takes the first ELF file among the inputs named by path,
shared objects and lazy archive members included, scripts and `-l`
excluded. mortar is mold's rule (2) with lld's as the fallback (3):

- one mortar links for every target, so there is no built-in default;
- rule 2 needs no search, so the target is known before any `-l` is
  resolved and §4.4 can skip a library of the wrong machine, which is what
  `-L/usr/lib32 -L/usr/lib -lfoo main.o` needs. With lld's rule the first
  library found decides, and `ld -Lx86 -Larm -lf main.o` fails where GNU
  ld and mold succeed [E2];
- both are functions of the `Plan` and file contents.

Every real line in the census (`spec/cli.md` §6) has `-m`; rules 2-4 serve
direct `ld` users.

### 3.2 The check of target options

When the target came from rule 2 or 3, the input phase calls
`mortar_cli::check_target(plan, target)` once it is known
(`spec/cli.md` §2.9, the WP-008 follow-up) and emits its diagnostics with
the key `(20, arg, 0)`: phase 20, the argument of the option
(`spec/model.md` §3.1). The implicit scripts' `OUTPUT_FORMAT`,
`OUTPUT_ARCH` and `TARGET` (script.md §2.5) are checked at the same point.
`OUTPUT_FORMAT` never decides the target.

### 3.3 Files of another target

Every ELF file of the link (objects, every relocatable archive member,
lazy or not, shared objects) must have the target's machine and class.
One that does not is `incompatible-file` at that file, `incompatible file:
<target> is expected but got <machine>` (mold's wording), with a note
naming what set the target (`the target was set by start.o`, or `by -m
elf_x86_64`). lld and mold report it for lazy archive members too; GNU ld
says nothing about a lazy member of another machine and reports an
extracted one as `file in wrong format` [E2]. mortar reads every member
(§7.4), so it reports every one.

Exception: a library candidate of another target found by a search is
skipped (§4.4), not reported as incompatible.

### 3.4 `e_flags`

`e_flags` compatibility belongs to the architecture specs. The input phase
calls, per ELF file and in parallel, `Arch::check_input_flags(e_flags) ->
Result<(), Diagnostic>` (a file whose flags the architecture refuses), and
folds the files in `FileId` order through `Arch::merge_input_flags(acc,
e_flags, FileId) -> Result<acc, Diagnostic>` for the cross-file rules
(both named here for `spec/arch.md`; all flags are 0 for the three
targets' Linux ABIs today).

## 4. Finding files

### 4.1 Paths

A `Path` input is opened as written, relative to the working directory.
It is never searched (all three: `cannot find libq.a`, `cannot open libq.a`
with `-Ldd` naming a directory that has it [E5]). `-` is a file named `-`
(GNU ld and lld; mold rejects it as an option [E5]).

A `Path` that starts with `=` is refused, `feature` `'=' prefix on an
input path`, with a note: GNU ld reads it as inside the sysroot, lld and
mold as a file whose name starts with `=` [E6]. (`=` on `-L` and in
scripts is specified: §4.3, script.md §2.4.)

### 4.2 The library search path

The **library search path** at a point of load order is, in order:

1. the `-L`/`--library-path` directories of the whole command line, in
   argument order, wherever they appear: a `-L` written after the `-l`
   that needs it applies (all three [E3]);
2. the `SEARCH_DIR` directories of the implicit scripts expanded *before*
   that point in load order, in load order; within one script, all of its
   `SEARCH_DIR`s apply to all of its own entries (script.md §2.6).

GNU ld and lld apply a script's `SEARCH_DIR` to the command-line `-l`s
after the script, not to those before it; mold implements no
`SEARCH_DIR` [E10].

**No built-in directories.** GNU ld appends its configured directories
(`ld --verbose`: `=/usr/local/lib/aarch64-linux-gnu`,
`=/lib/aarch64-linux-gnu`, `=/usr/lib/aarch64-linux-gnu`,
`=/usr/local/lib`, `=/lib`, `=/usr/lib`, `=/usr/aarch64-linux-gnu/lib`, and
the 64/32 variants on the cross linkers) after these; lld and mold have
none [E5]. mortar has none:

- every driver in the census passes its own complete list
  (`gcc`: the GCC directory, `../../../aarch64-linux-gnu`, `/lib/<triple>`,
  `/lib`, `/usr/lib/<triple>`, `/usr/lib`; clang the same without the GCC
  `..` forms [E5]); a built-in list could only add directories the driver
  chose not to search;
- the directories GNU ld adds that the drivers do not pass
  (`/usr/local/lib*`, `/usr/<triple>/lib`) are host configuration, which
  would make a link's meaning depend on the machine it runs on;
- one mortar serves three targets and every distribution layout: there is
  no single right list.

The cost: `ld main.o -lc` without `-L` finds nothing in mortar, lld and
mold (`library-not-found` lists what was searched, §4.5). `-nostdlib`
therefore has no effect in mortar; its row is "accepted without effect"
with that reason (§17, requested change to `spec/cli.md`).

### 4.3 The sysroot

`--sysroot DIR` (the last one) is the sysroot, as written (relative to the
working directory if relative). There is no default sysroot (GNU ld's
configured one is empty here: `--print-sysroot` prints nothing [E6]).

A `-L` directory the command line marked `in_sysroot` (written `=D` or
`$SYSROOT D`, `spec/cli.md` §3.3) is:

| sysroot | `D` | directory | references [E6] |
|---|---|---|---|
| none | any | `D` | GNU ld, lld; mold finds nothing |
| `S` | starts with `/` | `S` joined with `D` (`S/lib` for `S`, `/lib`) | all three |
| `S` | does not start with `/` | `ambiguous-sysroot-path` error | GNU ld concatenates (`=lib` is `Slib`, by `-t`), lld joins (`S/lib`); mold links, which directory it used is not visible (*not verified*) |

A `$SYSROOT` prefix is GNU ld's; lld does not know it (searches a
directory literally named `$SYSROOT/lib`), mold does [E6]. mortar follows
GNU ld, which documents it (script.md §2.4 rule 2 makes the same choice).

A `-L` directory that is absolute and not marked is not prefixed (all
three [E6]: `-nostdlib --sysroot=S -L/lib -lf` does not find `S/lib/libf.a`).

Script entries follow script.md §2.4 (rules 1 and 2).

`-L ''` (an empty directory) is `invalid-option-value` at its argument:
lld searches the working directory, GNU ld and mold find nothing [E3].

### 4.4 `-l`

For `-l name` (`Library`), for each directory `D` of the search path in
order, the candidates are `D/libname.so` then `D/libname.a`, or only
`D/libname.a` when `link_static` is in effect. For `-l :file`
(`LibraryFile`) the one candidate per directory is `D/file`, whatever
`link_static` says. Shared before static **per directory** (all three
[E3]: `-Lda -Ldso -lf` takes `da/libf.a`; `-Ldso -Lda` and one directory
holding both take the `.so`). Script entries `-lname` and `-l:file`
search the same way with the state of their position (script.md §2.4
rule 4).

A candidate is examined with `stat` (following symbolic links):

| result | what happens | references [E4] |
|---|---|---|
| does not exist (`ENOENT`, `ENOTDIR`; a dangling link) | next candidate | all three |
| a regular file | read and sniffed (§2): it is **found**, unless it is skipped below | all three |
| a directory | `cannot-open-file`, `Is a directory` | lld, mold; GNU ld skips it |
| any other error (`ELOOP`, `EACCES`) | `cannot-open-file` with the reason | mold for `ELOOP`; GNU ld and lld skip it |

A candidate is never skipped silently because it could not be examined: a
broken file that shadows a good one is an error, so that fixing the
broken one cannot change which library a later link takes.

**Incompatible candidates.** When the target is known (§3.1), a candidate
that is an ELF shared object, or an archive whose first relocatable
member is an object, of another machine or class, is skipped with the
warning `skipped-incompatible`: `skipping incompatible xd/libf.a when
searching for -lf` (GNU ld's wording; GNU ld prints it as a warning:
`--fatal-warnings` fails the link). GNU ld and mold skip (mold: `skipping
incompatible file`); lld takes the first candidate and reports it as
incompatible [E2]. Multilib directory lists depend on skipping. A candidate
that is not a library (garbage, a text file that is not a script) is
found, and its error reported, not skipped (all three [E4]).

**Not found.** `library-not-found` (`unable to find library -lname`, lld's
wording; `spec/diag.md` §8.2 gives the reasons for not using GNU ld's
`cannot find -lname: No such file or directory`), at the argument, with
one note per directory searched (`searched /usr/lib/aarch64-linux-gnu`)
in order, or the note `the library search path is empty`; and, when
`link_static` excluded a `.so` that exists, the note `D/libname.so exists
but -Bstatic is in effect` (GNU ld's corresponding hint: `have you
installed the static version of the name library ?` [E3]).

### 4.5 What a found file is called

The **found path** of a file is the path as written (a `Path` input), the
directory as written joined with the candidate's name (one `/` is inserted
unless the directory ends with `/`: GNU ld writes `d1//libf.so`, lld
`d1/libf.so` [E21]), or the script-relative result of script.md §2.4. It is
never made absolute, canonical, or lexically normalized: `./d1/../d1/libf.so`
stays as written (GNU ld; lld and mold remove `.` and `..` in the
dependency file [E21], which is wrong when `d1` is a symbolic link).
Diagnostics, `--dependency-file`, the map file, `-t` and the identity of
§9.4 use the found path.

### 4.6 What the phase reads from its environment

The working directory and the file system. **No environment variable.**
`LIBRARY_PATH` is read by none of the three (it is the compiler driver's)
[E5]; `LD_LIBRARY_PATH` affects no `-l` search in any of them [E5];
GNU ld reads `LD_LIBRARY_PATH` when it looks for the `DT_NEEDED` entries
of shared objects (it makes a link succeed that fails without it [E7]);
its documentation names `LD_RUN_PATH` too (*not verified*). mortar does not
(§8.6): a link's meaning must not depend on the shell it is run from
(`docs/ARCHITECTURE.md`, principle 6). `MORTAR_SCHED_SEED` (tests,
`spec/model.md` §6) changes scheduling, never results.

`-rpath-link` directories are not part of the library search path (all
three [E7]).

### 4.7 `-R`

`-R X` (`JustSymbols`, `spec/cli.md` §7.2): if `X` names a directory, it
is `-rpath X` (all three write `DT_RUNPATH` `X` [E25]), recorded in
`Inputs.rpath_from_r` with its argument index, so that the synthetic
phase orders it among the `-rpath`s by argument; otherwise it is
`--just-symbols X`, refused (`feature` `--just-symbols`) until specified
(GNU ld reads the file's symbols, lld always means `-rpath`, mold refuses
`-R FILE` [E25]).

## 5. Linker scripts as inputs

### 5.1 Expansion

A file in input position that sniffs as a script (§2.1 row 8) is an
**implicit script**. It is parsed (`mortar_script::parse`) and replaced, in
place in load order, by the files its commands name (`spec/model.md`
§2.1). Its commands, as script.md §2.3-§2.8 define them:

| command | what this phase does |
|---|---|
| `INPUT`, `GROUP`, `AS_NEEDED` | the entries, in order, are inputs at the script's position (§5.2) |
| `SEARCH_DIR` | extends the library search path (§4.2) |
| `INCLUDE` | expanded as script.md §2.8 says (search: working directory, then the library search path; 10 levels) |
| `OUTPUT_FORMAT`, `OUTPUT_ARCH`, `TARGET` | checked against the target once it is known (§3.2) |
| `OUTPUT`, `ENTRY`, `EXTERN` | recorded in `Inputs.directives`, with the script position, for the phases that use them (script.md §2.7) |
| `STARTUP` | refused, `script-command` `STARTUP`: GNU ld moves the file to the front of the link, lld and mold do not implement it (script.md §2.3), and no system script uses it |
| any refused command | already refused by the parser (script.md §2.10) |

The system's implicit scripts are of this form (`libc.so`: `OUTPUT_FORMAT(
elf64-littleaarch64) GROUP ( /lib/aarch64-linux-gnu/libc.so.6
/usr/lib/aarch64-linux-gnu/libc_nonshared.a AS_NEEDED (
/lib/ld-linux-aarch64.so.1 ) )`; `libgcc_s.so`: `GROUP ( libgcc_s.so.1
-lgcc )`, a relative name found next to the script [E8]).

### 5.2 Positional state of a script's entries

An entry is read with the positional state of the input that brought the
script into the link (`spec/cli.md` §3.3), changed as follows:

- inside `AS_NEEDED ( ... )`, `as_needed` is on; after the `)`, it is the
  including state again (all three [E8]);
- the entries of `GROUP` form a group, unless the script already stands in
  a group (the outermost group is recorded, as `spec/cli.md` §3.3 does for
  nested command-line groups);
- `-l` entries search with the including `link_static` (script.md §2.4
  rule 4);
- `whole_archive` applies to archives a script names (all three [E8]);
- a `--start-lib` region applies to relocatable objects a script names
  (lld, mold [E14]; GNU ld has no `--start-lib`).

### 5.3 Nesting

An entry may name another implicit script, which expands in turn (all
three [E9]).

- **Cycles.** A script whose file (by device and inode, §9.4) is already
  being expanded on the current chain is `script-cycle` at the entry that
  names it, with `>>> named by` context lines for the chain, innermost
  first. GNU ld and mold loop until killed and lld overflows its stack on
  `c1.ld: INPUT(c2.ld)`, `c2.ld: INPUT(c1.ld)` and on a script naming
  itself [E9].
- **Depth.** More than 16 implicit scripts on one chain is refused,
  `feature` `linker scripts nested more than 16 deep`. (Separate from
  `INCLUDE`'s 10 levels and `AS_NEEDED`'s 16.)

A script named twice (not on one chain) expands twice: it is two mentions.

## 6. Relocatable objects

An `ET_REL` file of the target, named directly, by a script, or by
`--start-lib` (§7.6), is one file: a `FileId` (§10), parsed with
`mortar_elf::Object` (elf.md §3). Every non-local symbol-table entry is
interned (`spec/model.md` §2.3); `SHT_GROUP` signatures are interned
(`ComdatId`); the LTO check of §2.2 is made on the same pass over the
symbol table. Structural errors of the object found on that pass are
`malformed-input`; the reader's other checks happen when later phases ask
for sections (elf.md §1: parse is constant time).

The objects of an odd but legal shape (no `.text`, `sh_addralign` 0,
`SHN_ABS` symbols) link as any other: `tests/link/input/yaml2obj-odd.test`.

## 7. Archives

### 7.1 Opening

An archive is read whole (§11) and parsed with `Archive::parse`
(archive.md). Any error of the reader is `malformed-input` at the archive
(or at `archive(member)` when the reader names a member), except a
malformed symbol index: the index is not used (§7.4), so a malformed one
is the **warning** `malformed-input` (`the symbol index is malformed and
is not used: ...`) and the members are read. GNU ld rejects such an
archive (`file format not recognized`), lld and mold link it without a
word [E12]. This needs a change of `Archive::parse` (§17, archive.md).

Every mention of an archive is an archive of its own: it gets an
`ArchiveId`, in load order (§10; requested change to `spec/model.md` §2,
which says "in order of first appearance").

### 7.2 Members

The ordinary members are walked in archive order (archive.md §7.1). A
member's bytes:

- regular archive: the member's slice of the archive's buffer
  (`Content::Inline`);
- thin archive (`Content::External`): the file at the stored path, which
  is relative to the directory of the archive's found path unless it
  starts with `/` (binutils; all three [E13]), read as a file of its own
  (§11);
- GNU's nested form in a thin archive (`Content::ExternalMember`): the
  archive at that path (relative as above) is read and parsed, and its
  member at `header_offset` is the member; it must be `Inline`, else
  `malformed-input`. GNU ld links this form; lld refuses it; mold takes
  nothing from it, silently (measured by WP-005, archive.md §7.3). mortar
  follows GNU ld (WP-005 open question 1): the meaning is unambiguous.

A thin member's file that cannot be opened is `cannot-open-file` at
`archive(member)`, **whether or not the member would be extracted**: lld
and mold report a missing unneeded member, GNU ld only a needed one [E13].
mortar reads every member (§7.4). A thin member whose size differs from
its header's size is read at its actual size, without a diagnostic: all
three do the same [E13], and the header's size is used for nothing once
the index is not (§7.4).

The member's **display name** is `archive(member)` (`FileRef::in_archive`):
`member` is the member name for a regular archive and the path that was
opened for a thin one (`sub/libt.a(sub/objs/f.o)`, GNU ld's form [E13]).

### 7.3 What a member may be

Each member is sniffed (§2.1, except that row 1 and row 8 do not apply:
an empty or text member is not a script) and treated by this table.
"Lazy" is a member of an archive read without `--whole-archive`.

| member | lazy | `--whole-archive` | references [E11] |
|---|---|---|---|
| relocatable object of the target | a `FileId` | a `FileId` | all three |
| relocatable object of another machine | `incompatible-file` | `incompatible-file` | lld, mold: error either way; GNU ld: silent when lazy |
| LLVM bitcode, GCC slim LTO object | refused as §2.1/§2.2 | refused | mold: error; GNU ld, lld: lazy silent (lld would perform LTO) |
| fat LTO object | a `FileId` (§2.2) | a `FileId` | — |
| text, empty, unknown bytes, a nested archive, a shared object, an executable, a core file | warning `ignored-member` | error `ignored-member` | lazy: lld warns `neither ET_REL nor LLVM bitcode`, GNU ld silent, mold silent (warns for a shared object); whole: GNU ld errors except for a shared-object member (exit 0), lld errors except for an executable member (exit 0), mold silent |
| ELF that is malformed, or refused by §2.4 | as §2.4 | as §2.4 | — |

Reasons:

- A member that is not an object contributes nothing in any reference.
  mortar says so, as lld does, because an archive that silently carries a
  member nothing can use is a packaging error someone wants to hear about
  (no member of the 95 system archives on this machine is one [E11]).
  Under `--whole-archive` the user asked for every member to be linked,
  and one cannot be: an error, as GNU ld and lld.
- LTO members are refused even when lazy: their symbols are in an IR
  table mortar does not read, so mortar cannot decide whether they would
  be extracted; GNU ld's and lld's silent skip of a lazy one is a guess.
- A member of another machine is an error even when lazy: it is a broken
  library, and mortar parses every member anyway.

`ignored-member` names what the member is (`a shared object`, `an
archive`, `not an ELF file`, `empty`, `an executable (ET_EXEC)`).

### 7.4 The symbol index

**mortar does not use the archive symbol index.** Every relocatable member's
symbol table is read and its non-local keys interned (`spec/model.md`
§2.3) whether or not the member is extracted, because the numbering of
symbols and the order-independent extraction of model §3.3 need every
member's keys. Extraction is therefore decided from the members
themselves, as lld and mold decide it, and the index costs nothing to
ignore.

What the index does in the references [E12]: GNU ld resolves from it. An
archive without one is an error (`archive has no index; run ranlib to add
one`); a stale one (members replaced without `ranlib`) makes GNU ld extract
the member the index names, which no longer defines the symbol (undefined
reference), and miss a symbol a member defines and the index does not.
lld and mold link archives without an index, and resolve a stale one by
the members. An index can lie; the members cannot. WP-005's open question
2 is answered: the members.

### 7.5 `--whole-archive`

Each relocatable member of an archive read with `whole_archive` has it in
its `files` row; `spec/resolve.md` makes such members live.

### 7.6 `--start-lib` and `--end-lib`

A relocatable object read in a lib region (`lib` of `spec/cli.md` §3.3,
directly or through a script, §5.2) is a **lazy object**: a `FileId` with
`lazy` set and no `ArchiveId`, which `spec/resolve.md` treats like an
archive member. Shared objects and archives in a lib region are read as
anywhere else (lld, mold [E14]). `--whole-archive` does not make a lazy
object live (lld, mold: `--whole-archive --start-lib k.o --end-lib` does
not link `k` [E14]). GNU ld does not have the options
(`unrecognized option '--start-lib'` [E14]).

## 8. Shared objects

### 8.1 What is accepted

An `ET_DYN` file of the target is read with the shared-object reader
(elf.md §6, WP-006), which this spec needs to provide: `DT_SONAME`;
`DT_FLAGS_1`; the `DT_NEEDED` entries in table order; `DT_RUNPATH` and
`DT_RPATH`; the dynamic symbol table with versions; whether the file has
section headers. It is accepted except:

- **A position-independent executable** (`DF_1_PIE` in `DT_FLAGS_1`):
  refused, `object-type` `DF_1_PIE`, note `a position-independent
  executable cannot be linked against`. GNU ld refuses it (`cannot use
  executable file`); lld and mold read it as a shared object, which
  exports only what was exported (`undefined symbol: f` for a function
  `pie.c` defines) [E18]. `PT_INTERP` is not the test: glibc's
  `libc.so.6` has a `PT_INTERP` and no `DF_1_PIE` [E18], and is linked
  against by every program. A PIE written by a linker that does not set
  `DF_1_PIE` is read as a shared object, as lld and mold read it.
- **No section headers** (`llvm-objcopy --strip-sections`): refused,
  `feature` `shared object without section headers`. GNU ld refuses
  (`file in wrong format`); lld and mold accept it, find no symbols, and
  report the program's references as undefined [E18]. WP-006's reader can
  read the dynamic table through `PT_DYNAMIC` (`SharedObject::parse_by`),
  but no reference does so, so there is nothing to test it against
  (`docs/AGENTS.md` rule 4). `SharedObject::parse` is called with section
  headers required.
- **`DT_SONAME` twice**: `malformed-input` (the reader refuses a repeated
  tag). GNU ld and lld record the last one, mold the first [E18]; no
  writer produces it.
- **`link_static` in effect** (`-static`, `-Bstatic`, a shared object
  named by path or by `-l:`, or by a script, after it): the error
  `static-link-of-shared-object`, `attempted static link of dynamic object
  d1/libf.so` (GNU ld's and lld's wording; both reject it). mold accepts
  the line, drops the shared object and writes a static executable in
  which `f` is undefined (`U f`, no dynamic section) [E17].

### 8.2 Identity: one file per soname

A shared object's **soname** is its `DT_SONAME`, or else its needed name
(§8.3). A shared object whose soname equals that of a shared object loaded
earlier in load order is **not loaded**: it has no `FileId`, and its
mention is recorded in `Inputs.opened` (§10.3).

All three ignore the later one, also when it is another file with other
contents: a second `libf.so.1` that also defines `g` does not satisfy a
reference to `g` in any of them [E16]. The dynamic loader loads one
object per soname, so binding to the first is the only consistent
choice; mortar also tells the user when the two differ:

- the same file (device and inode), or equal bytes: no diagnostic;
- different bytes: the warning `duplicate-soname` at the later file,
  `DT_SONAME 'libf.so.1' is already loaded; this file is not used`, with
  the note `loaded from d1/libf.so`.

The loaded object's `as_needed` is on only if it is on at **every** mention
of the soname: GNU ld and lld record `DT_NEEDED` for `--as-needed libf.so
--no-as-needed libf.so` with `f` unused; mold takes the first mention's
state and records none [E16]. `copy_dt_needed` is on if it is on at any
mention (*not verified*). The other positional state is the first
mention's.

### 8.3 The needed name

The name a `DT_NEEDED` entry gets if the shared object is needed
(`spec/synth.md` writes it):

| shared object | needed name | references [E16] |
|---|---|---|
| has `DT_SONAME` | the soname | all three |
| no soname, a `Path` input | the path as written (`ns/libf.so`, `/abs/ns/libf.so`, `./ns/../ns/libf.so`) | all three |
| no soname, found by `-l name` or `-l :file` on the command line | the candidate's file name (`libf.so`) | all three |
| no soname, named by a script | refused, `feature` `shared object without DT_SONAME named by a linker script` | `INPUT(ns/libf.so)`: GNU ld `./ns/libf.so`, lld and mold `ns/libf.so`; found by the search: GNU ld and mold the found path, lld the name as written; `INPUT(-lf)`: GNU ld and lld `libf.so`, mold `ns/libf.so` |

The last row is rare (system scripts name libraries that have sonames)
and the three disagree on every form of it.

### 8.4 An executable with shared objects and no interpreter

A link of kind `StaticExecutable` or `StaticPie` (`spec/cli.md` §3.2: no
`-dynamic-linker`) that loads a shared object is refused, `feature`
`dynamic executable without -dynamic-linker`, with the note naming the
shared object. GNU ld gives the output its configured default
interpreter (`/lib/ld-linux-aarch64.so.1` here); lld and mold write a
dynamic executable with no `PT_INTERP` [E17], which the kernel starts
without a loader. GNU ld's default is a property of its build (and wrong
for musl), and lld's output does not run; the compiler drivers always
pass `-dynamic-linker`. This settles the `spec/cli.md` §3.2 note.

### 8.5 A shared object named twice

The same file named twice is one soname: §8.2 (all three record one
`DT_NEEDED` [E16]).

### 8.6 The `DT_NEEDED` closure (M2)

Shared objects that a loaded shared object needs and the line does not
name are loaded only when some rule needs their symbols:
`--copy-dt-needed-entries` (GNU ld only: lld and mold reject the option
[E7]), and whatever `spec/resolve.md` decides for undefined symbols of
shared objects (`--no-allow-shlib-undefined`; GNU ld reports `undefined
reference` inside a shared object whose needed library it cannot find,
lld and mold do not [E7]). This section specifies the loading; when it
happens is `spec/resolve.md`'s.

For needed name `N` of loaded shared object `S`, with no loaded shared
object of soname `N`:

1. `N` containing `/` is opened as written;
2. otherwise the first existing `D/N`, for `D` in: the `-rpath-link`
   directories in order; the `-rpath` directories in order (and `-R DIR`,
   §4.7); `S`'s `DT_RUNPATH`, or its `DT_RPATH` if it has no
   `DT_RUNPATH`, entries split at `:`, `$ORIGIN` and `${ORIGIN}` replaced
   by the directory of `S`'s found path (GNU ld finds `libf.so` through
   `$ORIGIN/../hid` [E7]); `$LIB` and `$PLATFORM` are refused (`feature`).
   Sysroot prefixing of these directories: *not verified*; refused with a
   sysroot until measured.

GNU ld also searches `LD_RUN_PATH`, `LD_LIBRARY_PATH`, its built-in
directories and `/etc/ld.so.conf`, and **not** the `-L` directories (a
link with `-Lhid` still says `libf.so, needed by dg/libg.so, not found`
[E7]). mortar reads no environment (§4.6) and no host configuration
(§4.2).

Found files are loaded as shared objects with `as_needed` on and the
other state of `S`, numbered after the walk (§10.2). A needed name not
found is the warning `needed-not-found`: `libf.so, needed by
dg/libg.so, not found (try using -rpath or -rpath-link)` (GNU ld's
wording, which tools and users search for), once per `(N, S)`.

## 9. The same file twice

### 9.1 Mentions, not files

Each mention of a file in load order is read as its own input, with these
exceptions only: a shared object (one per soname, §8.2); a script on its
own expansion chain (a cycle, §5.3).

| named twice | mortar | references |
|---|---|---|
| an object (`f.o f.o`, `f.o ./f.o`, a hard link) | two `FileId`s: `duplicate-symbol` for its definitions | all three [E15] |
| an archive (`libf.a libf.a`, through a symbolic link, by path and by `-l`) | two `ArchiveId`s, two sets of member `FileId`s | all three: `--whole-archive libf.a libf.a` and `libf.a link.a` are duplicate definitions [E15] |
| a shared object | one `FileId` | all three [E16] |
| a script | expanded twice | — |

Which members of two mentions of one archive become live is
`spec/resolve.md`'s: for `libf.a --whole-archive libf.a`, GNU ld and lld
extract the lazy copy too and report a duplicate definition, mold does
not [E15].

### 9.2 Paths are not canonicalized

Whether two mentions are "the same file" never depends on canonical paths,
symbolic links or inodes, except for the two exceptions of §9.1 and the
store's read-once rule (§11.2). The references agree [E15, E16].

### 9.3 Bytes are shared

Mentions of one file (same device and inode at `fstat`) are read once:
the store holds one buffer, and each mention's rows point into it (§11.2).

### 9.4 Identity for incremental links

`spec/model.md` §7.1 says a file's identity is "the path as resolved". It
is made exact here:

- **file**: the found path (§4.5), plus the **mention ordinal**: the number
  of earlier files in load order with the same found path (0 for the
  first). Two mentions of `f.o` are `(f.o, 0)` and `(f.o, 1)`;
- **archive member**: the archive's identity, the member name, and the
  ordinal among the archive's members of that name (model §7.1);
- with the content hash of the bytes read (§11.5).

Not canonical, not by inode: a build system that replaces a file through
a symbolic link (or a new inode at the same path: `ar` and compilers write
a temporary and rename it) keeps the path, which is what the user means by
"the same input"; an inode changes on every rebuild. The content hash
decides whether it changed.

## 10. Load order and ids

### 10.1 The walk

`FileId`s and `ArchiveId`s are defined by this sequential walk. A parallel
implementation reads, sniffs and parses in any order (§12) and must
produce exactly these numbers. `FileId(0)` is the internal file
(`spec/model.md` §2.1).

```
next_file = 1, next_archive = 0
for each Plan input i, in command-line order:
    visit(i, the input's state, chain = [])

visit(input, state, chain):
    F = the file the input names (§4; for a script entry, script.md §2.4)
    match the kind of F (§2):
        relocatable object (§6):   file row (F, state, lazy = state.lib.is_some()); next_file += 1
        archive (§7):              a = next_archive; next_archive += 1
                                   for each ordinary member m, in archive order:
                                       if m is a relocatable object of the target:
                                           file row (m, state, archive = (a, header offset)); next_file += 1
        shared object (§8):        if its soname is loaded: merge the state (§8.2); no row
                                   else: file row (F, state); next_file += 1
        script (§5):               for each entry e of its INPUT, GROUP, AS_NEEDED and INCLUDE
                                   expansions, in source order:
                                       visit(e, state changed by §5.2, chain + [F])
        empty file:                nothing
```

Then (M2) the closure of §8.6, breadth first: the loaded shared objects
in `FileId` order, each one's `DT_NEEDED` entries in table order; a file
found takes the next `FileId`, and its own entries are visited after those
of every file before it.

Every file opened, whatever its kind, is appended to `Inputs.opened` in
the order the walk reaches it (§10.3). A refused or erroneous file ends
the walk for that input and the phase fails (§12.3); no ids are given out
in a failed phase.

Properties the walk guarantees (`spec/model.md` §2.1):

- the numbering is a function of the `Plan`, the working directory and the
  contents of the files reached;
- every relocatable member of every archive mention has a `FileId`,
  extracted or not; lazy objects of `--start-lib` too;
- `ArchiveId` is one per archive mention, in load order, also for an
  archive with no relocatable member.

### 10.2 Group ordinals

The `group` of the positional state is renumbered in load order: groups
are counted from 1 in the order of their first file in load order, over
command-line groups and script `GROUP`s together; a script `GROUP` inside
a command-line group is part of it (§5.2). (`spec/cli.md`'s ordinals
count command-line groups only; they are not used after this phase.)

### 10.3 Rows

`files[FileId]` (`spec/model.md` §3.2) holds:

| column | value |
|---|---|
| `kind` | `Internal`, `Object`, `SharedObject` |
| `name` | the display `FileRef`: the found path, or `archive(member)` (§7.2) |
| `archive` | `(ArchiveId, member header offset)` for an archive member; for a thin member also the member's own found path |
| `origin` | the index of the `Plan` input, its argument index, and the script entry that named it, if any (an index into `Inputs.script_entries`: `(script file, entry offset, parent entry)`, the chain the `named by` context lines are drawn from) |
| `state` | `as_needed`, `whole_archive`, `lazy` (a `--start-lib` object), `link_static`, `copy_dt_needed`, `exclude_libs` (below), `group` (§10.2) |
| `reader` | re-created from the store: `Inputs::object(f)`, `Inputs::shared(f)` (§11.4) |
| `sections` | `e_shnum` |
| `syms` | the range of `sym_map` |
| `identity`, `hash` | §9.4, §11.5 |

`exclude_libs` is on for a member of an archive whose found path's last
component equals a `--exclude-libs` name, or when a name is `ALL` (all
three [E26]; `ex/libex.a` matches no member, as in all three). A name
that equals a loaded archive's last component without its `.a` (`libex`)
is `invalid-option-value`: GNU ld matches it, lld and mold do not [E26].

`archives[ArchiveId]`: found path, kind (regular, thin), the `Plan` input.

`Inputs.opened`: every file opened, in walk order: found path, kind
(including scripts, ignored members' archives, shared objects not loaded,
closure files), origin. `--dependency-file` lists the distinct found
paths of this list in its order (GNU ld lists a script twice, lld and mold
once [E21]); `-t` prints it.

`Inputs.directives`: `OUTPUT`, `ENTRY`, `EXTERN` of implicit scripts, with
their script positions (§5.1). `Inputs.rpath_from_r`: §4.7.

## 11. The store: getting bytes into memory

### 11.1 No mapping

Every crate is `forbid(unsafe_code)`, and a memory mapping is `unsafe` in
Rust for a reason beyond the call: if another process truncates a mapped
file, reading the lost pages raises `SIGBUS`, a crash instead of a
diagnostic, and a mapped file changed during the link changes the bytes
under the linker's feet. Version one **reads**. Mapping is the expected
first request for `unsafe` (`docs/ARCHITECTURE.md`, principle 4); §11.6 is
the measurement it would have to beat.

### 11.2 Reading

Per distinct file (device and inode, §9.3):

1. `open`; `fstat` (size `n`, `st_mtime`, `st_ctime`). Not a regular
   file: §2.5;
2. one owned buffer of exactly `n` bytes;
3. a file of at most 4 MiB is read by one worker with `read_exact`; a
   larger one in 4 MiB chunks, read in parallel by `read_exact_at` into
   disjoint slices of the buffer (§12.1);
4. one more read at offset `n` must return 0 bytes, and a second `fstat`
   must give the same size, `st_mtime` and `st_ctime`; otherwise, or on a
   short read, the error `input-changed` at the file: `file changed while
   it was read`;
5. `close`.

An archive is read whole and its members are slices of its buffer. A thin
member, and the archive named by a nested thin member, is a file of its
own, read the same way.

**Descriptors.** A file is open only while it is read: at most one
descriptor per worker, plus one per large file being read in chunks (at
most one per worker): never more than twice the thread count, whatever the
number of inputs.

**Limits.** A file whose size does not fit `usize` is refused, `feature`
`input file larger than the address space`; a file of 2^40 bytes or more
is refused (`feature` `input file of 1 TiB or more`) because diagnostic
keys hold 40 bits of offset (§12.2). There is no 4 GiB limit: offsets are
`u64` throughout (`spec/model.md` §1). More than 2^32 − 1 files is
`spec/model.md` §1's refusal.

### 11.3 Memory

**Resident input bytes equal the bytes of every file read**, from the read
to the end of the link, and they are anonymous memory: not page cache
that the kernel can drop and re-read, as a read-only mapping is. Measured
(§11.6): reading `cxx-medium-g`'s 200.8 MiB of inputs leaves 201.1 MiB
anonymous; mapping and touching every page leaves 202.6 MiB of
file-backed resident pages and 0.2 MiB anonymous. The peak a user sees
in `top` is the same; the difference is what the kernel can do under
memory pressure.

### 11.4 Room for reading less

The tables never hold a reference into the store (`spec/model.md` §1):
they address bytes by `(FileId, range)` and `SectionId`, and a file's
reader is re-created from the store on demand (`Inputs::object(f)` costs
`Object::parse`, constant time, elf.md §1), not stored as a borrow. So the
store may later hold less than every whole file without a table changing:

- **lazy members in two stages.** This phase needs, per relocatable member:
  the ELF header, the section header table, the section-name string table,
  `.symtab` and its string table, and the `SHT_GROUP` sections. Measured
  on `cxx-medium-g`'s two archives (201 MB): symbol and string tables are
  21.5 MB and section headers 9.2 MB, about 15%; debug sections and their
  relocations are 147.8 MB, 73% [E22]. The bodies of members that
  resolution does not extract need never be read. The store is filled
  between phases with exclusive access (`&mut Store`, between
  `mortar_resolve` and the next phase), so safe Rust allows it;
- **non-alloc debug sections at fill time**, read by `mortar_image`
  directly into the output buffer (`spec/model.md` §1 already allows
  this).

Both need a reader over a file that is not wholly resident. A zero-filled
buffer of the file's size with only some ranges read is **not** that:
measured on this machine (transparent huge pages `always`), a 512 MiB
`vec![0u8; ..]` costs nothing until written, but writing one byte per MiB
makes 161 MiB resident [E23]. The parts must be separate buffers, and
`mortar_elf` must read from a set of `(offset, bytes)` parts: a request for
a later stage of elf.md (§17), not version one.

### 11.5 Content hash

The content hash of a file (incremental links, `spec/model.md` §7.1, M7)
is computed from the buffer that was read: the bytes hashed are exactly
the bytes linked, which a mapping of a file changing during the link
cannot promise. The hash function is `spec/incr.md`'s.

### 11.6 Read against map: measurement

A throwaway program (Rust std only, `rustc -O`; mapping through a `libc`
declaration, in the scratch directory) read the input files of the bench
workloads (`spec/harness.md` "bench"), keeping every buffer, then touched
one byte per 4 KiB page; or mapped them (`PROT_READ`, `MAP_PRIVATE`) and
touched every page, as a linker that copies all sections does. Warm page
cache; the workspace is a bind mount of the host (opening a file is slow);
the machine was loaded (load average 23-28 on 24 cores), so the median of
7 runs is given with the minimum in parentheses; where a configuration was
run in two series, both medians are given [E22].

| workload | files | bytes | read, 1 thread | read, chunked, 8 threads | read, chunked, 16 threads | map + touch, 8 threads | map + touch, 16 threads | map, no touch, 8 threads |
|---|---|---|---|---|---|---|---|---|
| `cxx-medium-g` | 19 | 210.6 MB | 442-466 ms (352) | 145-197 ms (132) | 121-168 ms (114) | 196-248 ms (175) | 246-264 ms (210) | 1.3 ms |
| `synthetic-objects` | 1013 | 86.0 MB | 919 ms (719) | 192-197 ms (163) | 130-177 ms (119) | 174 ms (129) | 89 ms (80) | 69 ms |

(`synthetic-objects` has no file above 4 MiB, so "chunked" is whole-file
reading there; one 16-thread series gave a 451 ms median, an outlier of
the load.) Peak resident memory was the input size plus about 2 MiB in
every mode that touches the bytes (`cxx-medium-g`: 202.8 MiB for 200.8
MiB of input; `synthetic-objects`: 84.1 MiB for 82.0 MiB). Reading whole
files without chunking cannot use more threads than there are files:
`cxx-medium-g` with 8 threads read whole files in 591 ms (481), and with 16
in 412 ms (321), because two archives are 96% of its bytes; chunking is
what makes it scale.

For scale, the bench results on the same machine (`spec/harness.md`
"bench"): lld links `cxx-medium-g` in 934 ms with a peak RSS of 332 MB,
mold in 725 ms and 411 MB, both mapping their inputs. Reading the inputs
costs about what mapping and touching them costs; it is a fifth to a
quarter of mold's whole link on this machine. Mapping wins where a
linker does not touch what it maps (unextracted members, debug sections
copied by `copy_file_range`): §11.4's stages are the safe way to the same
saving.

## 12. Parallelism and determinism

### 12.1 What runs in parallel

| step | unit | depends on |
|---|---|---|
| `stat` and sniff of `Path` inputs; the target by §3.1 rule 2 | `Plan` input | nothing |
| search of each `-l` over the `-L` part of the search path | `Plan` input | the target (to skip incompatible candidates) |
| reading | file, or 4 MiB chunk of a large file | its path |
| parsing scripts; resolving and reading their entries | script; entry | the script's bytes |
| per object and per archive member: header, section headers, the symbol-table pass (§6: LTO check, interning slot) | file / member | its bytes |
| the walk of §10.1 (counting only) and the prefix sums of `FileId`s | sequential | the kinds of all files reached |
| `SymbolId`, `ComdatId` numbering | `spec/model.md` §2.3 | `FileId`s |

The search path of an `-l` after a script includes the script's
`SEARCH_DIR`s (§4.2) only after every `-L`. So a search that finds a
candidate in the `-L` directories is final whatever any script says, and
only a search that fails there waits for the scripts before it in load
order: in practice every search is final in the first wave. The walk
itself is cheap (no bytes, counts only) and sequential by definition.

### 12.2 Keys

Diagnostics of this phase have the key `(20, unit, seq)`:

- `unit` is the argument index of the `Plan` input whose expansion reached
  the file (for an entry of `libc.so`, the argument of `-lc`; for a
  closure file of §8.6, that of the shared object that needs it);
- `seq` is `k × 2^40 + offset`: `k` is the file's ordinal among the files
  that unit's expansion opens, in walk order (the input's own file is 0;
  a script's entries follow it, a thin archive's member files follow the
  archive), and `offset` the byte offset of the construct in that file (an
  archive member: the member's header offset plus the offset within it;
  0 for "the file"). Problems of an argument itself (`-L ''`,
  `library-not-found`) have `k = 0`, `offset = 0`.

`spec/model.md` §8 says "byte offset of the construct in the file as
given": one input can reach several files, so the file's ordinal is
needed in front (requested change, §17).

### 12.3 All errors, in order

Every file reachable is read and checked, in parallel, whatever errors
other files have: the phase does not stop at the first bad file. The
report shows every error in key order, up to `--error-limit`
(`spec/diag.md` §6.4). GNU ld reports every file it cannot find and then
stops at the first it cannot read; lld every file it cannot open or find;
mold the first only [E20]. Errors are not followed into consequences: the
entries of a script that does not parse are not visited, and the phase
emits no id when it fails.

Whether a diagnostic is emitted, and its key, are functions of the inputs
(`spec/diag.md` §6.1): the order in which workers finish is not observable.
Tests run the phase under `MORTAR_SCHED_SEED` and thread counts 1, 2, 7,
64 and compare `Inputs` and the rendered report (§16).

## 13. Diagnostics

All with phase 20 and the key of §12.2. "New" kinds are proposed for
`spec/diag.md` §3.1 (§17), in the files area unless noted.

| condition | severity | kind id | message (wording source) | GNU ld 2.44 says |
|---|---|---|---|---|
| a path does not exist / cannot be opened or read | error | `cannot-open-file` | `cannot open {path}: {reason}` (lld, mold) | `cannot find {path}: No such file or directory` |
| a directory as input | error | `cannot-open-file` | reason `Is a directory` | `cannot find dir: file format not recognized` |
| a `-l` candidate cannot be examined (`ELOOP`, `EACCES`, a directory) | error | `cannot-open-file` | as above | skips it |
| `-l` finds nothing | error | `library-not-found` | `unable to find library -l{name}` (lld) + notes §4.4 | `cannot find -l{name}: No such file or directory` |
| `-L ''`; a `--exclude-libs` name GNU ld alone matches | error | `invalid-option-value` | `{option}: invalid value '{value}'` | — |
| `=` + relative directory with a sysroot (`-L`, scripts) | error | `ambiguous-sysroot-path` (new) | `ambiguous sysroot path '{spelling}'`, notes with both readings (own) | concatenates |
| an incompatible `-l` candidate | warning | `skipped-incompatible` (new) | `skipping incompatible {path} when searching for -l{name}` (GNU ld) | the same |
| no `-m` and no ELF input | error | `no-target` (new) | `target emulation unknown: -m or at least one .o file required` (lld) | uses its default |
| an ELF file of another target | error | `incompatible-file` (new) | `incompatible file: {expected} is expected but got {found}` (mold), note: what set the target | `Relocations in generic ELF (EM: 62)` / `file in wrong format` |
| bytes of no known format | error | `unknown-file-type` | `unknown file type` (lld, mold) | `file format not recognized` |
| a script that does not parse | error | `malformed-input` | script.md §2.9 | `syntax error` |
| a structural error of an ELF file or an archive | error | `malformed-input` | the reader's text | `file format not recognized` |
| a malformed archive symbol index | warning | `malformed-input` | `the symbol index is malformed and is not used: {what}` (own) | `file format not recognized` |
| a member that is not a relocatable object | warning (lazy), error (`--whole-archive`) | `ignored-member` (new) | `archive member is not a relocatable object: {what}`, at `archive(member)` (own; lld: `archive member 'x' is neither ET_REL nor LLVM bitcode`) | silent; whole: `member a.a(x) in archive is not an object` |
| a file changed while read | error | `input-changed` (new) | `file changed while it was read` (own) | — |
| a script cycle | error | `script-cycle` (new) | `linker script names itself`, `named by` chain (own) | loops |
| a shared object of a soname already loaded, different bytes | warning | `duplicate-soname` (new) | `DT_SONAME '{soname}' is already loaded; this file is not used`, note `loaded from {path}` (own) | silent |
| a shared object while `link_static` | error | `static-link-of-shared-object` (new) | `attempted static link of dynamic object {path}` (GNU ld, lld) | the same |
| a needed library not found (§8.6) | warning | `needed-not-found` (new) | `{name}, needed by {path}, not found (try using -rpath or -rpath-link)` (GNU ld) | the same |
| the target's options of another target (`check_target`) | error | `unknown-option` | `spec/cli.md` §2.9 | — |
| every refusal of §14 | error | `refused` | `unsupported {noun}: {name}` | various |

Places: a file diagnostic is `.at` the file (`FileRef`, an archive member
as `archive(member)`); a script problem at `Place::Script`; a problem of
an argument at `Place::Argument`. A file reached through a script carries
`>>> named by` lines: the script position, then the argument
(`spec/diag.md` §2.4; script.md §2.4 rule 5).

## 14. Refused by name

| class | name | where |
|---|---|---|
| `file-format` | `LLVM bitcode`, `GCC LTO object`, `Mach-O`, `COFF`, `WebAssembly` | §2.1, §2.2, §7.3 |
| `file-format` | `big-endian ELF`, `x32`, `ILP32`, `ELF64 EM_386` | §2.4 |
| `file-format` | `binary`; any other `-b` name | §2.6 |
| `machine` | `EM_<name>` or the number | §2.4 |
| `object-type` | `ET_EXEC`, `ET_CORE`, `ET_NONE`, `0x<type>` | §2.2 |
| `object-type` | `DF_1_PIE` | §8.1 |
| `script-command` | `STARTUP` | §5.1 |
| `feature` | `'=' prefix on an input path` | §4.1 |
| `feature` | `input that is not a regular file` | §2.5 |
| `feature` | `--just-symbols` | §4.7 |
| `feature` | `linker scripts nested more than 16 deep` | §5.3 |
| `feature` | `shared object without section headers` | §8.1 |
| `feature` | `shared object without DT_SONAME named by a linker script` | §8.3 |
| `feature` | `dynamic executable without -dynamic-linker` | §8.4 |
| `feature` | `$LIB in DT_RUNPATH`, `$PLATFORM in DT_RUNPATH`, `DT_NEEDED search with a sysroot` | §8.6 |
| `feature` | `input file larger than the address space`, `input file of 1 TiB or more` | §11.2 |

## 15. Where the references differ

| case | GNU ld | lld | mold | mortar | § |
|---|---|---|---|---|---|
| target with no `-m` | built-in emulation | first ELF file in load order | first ELF file named by path | mold's rule, then lld's | §3.1 |
| incompatible `-l` candidate | skip, warning | error | skip, warning | skip, warning | §4.4 |
| lazy archive member of another machine | silent | error | error | error | §3.3 |
| a directory where `-l` looks | skipped | error | error | error | §4.4 |
| a symbolic-link loop where `-l` looks | skipped | skipped | error | error | §4.4 |
| `-L ''` | finds nothing | working directory | finds nothing | error | §4.3 |
| `=dir` (no `/`) with a sysroot | concatenates | joins with `/` | links (directory not verified) | error | §4.3 |
| `=dir` without a sysroot | `dir` | `dir` | not found | `dir` | §4.3 |
| `$SYSROOT` prefix on `-L` | sysroot | literal | sysroot | sysroot | §4.3 |
| `=` on a command-line path | sysroot | literal | literal | refused | §4.1 |
| built-in search directories | 7 (or 11) | none | none | none | §4.2 |
| `SEARCH_DIR` | applies to later `-l` | applies to later `-l` | unsupported | applies to later `-l` | §4.2 |
| script cycle | hangs | crashes | hangs | error | §5.3 |
| `STARTUP` | supported | unsupported | unsupported | refused | §5.1 |
| empty file as input | empty script | empty script | error | empty script | §2.3 |
| GCC slim LTO object, no plugin | warning, linked without its code | linked without its code | error | refused | §2.2 |
| LLVM bitcode | not recognized | LTO | error | refused | §2.1 |
| non-object archive member, lazy | silent | warning | silent | warning | §7.3 |
| non-object member, `--whole-archive` | error (a `.so` member: silent) | error | silent | error | §7.3 |
| archive without an index | error | fine | fine | fine | §7.4 |
| stale index | wrong extraction | members decide | members decide | members decide | §7.4 |
| malformed index | error | ignored | ignored | warning, ignored | §7.1 |
| missing thin member, not needed | silent | error | error | error | §7.2 |
| GNU nested thin member | linked | error | nothing taken | linked | §7.2 |
| `--start-lib` | unsupported | lazy objects | lazy objects | lazy objects | §7.6 |
| PIE as input | refused | shared object | shared object | refused | §8.1 |
| shared object without section headers | refused | no symbols | no symbols | refused | §8.1 |
| `DT_SONAME` twice | last | last | first | error | §8.1 |
| shared object with `-static` | error | error | dropped, symbols left undefined | error | §8.1 |
| `as_needed` over two mentions | needed if any is not as-needed | the same | first mention | GNU ld, lld | §8.2 |
| needed name, soname-less object from a script | several | several | several | refused | §8.3 |
| shared object, no `-dynamic-linker` | its default interpreter | no `PT_INTERP` | no `PT_INTERP` | refused | §8.4 |
| `--copy-dt-needed-entries` | supported | unknown option | unknown option | supported (M2) | §8.6 |
| `DT_NEEDED` search via `LD_LIBRARY_PATH` | yes | — | — | no | §8.6 |
| `-b binary` | supported | supported | refused | refused | §2.6 |
| `-R FILE` | just symbols | rpath | refused | refused | §4.7 |
| `--exclude-libs libex` (no `.a`) | matches | no match | no match | error | §10.3 |
| paths in the dependency file | as found | `.`, `..` removed | `.`, `..` removed | as found | §4.5 |
| errors of several bad inputs | all missing, then first unreadable | all missing | first | all, in key order | §12.3 |

## 16. Tests

### Behavioral tests (`crates/mortar_input/tests/`, named `sN_...` for §N)

- **§2**: `s2_1_sniff_table` (each row, from fixtures: `BC\xC0\xDE`, the
  wrapper, the five Mach-O magics, `\0asm`, the three COFF prefixes, a
  text that starts with `d\x86` is COFF, short COFF prefixes are not);
  `s2_1_extension_is_ignored` (an object named `x.so`, a script named
  `x.o`); `s2_2_e_type` (each row); `s2_2_gcc_slim_lto_refused`,
  `s2_2_gcc_fat_lto_linked` (`gcc -flto [-ffat-lto-objects]`);
  `s2_3_empty_file`; `s2_4_header_errors`; `s2_5_directory`,
  `s2_5_fifo_refused`; `s2_6_formats`.
- **§3**: `s3_1_m_decides`; `s3_1_first_path_input` (a shared object
  first, an archive first: its first relocatable member, lazy);
  `s3_1_scripts_and_l_do_not_count_in_rule_2`; `s3_1_rule_3_fallback`;
  `s3_1_no_target`; `s3_2_check_target_runs_when_no_m` (with the key);
  `s3_2_output_format_checked_late`; `s3_3_incompatible_object`,
  `s3_3_incompatible_lazy_member`, `s3_3_note_names_the_decider`;
  `s3_4_flags_hooks_called_in_fileid_order` (a recording `Arch`).
- **§4**: `s4_1_paths_not_searched`, `s4_1_dash_is_a_file`,
  `s4_1_equals_path_refused`; `s4_2_l_after_L`,
  `s4_2_search_dir_applies_later_only`, `s4_2_no_builtin_dirs`;
  `s4_3_sysroot_table` (each row), `s4_3_empty_L`;
  `s4_4_shared_before_static_per_directory`, `s4_4_bstatic`,
  `s4_4_l_colon`, `s4_4_candidate_stat_table` (dangling link, directory,
  loop), `s4_4_skip_incompatible_with_warning`,
  `s4_4_not_found_notes` (directories listed; the `-Bstatic` note);
  `s4_5_found_path_is_not_normalized` (`-L./d/../d`, `-Ld/`);
  `s4_6_environment_is_ignored` (`LIBRARY_PATH`, `LD_LIBRARY_PATH`,
  `LD_RUN_PATH` set to directories that would change the result);
  `s4_6_rpath_link_is_not_searched`; `s4_7_r_directory_is_rpath`,
  `s4_7_r_file_refused`.
- **§5**: `s5_1_commands` (each row); `s5_1_system_scripts` (the
  `libc.so`, `libm.so`, `libgcc_s.so` of the three targets under `/usr`,
  expanded to the expected files); `s5_2_state_table` (as-needed inside
  and after `AS_NEEDED`, whole-archive through a script, `-l` with
  `-Bstatic`, a script in a lib region, `GROUP` inside a group);
  `s5_3_nesting`, `s5_3_cycle_two`, `s5_3_cycle_self`, `s5_3_depth`.
- **§6**: `s6_object_rows` (sections, `sym_map` range, comdats).
- **§7**: `s7_1_malformed_index_warns`, `s7_1_archive_mentions_get_ids`;
  `s7_2_thin_relative_to_archive`, `s7_2_thin_missing_unneeded_member`,
  `s7_2_thin_size_changed`, `s7_2_gnu_nested_thin_member`,
  `s7_2_display_names`; `s7_3_member_table` (each row, lazy and whole);
  `s7_4_no_index`, `s7_4_stale_index_is_ignored` (both directions, the
  spliced archive of [E12]); `s7_5_whole_archive_state`;
  `s7_6_start_lib` (lazy rows, no `ArchiveId`; `.so` and `.a` unaffected;
  `--whole-archive` no effect).
- **§8**: `s8_1_pie_refused`, `s8_1_libc_so_6_accepted` (it has
  `PT_INTERP`), `s8_1_no_section_headers_refused`,
  `s8_1_soname_twice`, `s8_1_static_link_of_shared_object` (by path, by
  `-l:`, by a script); `s8_2_one_per_soname`,
  `s8_2_duplicate_soname_warning_only_when_bytes_differ`,
  `s8_2_as_needed_merge`; `s8_3_needed_name_table`;
  `s8_4_no_interpreter_refused`; `s8_6_closure_search_order`,
  `s8_6_origin`, `s8_6_not_found_warning`, `s8_6_numbering`.
- **§9**: `s9_1_mentions` (object, archive and shared object twice;
  symbolic and hard links); `s9_3_read_once` (a counting store);
  `s9_4_identity` (mention ordinals; the identity survives replacing a file
  by rename).
- **§10**: `s10_1_walk_reference` (a reference implementation of the
  pseudocode in the test, compared with `load` on generated command lines:
  objects, archives with object and non-object members, shared objects
  with repeated sonames, nested scripts, groups, lib regions);
  `s10_2_group_ordinals`; `s10_3_rows`, `s10_3_exclude_libs`,
  `s10_3_opened_and_dependency_file`.
- **§11**: `s11_2_changed_size`, `s11_2_short_read` (a file truncated
  between `fstat` and read, through a test hook in the store),
  `s11_2_grew`, `s11_2_descriptor_bound` (5 000 inputs with
  `RLIMIT_NOFILE` 64), `s11_2_chunked_read_equals_whole_read`.
- **§12**: `s12_determinism` (seeds × threads 1, 2, 7, 64: equal `Inputs`
  and report bytes); `s12_2_keys` (an error in the third file of `-lc`'s
  expansion sorts after one in the second); `s12_3_all_errors_reported`
  (the line of [E20]).
- **§13**: `s13_each_diagnostic` (one fixture per row: kind id, severity,
  place, key).

Tests of reference behavior run the three references through
`mortar_testutil::run_with`, skip with a note when one is missing and fail
under `MORTAR_REQUIRE_TOOLS=1` (as `spec/archive.md` §9).

### Link tests (`tests/link/input/`)

Each is a statement about the references and runs on all three
architectures unless noted; until mortar links (M1) each is
`xfail(mortar): no linking yet (M1)`. Where references differ, the test
holds each to its behavior with qualified expectations.

| test | pins |
|---|---|
| `per-directory-order.test` | `.so` before `.a` per directory, `-Bstatic`, `-l:` (all three) |
| `l-after-L.test` | a `-L` after the `-l` applies (all three) |
| `script-libc.test` | a program linked with the system `libc.so` script by `gcc`, and by `ld` with an equivalent hand-written `GROUP`/`AS_NEEDED` script: `DT_NEEDED` of `libc.so.6` only, `ld-linux` absent when unused (all three) |
| `script-state.test` | `--as-needed` through a script, `AS_NEEDED` restoring, `--whole-archive` through a script (all three) |
| `search-dir.test` | `SEARCH_DIR` applies to a later `-l` (GNU ld, lld; `skip(mold)`) |
| `sysroot.test` | `-L=/lib` and `$SYSROOT` with `--sysroot`; absolute `-L` not prefixed (`$SYSROOT`: `skip(lld)`) |
| `archive-member-kinds.test` | lazy non-object members do not break a link; a needed member is extracted from an archive that also holds them (all three) |
| `archive-no-index.test` | an archive written with `ar rcS` links (lld, mold; `xfail(bfd)`: `archive has no index`) |
| `archive-stale-index.test` | the spliced archive of [E12]: the member's symbol is found (lld, mold; `xfail(bfd)`) |
| `thin-archive.test` | a thin archive in another directory with relative members (all three) |
| `start-lib.test` | an unused lazy object is not linked and its duplicate definition is not reported (lld, mold; `skip(bfd)`) |
| `same-object-twice.test` | `f.o f.o` is a duplicate definition (`exit: fail`, all three) |
| `same-archive-twice.test` | `--whole-archive libf.a libf.a` is a duplicate definition (all three) |
| `same-soname.test` | two files with one soname: one `DT_NEEDED`, the second file's extra symbol undefined (`exit: fail`, all three) |
| `needed-name.test` | `DT_NEEDED` for a soname-less library by path, by `-l`, by absolute path (all three) |
| `as-needed-merge.test` | `--as-needed X --no-as-needed X` records `X` (GNU ld, lld; `skip(mold)`) |
| `static-shared.test` | `-static` with a shared object fails (GNU ld, lld; `skip(mold)`: links a broken program) |
| `incompatible-skip.test` | `-L<x86 dir> -L<native dir> -lf` links and warns (GNU ld, mold; `skip(lld)`); runs on one architecture with the other's library |
| `incompatible-object.test` | an object of another architecture fails (`exit: fail`, all three) |
| `executable-input.test` | an `ET_EXEC` and a PIE as inputs fail (`ET_EXEC`: all three; PIE: GNU ld, `skip(lld,mold)`) |
| `copy-dt-needed.test` | `--copy-dt-needed-entries` with `-rpath-link` (GNU ld only: `skip(lld,mold)`) |
| `rpath-origin.test` | a needed library found through `$ORIGIN` in `DT_RUNPATH` (GNU ld; others skip) |

## 17. Requested changes to other specs

| spec, section | current text | proposed text | reason |
|---|---|---|---|
| `model.md` §2, `ArchiveId` row | "in order of first appearance on the command line" | "one per archive mention, in load order (`spec/input.md` §10.1)" | each mention is an archive of its own in all three references (§9.1); positional state differs per mention |
| `model.md` §3.2 `target` | "the first object decides when `-m` did not, and every later file must agree or is diagnosed" | "decided by `spec/input.md` §3.1 when `-m` did not; every ELF file must agree (§3.3)" | the rule is two-tier and counts shared objects and archive members (§3.1) |
| `model.md` §3.2 `files` | positional state "(as-needed, whole-archive, lazy, static, exclude-libs)" | add `copy_dt_needed`, `group`; add parts `opened`, `script_entries`, `directives`, `rpath_from_r` (`spec/input.md` §10.3) | §8.6, §10.2, §10.3 |
| `model.md` §3.2 `files` | "the parsed reader ... over the store's bytes" | "a reader re-created from the store on demand" | §11.4: lets the store hold less without a table changing |
| `model.md` §7.1 file | "the path as resolved, plus member name and ordinal ..." | "the found path (`spec/input.md` §4.5) and the mention ordinal among files with that path, plus ..." | two mentions of one path are two files (§9.4) |
| `model.md` §8, input row, `seq` | "byte offset of the construct in the file as given" | "`k × 2^40 + offset` (`spec/input.md` §12.2)" | one input reaches several files (a script, a thin archive) |
| `cli.md` options table (`crates/mortar_cli/src/rows.rs`), `-nostdlib` | refused, "Search only the -L directories" | accepted without effect: "mortar has no built-in search directories (`spec/input.md` §4.2)" | §4.2 |
| `cli.md` §2.9 | "The phase that reads it calls `check_target` (follow-up for WP-021)." | "... calls it as `spec/input.md` §3.2 says; its diagnostics take phase 20" | §3.2 |
| `cli.md` §7.2, `-R` row | "the directory case needs the file system, and is the input phase's to decide (follow-up)" | "`spec/input.md` §4.7" | §4.7 |
| `script.md` §2.3 `STARTUP` | "(manual: like `INPUT`, but the file becomes the first input of the link). lld and mold do not implement it" | add: "`mortar_input` refuses it (`spec/input.md` §5.1)" | lld and mold do not implement it; no system script uses it |
| `script.md` §2.6 | "GNU ld applies a script's `SEARCH_DIR`s to all of that script's `-l` entries, also to those before it ...; lld only to later ones" | add: "GNU ld and lld also apply them to every command-line `-l` after the script; neither to a command-line `-l` before it (`spec/input.md` §4.2)" | [E10] |
| `script.md` §2.4 rule 2 | "mortar MUST make that an error naming both readings" | "... the error `ambiguous-sysroot-path` (`spec/input.md` §4.3), shared with `-L`" | one kind for one condition |
| `archive.md` §5, §6.5 | `parse` fails with `BadIndex` for the problems marked "parse" | `parse` succeeds and `symbol_index()` returns `Option<Result<SymbolIndex, Error>>`; or a `parse_without_index` that skips the index member | §7.1: the index is unused; lld and mold link such archives |
| `archive.md` §7.2 | "`name()` is the same path, which is what `ar t` lists" | "... which is what `ar t` lists when run in the archive's directory; run elsewhere, `ar t` joins it with the archive's directory (`sub/objs/f.o` for `sub/libt.a`)" | [E13]; the display rule of §7.2 |
| `diag.md` §3.1 | — | the new kinds of §13: `ambiguous-sysroot-path`, `duplicate-soname`, `ignored-member`, `incompatible-file`, `input-changed`, `needed-not-found`, `no-target`, `script-cycle`, `skipped-incompatible`, `static-link-of-shared-object` | §13 |
| `elf.md` §6 (WP-006) | — | the reader provides what §8.1 lists; `SharedObject::parse` with section headers required is the entry `mortar_input` uses | §8.1 |
| `elf.md` (later stage) | `Object` borrows the whole file | a reader over `(offset, bytes)` parts | §11.4 |

## 18. Open questions

1. **Empty input files.** Accepted as empty scripts (GNU ld, lld,
   script.md §3.4). An empty `.o` is more often a crashed compiler than an
   intended script. Recommendation: keep, as two references and the
   script spec agree; revisit with a warning kind if users are bitten.
2. **Skipping incompatible libraries** (§4.4) is a warning, so
   `--fatal-warnings` fails such links, as in GNU ld and mold.
   Recommendation: keep.
3. **No built-in search directories** (§4.2). Recommendation: keep; the
   `library-not-found` notes show the (empty) search path, which tells a
   direct `ld` user what to add.
4. **`-b binary`** (§2.6): refused. GNU ld and lld support it (embedding
   resources). Recommendation: specify it in a later stage of this spec
   (section name, `_binary_<name>_start/_end/_size` symbols) with tests
   against both.
5. **No interpreter with shared objects** (§8.4): refused. Recommendation:
   keep; a per-target default would hard-code glibc's paths.
6. **When the `DT_NEEDED` closure is loaded** (§8.6) is `spec/resolve.md`'s
   (WP-022); this spec only loads. Recommendation for it: load only for
   `--copy-dt-needed-entries` and an explicit `--no-allow-shlib-undefined`,
   as lld and mold never load it.
7. **Staged store** (§11.4). Recommendation: version one reads whole
   files; measure peak RSS against lld and mold on the bench set at M1,
   and cut the two-stage store and the parts reader as one work package
   if mortar's peak exceeds lld's.
8. **`library-not-found` wording** follows `spec/diag.md` §8.2 (lld's).
   The brief points out that build systems grep GNU ld's `cannot find -l`;
   `diag.md` §8.2 weighed this. Recommendation: keep lld's wording, and
   keep the decision in `diag.md`.
9. **LLVM fat LTO objects** (`.llvm.lto` sections, `clang
   -ffat-lto-objects`): treated as ordinary objects like GCC's fat
   objects; *not verified* against the references.
10. **`copy_dt_needed` over mentions** of one soname (§8.2): "any" is
    *not verified*.
