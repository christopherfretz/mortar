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
members, shared objects and the shared objects their `DT_NEEDED` entries
name (the closure, §8.6); the target when `-m` is absent; the store;
every part of `Inputs` (`spec/model.md` §3.2) and the per-link input table
of §10.4.

Out of scope: `-T`/`-dT` scripts (refused until M4, `spec/cli.md` §4),
version scripts and dynamic lists (`spec/script.md`, later stages),
which archive members and shared objects take part in the link and what
the closure is used for (`spec/resolve.md`), section classification
(`spec/model.md` §3.3; it may run here per file, I-file), `-z` page and
stack sizes (`spec/layout.md`; the forms the references accept are
recorded in the landed file, [E24]).

Shape (informative):

```rust
pub fn load<'s>(plan: &Plan, env: &LoadEnv, store: &'s Store, diag: &Collector)
    -> Option<Inputs<'s>>;              // None: errors were emitted (spec/diag.md §7)
pub struct LoadEnv { pub cwd: PathBuf, pub threads: Threads }   // nothing else: §4.7
pub struct Store { /* §11; Sync; append-only; every lazily filled part behind a once-cell */ }
pub struct Blob(u32);                   // one per distinct file read (§9.3), in the store's order
impl Store {
    pub fn new(policy: StorePolicy) -> Store;                   // §11.1: Resident or Staged
    pub fn resident(&self, b: Blob, r: Range<u64>) -> Option<Resident>;   // §11.3: inside one part
    pub fn bytes(&self, r: Resident) -> &[u8];                  // cannot fail: no other way to make a Resident
    pub fn copy_to(&self, b: Blob, r: Range<u64>, out: &mut [u8]) -> Result<(), ReadError>;
    pub fn hash(&self, b: Blob) -> ContentHash;                 // §11.6
}
```

A `Blob` is the store's own handle, given when a distinct file is opened
(the order in which workers open files is not observable: nothing but
the store sees a `Blob`). `Inputs` maps every `FileId` to its blob and
base offset (an archive member is its archive's blob plus the member's
offset), and the readers and later phases go through
`Inputs::resident(f, r)`, `Inputs::bytes` and `Inputs::copy_to(f, r, out)`.

The store is created before `load` and outlives `Inputs`: `Inputs<'s>`
borrows names and readers from it (`spec/model.md` §9, §10), and every
phase after `load` holds `&'s Store` only. Nothing takes `&mut Store`
after `load` returns; what a staged store reads later it reads into
once-cells it owns (§11.1), which safe Rust allows behind `&Store`.

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
The references on a slim object: GNU ld *without* a plugin warns `plugin
needed to handle lto object` and links without the code, exit 0; lld links
it silently as an object without code; mold refuses [E1]. On a real
`gcc -flto` line the driver passes `-plugin liblto_plugin.so` and
`-plugin-opt=…`, so GNU ld does perform LTO there and the link is correct
[E36]. mortar ignores `-plugin` (`spec/cli.md` §3.11) and so refuses such a
line by name: the user sees, at the first slim object,
`unsupported file format: GCC LTO object` (kind `file-format`), with the
note `mortar does not perform link-time optimization; compile without
-flto, or with -ffat-lto-objects`. It is never a link that is missing code.
This is the one place the check is made (`spec/resolve.md` §10's row for
`__gnu_lto_slim` is the same refusal; a file refused here never reaches
resolution, so resolution does not make it again).

### 2.3 Empty files

An empty file in input position (a zero-length regular file, or a
character device that is empty by §2.5, such as `/dev/null`) is an empty
linker script and contributes nothing (script.md §3.4). GNU ld and lld
accept both, as an input and under `-r`; mold says `unknown file type`
[E1, E33]. mortar is GNU ld. An empty *archive member* is not a script:
§7.3.

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

The path is `stat`ed (following symbolic links) *before* it is opened,
so mortar never opens, and never blocks on, a FIFO. By file type [E33]:

| type | GNU ld | lld | mold | mortar |
|---|---|---|---|---|
| regular | read | read | read | read |
| directory | `cannot find dir: file format not recognized` | `cannot open dir: Is a directory` | `mmap failed: No such device` | `cannot-open-file`: `cannot open dir: Is a directory; file format not recognized` (§13) |
| character device, empty (`/dev/null`) | empty input, exit 0 | empty input, exit 0 | `unknown file type` | opened; one read of one byte returns end of file, so the input is empty (§2.3) |
| character device, not empty (`/dev/zero`) | *not verified* | *not verified* | *not verified* | `malformed-input`, `not a regular file and not empty`: its size is not known, which the store needs (§11) |
| FIFO, empty writer | blocks (killed by the timeout) | empty input, exit 0 | `unknown file type` | `malformed-input`, `is a FIFO`, decided from `stat` without opening |
| FIFO carrying an object | `file not recognized: Illegal seek` | links it | error | as above |
| socket, block device | *not verified* | *not verified* | *not verified* | as a FIFO, `is a socket` / `is a block device` |

GNU ld is matched where it gives a result: empty inputs contribute
nothing and a directory is an error. Where GNU ld blocks (an empty FIFO)
or fails on a FIFO with content, mortar fails without opening, which is
GNU ld's outcome without the hang; this is the one row that differs from
lld (§15).

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
   scripts expanded and `-l` resolved. This is stated in load order, not
   in time: walking the entries in load order, every search *before* the
   first ELF file takes its first candidate (the compatibility test of
   §4.4 has no target to compare with), that file sets the target, and
   every search after it in load order applies §4.4. Parallel reading
   does not change which file that is (§12.1);
4. otherwise `no-target`, an error: `target emulation unknown: -m or at
   least one .o file required` (lld's wording; GNU ld uses its built-in
   default, mold says `-m option is missing` [E1]). It is not reported
   when an input already has an error (a missing file is `cannot-open-file`
   alone): that input may be the one that would have named the target
   (WP-027c ruling).

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

Exception: a candidate of another target reached by a *search* (`-l`,
`-l:`, a relative script entry, a `DT_NEEDED` closure probe) is skipped
(§4.4), not reported as incompatible. A file named by an explicit path
(a `Path` input, an absolute or `=`-prefixed script entry) is never
skipped: it is `incompatible-file`.

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
mold (`library-not-found` lists what was searched, §4.4).

**`-nostdlib`** (anywhere on the line) removes item 2: the `SEARCH_DIR`
directories of every implicit script are not part of the library search
path, for the command line's `-l`s and for the script's own entries
alike. That is its whole effect in mortar (there are no built-in
directories to drop). GNU ld and lld both do this: with `s.ld` =
`SEARCH_DIR(dd)`, `u.o s.ld -lq` links, and with `-nostdlib` it fails
(`cannot find -lq`, `unable to find library -lq`); `SEARCH_DIR(dd)
INPUT(-lq)` fails the same way under `-nostdlib` [E32]. Requested change
to `spec/cli.md` (§17).

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
| `S` | does not start with `/` | `ambiguous-sysroot-path` error | GNU ld and mold concatenate (`=lib` is `Slib`: GNU ld by `-t`, mold by `--trace`, and mold fails `library not found` when only `S/lib` exists [E37]); lld joins (`S/lib`) |

A `$SYSROOT` prefix is GNU ld's; lld does not know it (searches a
directory literally named `$SYSROOT/lib`), mold does [E6]. mortar follows
GNU ld, which documents it (script.md §2.4 rule 2 makes the same choice).

**Spelling.** The directory is `S` with every trailing `/` removed,
followed by `D` (which starts with `/`). `--sysroot=/ -L=/usr/lib` is
therefore `/usr/lib`, never `//usr/lib`, and `--sysroot=S/` is `S/lib`.
With `--sysroot=/`, GNU ld (`-t`, the dependency file), lld 22 and mold
(`--trace`) all print one slash; with `--sysroot=S/`, GNU ld prints
`S//lib/…` and lld `S/lib/…` [E35]. mortar's spelling is GNU ld's in the
common case and never doubles a slash. (The reviewer saw `//usr/…` from
an lld; lld 22 here does not do that: *not established* which version
did.)

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

**The skip rule (one rule for every search).** A *search* is: `-l name`,
`-l :file` (command line or script entry), every probe of a relative
script entry (script directory, working directory, library search path:
script.md §2.4 rule 3), and every probe of the `DT_NEEDED` closure
(§8.6). When the target is known at that point of load order (§3.1), a
candidate that exists and is one of

- an ELF file, of any `e_type`, whose machine or class is not the
  target's;
- an archive whose first relocatable object member (in archive order) is
  such a file;
- an implicit script whose `OUTPUT_FORMAT` (any of its three names, as
  script.md §2.5 reads it) names a BFD target that is not the target's;

is **skipped**: the warning `skipped-incompatible` is emitted at the
search and the search continues with the next candidate. The text is
GNU ld's: `skipping incompatible xd/libf.a when searching for -lf`, and
for a script entry or closure probe `… when searching for libgcc_s.so.1`
(the name as written, without `-l`) [E27]. It is a warning, so
`--fatal-warnings` fails the link.

Every other candidate that exists is **found**, including one that is
not a library at all (garbage, a text file that is not a script): its
error is reported, it is not skipped (all three [E4]).

A file named by an **explicit path** is never searched and so never
skipped: a `Path` input or an absolute or `=`-prefixed script entry of
another target is `incompatible-file` (§3.3).

The references [E2, E27, E28]: GNU ld skips in all four forms (an i686
`-L` of x86-64 `libm.so`/`libm.a`/`libc.so`/`libc.a`/`libgcc_s.so.1`; an
aarch64 line past an i686 `libc.so` script and `libc.a`; a script with
`OUTPUT_FORMAT(elf32-i386)` before an aarch64 one; `-l:crti.o` past the
x86-64 `crti.o`) and for the script-directory and working-directory
probes of a script entry. mold skips the same candidates (`skipping
incompatible file`). lld takes the first candidate and fails
(`… is incompatible with elf32-i386`). Multilib and multiarch directory
lists depend on skipping. mortar is GNU ld.

**Not found.** `library-not-found` at the argument (or script entry),
with the text `cannot find -lname: No such file or directory` (GNU ld's;
`cannot find -l:file` for `-l:`; the wording policy, §13), one note per
directory searched (`searched /usr/lib/aarch64-linux-gnu`) in order, or
the note `the library search path is empty`; one note per candidate
skipped as incompatible (`skipped xd/libf.a: elf32-i386`); and, when
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
§9.4 use the found path. Under a sysroot the directory is spelled as
§4.3 says (`/usr/lib/…` for `--sysroot=/`).

### 4.6 Search provenance (probes)

`Inputs.link.probes` (in the per-link table, §10.4) records **every probe**
the phase made, in load order: each `stat` of a candidate of a `-l` or
`-l:` search, of each step of a relative script entry's search
(script-directory and working-directory probes included), of an
`INCLUDE` search, and of the `DT_NEEDED` closure (§8.6). A row is

```rust
pub struct Probe<'s> {
    pub path: &'s [u8],          // the candidate as spelled (§4.5)
    pub search: SearchRef,       // the argument or script entry or closure edge
    pub outcome: ProbeOutcome,
}
pub enum ProbeOutcome { Absent, Skipped(TargetName), Found(FileId), Error(Errno) }
```

Probes after a found candidate are not made and not recorded. The table
is a function of the `Plan` and the file system as read; it is in load
order, so it does not depend on parallel scheduling.

**What an incremental link re-checks.** The result of every search is a
function of its probes: re-`stat` every `Absent`, `Skipped` and `Error`
probe, and every `Found` probe's identity (§9.4). If every outcome is the
same, every search finds what it found before; if any differs, the
searches from that probe on are re-run. The cost is one `stat` per probe
(a default `g++` line makes on the order of a hundred: six directories
times two candidates per `-l`, plus the closure) and no reads.
The incremental-linking spec (not yet written) owns the rest.

### 4.7 What the phase reads from its environment

The working directory and the file system. **No environment variable.**
`LIBRARY_PATH` is read by none of the three (it is the compiler driver's)
[E5]; `LD_LIBRARY_PATH` affects no `-l` search in any of them [E5];
GNU ld reads `LD_LIBRARY_PATH` when it looks for the `DT_NEEDED` entries
of shared objects (it makes a link succeed that fails without it [E7]);
its documentation names `LD_RUN_PATH` too (*not verified*), and on native
Linux builds it also reads `/etc/ld.so.conf`. mortar reads neither
(§8.6): a link's meaning must not depend on the shell it is run from
(`docs/ARCHITECTURE.md`, principle 6). `MORTAR_SCHED_SEED` (tests,
`spec/model.md` §6) changes scheduling, never results.

`-rpath-link` directories are not part of the library search path (all
three [E7]); they are the first step of the closure search (§8.6).

### 4.8 `-R`

`-R X` (`JustSymbols`, `spec/cli.md` §7.2): if `X` names a directory, it
is `-rpath X` (all three write `DT_RUNPATH` `X` [E25]), recorded in
`Inputs.link.rpath_from_r` with its argument index, so that the synthetic
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
| `OUTPUT`, `ENTRY`, `EXTERN` | recorded in `Inputs.link.directives`, with the script position, for the phases that use them (script.md §2.7) |
| `STARTUP` | refused, `script-command` `STARTUP`: GNU ld moves the file to the front of the link, lld and mold do not implement it (script.md §2.3), and no system script uses it |
| any refused command | already refused by the parser (script.md §2.10) |

The system's implicit scripts are of this form (`libc.so`: `OUTPUT_FORMAT(
elf64-littleaarch64) GROUP ( /lib/aarch64-linux-gnu/libc.so.6
/usr/lib/aarch64-linux-gnu/libc_nonshared.a AS_NEEDED (
/lib/ld-linux-aarch64.so.1 ) )`; `libgcc_s.so`: `GROUP ( libgcc_s.so.1
-lgcc )` [E8]). The relative entry `libgcc_s.so.1` is *not* next to the
script (`/usr/lib/gcc/aarch64-linux-gnu/14/` has no such file): rule 3 of
script.md §2.4 finds it through the library search path, at
`/usr/lib/gcc/aarch64-linux-gnu/14/../../../aarch64-linux-gnu/libgcc_s.so.1`.
That is a search, so the skip rule of §4.4 applies to it: on the i686
line with an x86-64 `-L` first, GNU ld skips
`/usr/x86_64-linux-gnu/lib/libgcc_s.so.1` and continues [E27]. A
`libgcc_s.so.1` in the working directory is taken before the search path
by all three [E28]; it is a probe recorded in §4.6.

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
- every probe of a relative entry (script directory, working directory,
  library search path) and of a `-l` entry is a search: §4.4's skip rule
  applies to each, and each is recorded (§4.6). An absolute or
  `=`-prefixed entry is an explicit path and is never skipped;
- under `-nostdlib`, the script's `SEARCH_DIR`s do not apply to its own
  entries either (§4.2);
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
(`ComdatId`) by their **effective signature**: the signature symbol's
name, or, when that symbol is `STT_SECTION` (as GNU as writes for a group
named like its section), the name of the section it refers to
(`spec/resolve.md` §6.1); the LTO check of §2.2 is made on the same pass
over the symbol table. Structural errors of the object found on that pass are
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
mention is recorded in `Inputs.link.opened` and in its soname unit (§10.4).

All three ignore the later one, also when it is another file with other
contents: a second `libf.so.1` that also defines `g` does not satisfy a
reference to `g` in any of them [E16]. The dynamic loader loads one
object per soname, so binding to the first is the only consistent
choice; mortar also tells the user when the two differ:

- the same file (device and inode), or equal bytes: no diagnostic;
- different bytes: the warning `duplicate-soname` at the later file,
  `DT_SONAME 'libf.so.1' is already loaded; this file is not used`, with
  the note `loaded from d1/libf.so`.

The loaded object's state merged over its mentions is a property of the
link, not of the file, and lives in the per-link table (§10.4), not in
its row: `as_needed` is on only if it is on at **every** mention of the
soname (GNU ld and lld record `DT_NEEDED` for `--as-needed libf.so
--no-as-needed libf.so` with `f` unused; mold takes the first mention's
state and records none [E16]); `copy_dt_needed` is on if it is on at any
mention (*not verified*). The row keeps the first mention's own state
(§10.3).

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

The same *soname* named twice is one file: §8.2 (all three record one
`DT_NEEDED` for `libf.so.1` named twice [E16]). A shared object without
`DT_SONAME` has the needed name of §8.3 as its soname, so the same file
named in different spellings is different sonames and different files:
`-Lns2 -lf ns2/libf.so ./ns2/libf.so` is three `FileId`s and three
`DT_NEEDED` entries, `libf.so`, `ns2/libf.so` and `./ns2/libf.so`, in all
three references [E34]. Named twice in the same spelling, it is one.

### 8.6 The `DT_NEEDED` closure (M2)

The **closure** is the set of shared objects reachable through
`DT_NEEDED` entries from the shared objects loaded by the walk (§10.1)
that the walk did not load itself. GNU ld loads it for executables and it
changes the output [E30]: `libg.so` needs `libh.so`, `libh.so` references
`hook`, the program defines `hook` and links only `-lg`; GNU ld exports
`hook` (it is in `.dynsym`) and the program prints 42, while lld and mold
do not export it and the program fails at run time with
`./libh.so: undefined symbol: hook`. mortar is GNU ld.

**When.** The input phase loads the closure when the output is an
executable (`Executable`, `PieExecutable`; a static kind loads no shared
object, §8.4), when it
is a shared object and `--no-allow-shlib-undefined` is in effect. It is
never loaded for `-r`. The decision is a function of the `Plan` only.
In a shared object whose closure is not loaded, the edges of the walk's
shared objects are still recorded, in the order below, and nothing is
searched: an edge is satisfied by a loaded shared object with soname `N`,
otherwise it is unsatisfied (`spec/dyn.md` D-11 item 3 reads them;
WP-123).

**Of what.** The closure of *every* shared object the walk loaded,
including those mentioned under `--as-needed` that resolution will later
find unneeded: the input phase needs no resolution result, and `Inputs`
stays immutable once the phase ends (`spec/model.md` §3.2). Which part of
it a later phase may use is stated below.

**Numbering.** Closure files get `FileId`s after every file of the walk.
The queue is processed breadth first: initially, for each shared object
of the walk in `FileId` order, its `DT_NEEDED` entries in table order;
each closure file found is appended with its own entries. Each entry is
an edge `(needing FileId, DT_NEEDED index, N)`. For edge `N`: if a
shared object already loaded (walk or closure) has soname `N`, the edge
is satisfied by it and nothing is searched; otherwise `N` is searched as
below; a file found whose soname is already loaded is not loaded (as
§8.2: recorded, `duplicate-soname` if its bytes differ); otherwise it
takes the next `FileId`. The order is a function of the `Plan` and the
file contents.

**Search** for edge `N` of needing shared object `S`, deterministic, no
environment variable, no `/etc/ld.so.conf`:

1. `N` containing `/` is opened as written (an explicit path: not
   skipped, §3.3);
2. otherwise the first candidate `D/N` that is found by §4.4 (skip rule
   included: a candidate of another target is skipped with
   `skipped-incompatible`, `… when searching for libz.so.1`), for `D` in:
   1. the `-rpath-link` directories, in argument order, each split at
      `:` (empty components ignored; GNU ld: `-rpath-link /nonexist:hid`
      works [E31]);
   2. the `-rpath` directories, in argument order, each split at `:`
      (and `-R DIR`, §4.8);
   3. `S`'s `DT_RUNPATH`, or its `DT_RPATH` if it has no `DT_RUNPATH`,
      split at `:`, `$ORIGIN` and `${ORIGIN}` replaced by the directory
      of `S`'s found path (GNU ld finds `libf.so` through
      `$ORIGIN/../hid` [E7]); `$LIB` and `$PLATFORM` are refused
      (`feature` `$LIB in DT_RUNPATH`);
   4. the directory of `S`'s found path;
   5. the `-L` directories (§4.2 item 1).

   The sysroot (§4.3) is applied as for `-L`: a directory of steps 1, 2
   and 5 written with `=` is inside the sysroot; an absolute entry of
   step 3 is prefixed with the sysroot when there is one (it names a path
   on the target system); a `$ORIGIN` entry and step 4 are found paths
   and are not prefixed. With `--sysroot=/`, which every cross driver
   passes, every one of these is the directory as written (§4.3
   spelling). Every probe is recorded (§4.6).

The search was checked against GNU ld's `--verbose` (`found N at P`) on
four lines [E29]: the default `g++` link (`ld-linux-aarch64.so.1`,
`libgcc_s.so.1`, `libm.so.6`), the x86-64 cross line
(`ld-linux-x86-64.so.2` at `/usr/x86_64-linux-gnu/lib64`), the i686 cross
line (`ld-linux.so.2`), and `gcc e.o -l:libelf.so.1` (`libz.so.1`,
`libzstd.so.1`). On every edge the file found is the file GNU ld found
(same file by `realpath`). The spelling differs on the last line: GNU ld
says `/lib/aarch64-linux-gnu/libz.so.1` (from `ld.so.conf` and its
built-in directories), mortar finds it through step 4 as
`/usr/lib/gcc/aarch64-linux-gnu/14/../../../aarch64-linux-gnu/libz.so.1`.
Steps 4 and 5 are mortar's: GNU ld searches neither (with `-Lhid` and no
`-rpath-link` it warns `libf.so, needed by dg/libg.so, not found` [E7,
E31]) and finds those files through `ld.so.conf` instead, which the
drivers' `-L` lists reproduce. A line where the needed library sits only
in the needing library's directory or a `-L` directory therefore links in
mortar and fails in GNU ld (`./libg.so: undefined reference to 'h'`)
[E31]; §15 lists it.

**What closure files are used for.** A closure file has a `FileId`, a
row and its dynamic symbols (`spec/dyn.md` D-12 is normative; this is
its summary):

- it is **never** a `DT_NEEDED` entry of the output;
- it **defines nothing the program binds to**: a reference from a
  regular object is not satisfied by a closure file's definition (GNU
  ld: `undefined reference`, with the hint `DSO missing from command
  line`; lld and mold `undefined symbol`; E-shlib 2.10a): the reference is
  undefined with the note `defined in <c>, which is not on the command
  line`;
- when it is **used** (in U: reached by a `DT_NEEDED` edge from a needed
  or used object, D-12), its **undefined references** are used for two
  things only: a program symbol (defined in a regular object of the
  link) that it references is exported to `.dynsym`, as for a symbol a
  command-line shared object references (the `hook` case); and under
  `--no-allow-shlib-undefined` its undefined references are judged as
  those of a command-line shared object (D-13). Its definitions satisfy
  that judgement and the loader-order binding of a weak program reference
  (D-11 `bind`). A closure file that is not used contributes nothing.

`--copy-dt-needed-entries` is refused by name (`spec/dyn.md` D-15; only
GNU ld accepts it); `--no-copy-dt-needed-entries` is the default and a
`noop`. Which part of the closure a later phase uses is
`spec/resolve.md`'s and `spec/dyn.md` D-12's: only the part reachable
from shared objects that end up needed; a shared object dropped by
`--as-needed` contributes nothing, through its closure either.

**Not found.** An edge with no candidate found is the warning
`needed-not-found` at `S`, `libf.so, needed by dg/libg.so, not found
(try using -rpath or -rpath-link)` (GNU ld's wording, which tools and
users search for), once per `(N, S)`, with one note per directory
searched. The consequence is GNU ld's: program symbols referenced only
from the missing library are not exported, the missing library's
undefined references are not judged, and references of other shared
objects to symbols only it would define are judged undefined under the
rules of `spec/resolve.md` (GNU ld fails `./libg.so: undefined reference
to 'h'` on an executable [E31]).

## 9. The same file twice

### 9.1 Mentions, not files

Each mention of a file in load order is read as its own input, with these
exceptions only: a shared object (one per soname, §8.2, §8.5); a script on its
own expansion chain (a cycle, §5.3).

| named twice | mortar | references |
|---|---|---|
| an object (`f.o f.o`, `f.o ./f.o`, a hard link) | two `FileId`s: `duplicate-symbol` for its definitions | all three [E15] |
| an archive (`libf.a libf.a`, through a symbolic link, by path and by `-l`) | two `ArchiveId`s, two sets of member `FileId`s | all three: `--whole-archive libf.a libf.a` and `libf.a link.a` are duplicate definitions [E15] |
| a shared object | one `FileId` per soname: one for a library with `DT_SONAME`; for one without, one per spelling of §8.3 (§8.5) | all three [E16, E34] |
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

Mentions of one file are read once: each distinct found path is opened
once (at its first mention in load order), and found paths whose open
files have the same device and inode share one store entry; each
mention's rows point into it (§11.2). A file read once for several
mentions has one outcome: a read error, or an `input-changed` (§11.5),
is one diagnostic keyed to its **lowest** mention in load order (the key
of §12.2 of that mention), and every later mention of it contributes
nothing and no further diagnostic.

### 9.4 Identity for incremental links

`spec/model.md` §7.1 says a file's identity is "the path as resolved". It
is made exact here:

- **file**: the found path (§4.5), plus the **mention ordinal**: the number
  of earlier files in load order with the same found path (0 for the
  first). Two mentions of `f.o` are `(f.o, 0)` and `(f.o, 1)`;
- **archive member**: the archive's identity, the member name, and the
  ordinal among the archive's members of that name (model §7.1);
- with the content hash of the bytes read (§11.6), and the probes that
  chose it (§4.6): a new candidate earlier in a search changes neither
  the path nor the hash, only a probe's outcome.

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

Then (M2, when §8.6 says the closure is loaded) the closure, breadth
first in `(needing FileId, DT_NEEDED index)` order, deduplicated by
soname against every shared object already loaded: a file found takes
the next `FileId`, and its own entries join the end of the queue. The
closure is part of the input phase; nothing in it depends on resolution.

Every file opened, whatever its kind, is appended to `Inputs.link.opened`
(§10.4) in the order the walk reaches it; every probe to `Inputs.link.probes`
(§4.6).

**Errors do not stop the walk.** The rule, so that the set of
diagnostics is a function of the inputs and not of the implementation:

- a mention that produces an error before its kind is known (search not
  found, cannot open or read, not recognized, refused at sniffing,
  malformed header, `incompatible-file`, `script-cycle`, the depth
  refusal) contributes **nothing**: no row, no entries; the walk goes on
  with the next item in load order;
- a script that does not parse contributes no entries; all of the
  parser's diagnostics are emitted (script.md §2.10);
- a script that parses has **every** entry visited, whatever happened to
  earlier entries;
- an archive has every member examined (§7.3), whatever happened to
  earlier members; a malformed member contributes nothing;
- a file that was classified is checked in full (§6-§8): an error in its
  body does not prevent the checks of other files;
- the closure is still processed for every shared object that was loaded,
  so its warnings appear also in a failing link;
- the target is decided by §3.1 over the files that were classified.

The phase fails if any error was emitted (§12.3), and then gives out no
ids (`load` returns `None`). Which later entries were visited is the
same in every run: every entry not inside a mention that contributed
nothing.

Properties the walk guarantees (`spec/model.md` §2.1):

- the numbering is a function of the `Plan`, the working directory and the
  contents of the files reached;
- every relocatable member of every archive mention has a `FileId`,
  extracted or not; lazy objects of `--start-lib` too;
- `ArchiveId` is one per archive mention, in load order, also for an
  archive with no relocatable member.

### 10.2 Group ordinals

Groups are numbered in load order: from 1, in the order of their first
file in load order, over command-line groups and script `GROUP`s
together; a script `GROUP` inside a command-line group is part of it
(§5.2). (`spec/cli.md`'s ordinals count command-line groups only; they
are not used after this phase.) The numbering depends on other mentions,
so it is in the per-link table (§10.4), not in the rows.

### 10.3 Per-file rows

`files[FileId]` (`spec/model.md` §3.2) holds only what is a function of
the file's bytes and **its own mention** (the `Plan` input or script
entry that named it, with that position's state), so that I-file holds
(`spec/model.md` §3.2):

| column | value |
|---|---|
| `kind` | `Internal`, `Object`, `SharedObject` |
| `name` | the display `FileRef`: the found path, or `archive(member)` (§7.2) |
| `archive` | `(ArchiveId, member header offset)` for an archive member; for a thin member also the member's own found path |
| `origin` | the index of the `Plan` input, its argument index, and the script entry that named it, if any (an index into `Inputs.link.script_entries`: `(script file, entry offset, parent entry)`, the chain the `named by` context lines are drawn from); for a closure file, the edge `(needing FileId, DT_NEEDED index)` |
| `state` | the mention's own `as_needed`, `whole_archive`, `lazy` (a `--start-lib` object), `link_static`, `copy_dt_needed`, `exclude_libs` (below); for a shared object, its first mention's |
| `reader` | borrowed from the store's resident parts: `Inputs::object(f)`, `Inputs::shared(f)` (§11.3) |
| `sections` | `e_shnum` |
| `syms` | the range of `sym_map` |
| `identity`, `hash` | §9.4, §11.6 |

`exclude_libs` is on for a member of an archive whose found path's last
component equals a `--exclude-libs` name, or when a name is `ALL` (all
three [E26]; `ex/libex.a` matches no member, as in all three). A name
that equals a loaded archive's last component without its `.a` (`libex`)
is `invalid-option-value`: GNU ld matches it, lld and mold do not [E26].

`archives[ArchiveId]`: found path, kind (regular, thin), the `Plan` input.

### 10.4 The per-link input table

Everything the phase produces that depends on more than one mention is in
one table, `Inputs.link`, which I-file does not cover: an incremental
link recomputes it whole (it is small: rows per mention and per probe,
no bytes) and compares.

```rust
pub struct LinkInputs<'s> {
    pub group: Column<FileId, Option<GroupOrdinal>>,   // §10.2
    pub sonames: Box<[SonameUnit]>,     // one per loaded shared object, FileId order
    pub closure: Box<[ClosureEdge]>,    // §8.6: (needing FileId, index, Satisfied(FileId) | NotFound)
    pub opened: Box<[Opened<'s>]>,      // every file opened, load order
    pub probes: Box<[Probe<'s>]>,       // §4.6
    pub script_entries: Box<[ScriptEntry]>,
    pub directives: Box<[Directive<'s>]>,   // OUTPUT, ENTRY, EXTERN with script positions (§5.1)
    pub rpath_from_r: Box<[(u32, &'s [u8])]>, // §4.8
}
pub struct SonameUnit {
    pub file: FileId,
    pub mentions: Box<[MentionRef]>,    // every mention of the soname, load order
    pub as_needed: bool,                // on only if on at every mention (§8.2)
    pub copy_dt_needed: bool,           // on if on at any mention
}
```

`opened`: found path, kind (including scripts, ignored members' archives,
shared objects not loaded, closure files), origin. `--dependency-file`
lists the distinct found paths of this list in its order (GNU ld lists a
script twice, lld and mold once [E21]); `-t` prints it. The file is
lld's layout, `OUT: \`, then ` PATH \` per path (the last without the
`\`), then an empty rule `PATH:` per path after a blank line, so make
does not stop on a deleted input. Every word, the output included, is
escaped as GCC's `-MD` escapes: `\` before a space, a tab or `#`, `$`
doubled, and a run of backslashes before a space, a tab or the word's
end doubled (lld 22 escapes the paths only; GNU ld 2.44 escapes none;
mold puts the paths on one line). A path holding a newline, which make
cannot read, is refused (`feature`, `dependency file path with a
newline`). It is written after a successful link only (GNU ld writes
it on failure too). Tests: `crates/mortar/src/depfile.rs`,
`dependency_file` in `crates/mortar/tests/options.rs` (WP-027e).

## 11. The store: getting bytes into memory

### 11.1 Two stores, one interface

The store owns every byte the link reads from its inputs. It is shared
and immutable to its users: `load` and every later phase hold `&'s
Store`; whatever the store fills after creating a file's entry it fills
into a once-cell (`std::sync::OnceLock`) of that entry, and entries live
in an append-only arena of fixed segments (segment `k` holds `2^k`
`OnceLock<Entry>` slots, allocated on first use and never moved), so a
`&'s [u8]` handed out stays valid for `'s` in safe Rust.

Two stores conform to one interface (§1), chosen by `StorePolicy`:

- **(a) `Resident`** (M1's first pipeline, and the fallback for an
  input that cannot be re-read). Every file read is read whole (§11.2)
  and has one part: the whole file.
- **(b) `Staged`** (the primary store, and the default; WP-021b,
  `spec/image.md` I-81). Each file's **resident parts** are read into
  owned buffers; everything else **never enters the store**: the image
  phase copies it straight into the output with positioned reads
  (`FileExt::read_exact_at`), through `copy_to`.

The staged store's resident ranges (§11.9 gives the read plan):

| file | resident |
|---|---|
| relocatable object (little-endian `ET_REL`) larger than 64 KiB, or such an archive member | the first 64 bytes; the section header table; `.shstrtab`; every `SHT_SYMTAB`/`SHT_DYNSYM` and the section its `sh_link` names; every `SHT_STRTAB`, `SHT_SYMTAB_SHNDX`, `SHT_GROUP`, `SHT_NOTE` (so `.note.gnu.property`); every `SHT_REL`/`SHT_RELA` whose target (`sh_info`) is `SHF_ALLOC`, that is itself `SHF_ALLOC`, or whose `sh_info` names no section; every `SHF_COMPRESSED` section (the readers decode its header, `spec/elf.md` §3.9); every `SHF_MERGE` section with `sh_entsize > 0`, allocated or not, and every `.eh_frame` (and `SHT_X86_64_UNWIND` on x86-64): phase 60 splits them and interns pieces by their bytes (`spec/synth.md` S-5, S-28, §4), so they are read whole and compared, and the pieces' representatives must stay readable; every section named `.gnu.warning*` (resolve reads its text, §10 of `spec/resolve.md`); every other section of at most 64 KiB, except a Mason container (`spec/meta.md` MT-6), which is never resident; `mortar_meta` reads it with positioned reads, only when a check that needs it is on (`spec/odr.md` O-42) |
| the same, of at most 64 KiB | the whole file (member), in one read whatever it holds; when `--odr` is off, or no check that needs the container is on, a top-level file's containers are then left out of its resident ranges: their bytes are read with the file and dropped (`spec/meta.md` MT-42: exactly the reads made with no container). An archive member is not: it stays resident whole, its container with it, and the archive's other reads (the chain's windows, a large member's head and tail) are made before its section headers are known, because reading every member's section headers first would make every archive link pay, Mason or not (`spec/meta.md` MT-42, exception E2) |
| relocatable object whose section structure the rules cannot follow (`e_shentsize` not the class's, the table out of bounds, more than 2^32 − 1 sections) | the whole file: the reader finds the error in it, as over the resident store |
| regular archive larger than 64 KiB | the magic; every member header; every special member (`/`, `/SYM64/`, `//`, `__.SYMDEF*`) and every BSD-named member (`#1/n`) whole; then each ordinary member as a file above; a member larger than 64 KiB that is not a little-endian `ET_REL` (a nested archive, a shared object, a text file): its first 64 bytes, all that §7.3 reads of it |
| anything else: a shared object (with its dynamic parts), a thin archive, a script, a file of another format, any file of at most 64 KiB, a character device | the whole file |

**Not resident**: bulk `PROGBITS` (`.text`, `.data`, `.rodata` bodies
above 64 KiB), debug sections that are not `SHF_MERGE` (`.debug_info`,
`.debug_line`, `.debug_loclists`, ...), and the relocation sections of
non-allocated sections (`.rela.debug_info`): the scan and fill read
them with `copy_to` (`spec/image.md` I-13, I-82).

A test-only byte counter per file range (compiled for tests only) records
which bytes each read covered, so that T-MT-42 (`spec/meta.md`) can show
that no container byte was read when no check needs it.

Which ranges are resident is a function of the file's bytes and the
policy only, never of timing, of the thread count or of what happened to
be read first (§11.4): the rules read the structure through what was
read already or one more positioned read, and give the same ranges
either way.

**No mapping.** Every crate is `forbid(unsafe_code)`, and a memory
mapping is `unsafe` in Rust for a reason beyond the call: if another
process truncates a mapped file, reading the lost pages raises `SIGBUS`,
a crash instead of a diagnostic, and a mapped file changed during the
link changes the bytes under the linker's feet. Mapping is the expected
first request for `unsafe` (`docs/ARCHITECTURE.md`, principle 4); §11.8
is the measurement it would have to beat, and the staged store is the
safe way to most of its saving.

### 11.2 Reading

Per distinct file (§9.3):

1. `stat` (§2.5), `open`, `fstat` (size `n`, `st_mtime`, `st_ctime`,
   device, inode);
2. one owned buffer per part, of exactly the part's size (the resident
   store: one buffer of `n` bytes);
3. a part of at most 4 MiB is read by one worker with `read_exact_at`; a
   larger one in 4 MiB chunks, read in parallel into disjoint slices of
   its buffer (§12.1);
4. the checks of §11.5;
5. `close` after the last read (§11.4).

An archive's parts are those of its members plus its headers and index;
members are ranges of its blob. A thin member, and the archive named by
a nested thin member, is a file of its own, read the same way.

**Limits.** A file whose size does not fit `usize` is refused, `feature`
`input file larger than the address space`; a file of 2^40 bytes or more
is refused (`feature` `input file of 1 TiB or more`) because diagnostic
keys hold 40 bits of offset (§12.2). There is no 4 GiB limit: offsets are
`u64` throughout (`spec/model.md` §1). More than 2^32 − 1 files is
`spec/model.md` §1's refusal.

### 11.3 Parts and ranges

A file's parts are disjoint, non-adjacent ranges, sorted by offset. They
are computed from the header and section header table, which are read
first: the candidate resident ranges are collected, and ranges that
overlap or touch, or are separated by less than 4 KiB, are **merged**
into one part. So:

- a requested resident range lies in exactly one part, or in none;
  `Store::resident(b, r)` returns `Some` exactly when `r` lies inside
  one part, and a `Resident` can be made no other way, so
  `Store::bytes` cannot be asked for a non-resident range and cannot
  fail;
- a structure that straddles what would have been two parts (a section
  header table or string table that overlaps or adjoins another resident
  range, including in a malformed file) is inside one merged part;
- a structure that overlaps a non-resident range (a malformed file whose
  string table overlaps a `.debug_info` body) makes the merged part
  cover that range too: the readers never see a structure split between
  memory and disk;
- `copy_to(b, r, out)` accepts any range within the file: the bytes of
  `r` that lie in parts come from memory, the others from one positioned
  read on the file's descriptor (from the first byte of `r` not in a part
  to the last; the bytes of parts inside that read are then overwritten
  with the parts' own, so a copy never mixes two versions of a resident
  byte). A range past the end of the file is `malformed-input` before any
  copy (the readers check section bounds, elf.md §3). `copy_to` takes
  `&self` and may be called by every worker at once (positioned reads on
  a shared descriptor, `read_exact_at`). A read that ends early is
  `input-changed` (§11.5).

The object and archive readers read through a parts view (`spec/elf.md`
§3.0: every read of a structure is a `resident` call; a section body
that is not resident is `NotResident` to the caller, which uses
`copy_to`). The shared-object and script readers take whole files, which
both stores hold whole. `Store::view(b)` is the view; `Store::whole(b)`
is `Some` exactly for a file held in one part.

### 11.4 Descriptors and lifetimes

**Resident store.** A file is open only while it is read: at most one
descriptor per worker, plus one per large file being read in chunks:
never more than twice the thread count, whatever the number of inputs.

**Staged store.** A file with bytes that are not resident stays open
from its first read to the end of the link (the store's drop); a file
held whole closes after its last read, as in the resident store. The
descriptor pins the inode: a file replaced by `rename` or removed during
the link is still read from the inode that was opened, so no file mixes
the bytes of two versions through replacement. (In-place writes:
§11.5.) While loading, the files of one batch are open from their first
read to their checks (§11.9); a batch is a quarter of the budget (at
least 16, at most 1 024 files).

**The budget.** Safe Rust without dependencies can neither call
`getrlimit` nor raise `RLIMIT_NOFILE`; the soft limit is read from
`/proc/self/limits` (`Max open files`; 1 024 when it cannot be read),
and the store keeps at most three quarters of it, less 32, at least 16,
open after loading (`StoreConfig::max_open` overrides it, the tests'
hook). A file loaded when the budget is spent is made **fully resident**
(read whole, checked against its parts, closed). When `open` fails with
`EMFILE` or `ENFILE`, the staged store makes the file it took last
fully resident and retries; if it holds no file open, the error is
`cannot-open-file` with the reason. A fully resident file answers
exactly as a staged one: `resident` succeeds for the same ranges (those
inside its declared parts), and `copy_to` returns the same bytes, from
memory instead of the descriptor. So **the output does not depend on
which files became resident**, nor on when descriptors ran out; only
memory use does. The test (§16) links with an artificially small
descriptor budget and compares the output with the unbudgeted link.

### 11.5 `input-changed`: best effort

Checks, per file: one read at offset `n` (the size from `fstat`) must
return 0 bytes; a short read is an error; and a second `fstat` at the
last read of loading must give the same size, `st_mtime`, `st_ctime` and
`st_nlink` (the link count: a held file renamed over or removed loses a
link, which a filesystem with a coarse clock may not show in `st_ctime`
when the unlink comes within one tick of the last write; WP-032a measured
191 of 200 such unlinks invisible to `st_ctime` on overlayfs `/tmp`, 0 of
200 to `st_nlink`).
Otherwise the error `input-changed` at the file's lowest mention (§9.3):
`file changed while it was read`. The staged store makes the checks
after its parts are read (§11.9 wave D), so a file truncated between its
first reads and its parts is `input-changed`, never a part of zeros or a
panic. After loading, a staged file's `copy_to` that reads short is
`input-changed` (`CopyError::Changed`), and `Store::recheck(b)` repeats
the `fstat` check on the held descriptor, for the end of the image
phase.

What is **guaranteed**: the bytes hashed are the bytes linked, per part
(§11.6): no byte reaches a table or the output that was not part of the
read that was hashed. What is **not guaranteed**: a concurrent writer
that rewrites a file in place, without changing its size, inside the
timestamp granularity, can produce a read that is neither version (large
parts are read in parallel chunks) and that no check detects; the
reviewer measured 199 of 200 such same-size writes undetected on `/tmp`
(and 0 of 200 on the workspace mount) [review of 2026-11-14, not
re-measured]. No linker detects this; build systems do not write inputs
during a link. The check is a diagnostic aid, not a proof.

### 11.6 Content hash

`ContentHash` of a file is one function of the whole file's bytes (the
hash function is the incremental spec's; `spec/model.md` §7.1, M7). Both
stores compute the same value for the same bytes, so the identities of
§9.4 do not depend on the store:

- **resident store**: computed over the buffer as it is read: the bytes
  hashed are exactly the bytes linked;
- **staged store**: computed **lazily**, only when `hash(b)` (or
  `hash_range(b, r)`, an archive member's) is called (by an incremental
  link, or a walk whose `StoreConfig::hashes` asks for `File::hash`; a
  plain link never calls it, and `File::hash` is then 0). `hash(b)`
  streams the file through positioned reads into a buffer of 4 MiB (the
  hash's block) that is not kept, computes the file's hash, and checks
  every resident part against the same range of the stream, byte for
  byte; a mismatch, or a read that ends early, is `input-changed`
  (`ReadError::Changed`). So the hashed bytes are the resident bytes
  linked, and the hash is the whole-file hash. The bytes `copy_to`
  hands out are not recorded (that would hash every byte the image
  copies on every link, for a hash only an incremental link asks for):
  for them the guarantee is §11.5's, `recheck` at the end of the image
  phase (`spec/landed/input/WP-021b.md`, ambiguity 3).

**`File::hash` 0 is "not computed".** Under the staged store without
`StoreConfig::hashes`, the walk does not hash and `File::hash` is
`ContentHash(0)`. That value is a sentinel, not a hash: no code may
read a 0 as evidence that a file is unchanged. Whether the hashes were
computed is `Store::hashes()`; code that needs a hash (an incremental
link) asks `Store::hash` or loads with `StoreConfig::hashes` set.

**`copy_to` ranges (M1).** M1 does not record the ranges `copy_to`
hands out. M7 (incremental linking) records them, with their hash,
behind a `StoreConfig` flag that only an incremental link sets, so a
plain link never pays for it.

What incremental linking then requires of a staged link: one further
read of every byte of every staged file, without memory (a sequential
read, as fast as the resident store's first read), or the resident
store. The incremental spec chooses.

### 11.7 Memory

**Resident input bytes equal the bytes of every resident part**, from
the read to the end of the link, and they are anonymous memory: not page
cache that the kernel can drop and re-read, as a read-only mapping is.
Measured (§11.8): reading `cxx-medium-g`'s 200.8 MiB of inputs leaves
201.1 MiB anonymous; mapping and touching every page leaves 202.6 MiB of
file-backed resident pages and 0.2 MiB anonymous. The peak a user sees
in `top` is the same; the difference is what the kernel can do under
memory pressure.

What the staged store saves, measured on `cxx-medium-g`'s two archives
(201 MB): symbol and string tables are 21.5 MB and section headers 9.2
MB, about 15%; debug sections and their relocations are 147.8 MB, 73%
[E22]. The parts must be separate buffers: a zero-filled buffer of the
file's size with only some ranges read is not a saving, because on this
machine (transparent huge pages `always`) a 512 MiB `vec![0u8; ..]`
costs nothing until written, but writing one byte per MiB makes 161 MiB
resident [E23].

Measured with the staged store (WP-021b, `spec/landed/input/WP-021b.md`):
of `cxx-medium-g`'s 210.6 MB of inputs, 129.8 MB is resident. Of the two
archives' 201 MB: `.debug_str` and the other non-allocated `SHF_MERGE`
sections 64.6 MB, symbol, string, group and note tables 22.1 MB, section
headers 9.2 MB, archive indexes and name tables 7.2 MB, relocations of
alloc sections 6.9 MB, other sections of at most 64 KiB 11.6 MB,
`.eh_frame` 1.8 MB; not resident: relocations of non-alloc sections
45.9 MB and large sections 32.8 MB. Without the merge sections the
resident share would be `spec/image.md` I-93's 59.9 MB; phase 60 reads
them whole and keeps its representatives' bytes (`spec/synth.md` S-5,
S-28), so they are resident here (the landed record's ambiguity 1). The
peak formula for both stores is `spec/image.md` I-92's.

### 11.8 Read against map: measurement

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
copied by `copy_file_range`): the staged store (§11.1) is the safe way to the same
saving.

### 11.9 The staged store's read plan

`read_many` reads its jobs in batches (§11.4), each in four parallel
waves; the parts are a function of the bytes alone (§11.1), whatever the
waves read first.

- **(A) per file**: `open`, `fstat`. A file of at most 64 KiB, or one
  that is neither a little-endian `ET_REL` nor a regular archive, is one
  range: the whole file (a little-endian `ET_REL` of at most 64 KiB with
  a Mason container when no check needs the container: the whole file
  is read in this wave, in one read, and the ranges are the file less
  its containers, §11.1's row). Otherwise one positioned read of its first
  `StoreConfig::head` bytes (4 KiB). An object: one read of its last
  `StoreConfig::tail` bytes (64 KiB), where GCC, Clang and `as` put the
  section header table, `.symtab`, `.strtab` and `.shstrtab`; then the
  rules of §11.1, each structure not inside what was read costing one
  more read. An archive: the chain of member headers, each miss one read
  of a window from the header (`head` + 64 bytes, doubled while the
  misses follow each other closely, as in a run of small members, up to
  1 MiB): a member's first bytes come with its header.
- **(B) per member larger than 64 KiB** that is an object: one read of
  its last `tail` bytes, then the rules.
- Candidate ranges are merged into parts (§11.3).
- **(C) per part**, in 4 MiB chunks: the bytes already read are copied;
  the rest is one positioned read per chunk.
- **(D) per file**: the checks of §11.5. A file held whole closes; any
  other stays open within the budget (§11.4).

The plan reads every byte of a large object at most once beyond its
first `head` and last `tail` bytes, and makes about four positioned reads
per large object (`spec/landed/input/WP-021b.md` measures it).

## 12. Parallelism and determinism

### 12.1 What runs in parallel

| step | unit | depends on |
|---|---|---|
| `stat` and sniff of `Path` inputs; the target by §3.1 rule 2 | `Plan` input | nothing |
| search of each `-l` over the `-L` part of the search path | `Plan` input | the target (to skip incompatible candidates); under §3.1 rule 3, the searches before the target-setting file in load order take their first candidate and are final |
| reading | file, or 4 MiB chunk of a large file | its path |
| parsing scripts; resolving and reading their entries | script; entry | the script's bytes |
| per object and per archive member: header, section headers, the symbol-table pass (§6: LTO check, interning slot) | file / member | its bytes |
| the walk of §10.1 (counting only) and the prefix sums of `FileId`s | sequential | the kinds of all files reached |
| the closure search of each `DT_NEEDED` edge (§8.6) | edge | the needing file's dynamic table; the numbering is assigned afterwards, sequentially, in queue order |
| `SymbolId`, `ComdatId` numbering | `spec/model.md` §2.3 | `FileId`s |

The search path of an `-l` after a script includes the script's
`SEARCH_DIR`s (§4.2) only after every `-L`. So a search that finds a
candidate in the `-L` directories is final whatever any script says, and
only a search that fails there waits for the scripts before it in load
order (under `-nostdlib`, none waits): in practice every search is final in the first wave. The walk
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
mold the first only [E20]. Which entries are still visited after an
error is the rule of §10.1: a mention that fails contributes nothing,
every other entry is visited, and the phase emits no id when it fails.

Whether a diagnostic is emitted, and its key, are functions of the inputs
(`spec/diag.md` §6.1): the order in which workers finish is not observable.
Tests run the phase under `MORTAR_SCHED_SEED` and thread counts 1, 2, 7,
64 and compare `Inputs` and the rendered report (§16).

## 13. Diagnostics

All with phase 20 and the key of §12.2. "New" kinds are proposed for
`spec/diag.md` §3.1 (§17), in the files area unless noted.

**Wording policy** (requested for `spec/diag.md`, §17): where GNU ld has a
phrase for a condition that configure scripts and build tools match,
mortar's message for the same condition contains that phrase verbatim.
The phrases and the kinds that carry them:

| GNU ld phrase | kinds |
|---|---|
| `cannot find -l{name}` (and `cannot find -l:{file}`) | `library-not-found` |
| `skipping incompatible {path} when searching for {-lname or name}` | `skipped-incompatible` |
| `file not recognized: file format not recognized` | `unknown-file-type`; the `file-format` refusals of §2.1 rows 4-7 (GNU ld says it for each); `cannot-open-file` for a directory |
| `No such file or directory` | `cannot-open-file` and `library-not-found` (the reason, `strerror(ENOENT)`) |
| `attempted static link of dynamic object` | `static-link-of-shared-object` |
| `needed by {path}, not found (try using -rpath or -rpath-link)` | `needed-not-found` |

| condition | severity | kind id | message (wording source) | GNU ld 2.44 says |
|---|---|---|---|---|
| a path does not exist / cannot be opened or read | error | `cannot-open-file` | `cannot open {path}: {reason}` (lld, mold) | `cannot find {path}: No such file or directory` |
| a directory as input | error | `cannot-open-file` | `cannot open {path}: Is a directory; file format not recognized` | `cannot find dir: file format not recognized` |
| a `-l` candidate cannot be examined (`ELOOP`, `EACCES`, a directory) | error | `cannot-open-file` | as above | skips it |
| `-l` finds nothing | error | `library-not-found` | `cannot find -l{name}: No such file or directory` (GNU ld) + notes §4.4 | the same |
| a FIFO, socket or block device as input; a non-empty character device | error | `malformed-input` | `is a FIFO` / `is a socket` / `is a block device` / `not a regular file and not empty` (own) | FIFO: hangs, or `Illegal seek` |
| `-L ''`; a `--exclude-libs` name GNU ld alone matches | error | `invalid-option-value` | `{option}: invalid value '{value}'` | — |
| `=` + relative directory with a sysroot (`-L`, scripts) | error | `ambiguous-sysroot-path` (new) | `ambiguous sysroot path '{spelling}'`, notes with both readings (own) | concatenates |
| an incompatible candidate of any search (§4.4) | warning | `skipped-incompatible` (new) | `skipping incompatible {path} when searching for -l{name}`, or `… for {name}` for a script entry or closure edge (GNU ld) | the same |
| no `-m` and no ELF input | error | `no-target` (new) | `target emulation unknown: -m or at least one .o file required` (lld) | uses its default |
| an ELF file of another target | error | `incompatible-file` (new) | `incompatible file: {expected} is expected but got {found}` (mold), note: what set the target | `Relocations in generic ELF (EM: 62)` / `file in wrong format` |
| bytes of no known format | error | `unknown-file-type` | `{path}: file not recognized: file format not recognized` (GNU ld) | the same |
| a format named and not read (§2.1 rows 4-7) | error | `refused` | `unsupported file format: Mach-O; file not recognized: file format not recognized` | `file format not recognized` |
| a GCC slim LTO object | error | `refused` | `unsupported file format: GCC LTO object`, note naming link-time optimization (§2.2) | performs LTO through the plugin |
| a script that does not parse | error | `malformed-input` | script.md §2.9 | `syntax error` |
| a structural error of an ELF file or an archive | error | `malformed-input` | the reader's text | `file format not recognized` |
| a malformed archive symbol index | warning | `malformed-input` | `the symbol index is malformed and is not used: {what}` (own) | `file format not recognized` |
| a member that is not a relocatable object | warning (lazy), error (`--whole-archive`) | `ignored-member` (new) | `archive member is not a relocatable object: {what}`, at `archive(member)` (own; lld: `archive member 'x' is neither ET_REL nor LLVM bitcode`) | silent; whole: `member a.a(x) in archive is not an object` |
| a file changed while read (best effort, §11.5) | error | `input-changed` (new) | `file changed while it was read` (own), at the lowest mention | — |
| a script cycle | error | `script-cycle` (new) | `linker script names itself`, `named by` chain (own) | loops |
| a shared object of a soname already loaded, different bytes | warning | `duplicate-soname` (new) | `DT_SONAME '{soname}' is already loaded; this file is not used`, note `loaded from {path}` (own) | silent |
| a shared object while `link_static` | error | `static-link-of-shared-object` (new) | `attempted static link of dynamic object {path}` (GNU ld, lld) | the same |
| a needed library not found (§8.6) | warning | `needed-not-found` (new) | `{name}, needed by {path}, not found (try using -rpath or -rpath-link)` (GNU ld), notes: the directories searched | the same |
| descriptors exhausted with no staged file to make resident | error | `cannot-open-file` | reason `Too many open files` | — |
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
| `feature` | `--just-symbols` | §4.8 |
| `feature` | `linker scripts nested more than 16 deep` | §5.3 |
| `feature` | `shared object without section headers` | §8.1 |
| `feature` | `shared object without DT_SONAME named by a linker script` | §8.3 |
| `feature` | `dynamic executable without -dynamic-linker` | §8.4 |
| `feature` | `$LIB in DT_RUNPATH`, `$PLATFORM in DT_RUNPATH` | §8.6 |
| `feature` | `input file larger than the address space`, `input file of 1 TiB or more` | §11.2 |

## 15. Where the references differ

| case | GNU ld | lld | mold | mortar | § |
|---|---|---|---|---|---|
| target with no `-m` | built-in emulation | first ELF file in load order | first ELF file named by path | mold's rule, then lld's | §3.1 |
| incompatible candidate of a search (`-l`, `-l:`, script entry incl. script-directory and working-directory probes, script with another `OUTPUT_FORMAT`) | skip, warning | error | skip, warning | skip, warning | §4.4 |
| lazy archive member of another machine | silent | error | error | error | §3.3 |
| a directory where `-l` looks | skipped | error | error | error | §4.4 |
| a symbolic-link loop where `-l` looks | skipped | skipped | error | error | §4.4 |
| `-L ''` | finds nothing | working directory | finds nothing | error | §4.3 |
| `=dir` (no `/`) with a sysroot | concatenates | joins with `/` | concatenates | error | §4.3 |
| found path, `--sysroot=S/` | `S//lib/…` | `S/lib/…` | not checked | `S/lib/…` | §4.3 |
| `=dir` without a sysroot | `dir` | `dir` | not found | `dir` | §4.3 |
| `$SYSROOT` prefix on `-L` | sysroot | literal | sysroot | sysroot | §4.3 |
| `=` on a command-line path | sysroot | literal | literal | refused | §4.1 |
| built-in search directories | 7 (or 11) | none | none | none | §4.2 |
| `SEARCH_DIR` | applies to later `-l` | applies to later `-l` | unsupported | applies to later `-l` | §4.2 |
| `-nostdlib` and `SEARCH_DIR` | ignored | ignored | — | ignored | §4.2 |
| script cycle | hangs | crashes | hangs | error | §5.3 |
| `STARTUP` | supported | unsupported | unsupported | refused | §5.1 |
| empty file, `/dev/null` as input | empty script | empty script | error | empty script | §2.3, §2.5 |
| a FIFO as input | hangs (empty) or error | reads it | error | error, without opening | §2.5 |
| GCC slim LTO object, no plugin | warning, linked without its code | linked without its code | error | refused | §2.2 |
| `gcc -flto` line (with `-plugin`) | LTO | — | — | refused, naming LTO | §2.2 |
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
| `--copy-dt-needed-entries` | supported | unknown option | unknown option | refused by name (`spec/dyn.md` D-15; withdrawn from "supported (M2)") | §8.6 |
| `DT_NEEDED` closure for an executable | loaded: exports program symbols it references | not loaded | not loaded | loaded (M2) | §8.6 |
| closure search in the needing library's directory and `-L` | no (uses `ld.so.conf`, built-in directories) | — | — | yes, after `RUNPATH` | §8.6 |
| `DT_NEEDED` search via `LD_LIBRARY_PATH` | yes | — | — | no | §8.6 |
| `-b binary` | supported | supported | refused | refused | §2.6 |
| `-R FILE` | just symbols | rpath | refused | refused | §4.8 |
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
  `s2_2_flto_line_refused_naming_lto` (the `gcc -flto` argv, `-plugin`
  included); `s2_3_empty_file`, `s2_3_dev_null` (as input and with `-r`);
  `s2_4_header_errors`; `s2_5_directory`, `s2_5_fifo_not_opened` (a FIFO
  with no writer: the phase returns, the error is emitted, the FIFO was
  never opened), `s2_5_socket`, `s2_5_nonempty_char_device`;
  `s2_6_formats`.
- **§3**: `s3_1_m_decides`; `s3_1_first_path_input` (a shared object
  first, an archive first: its first relocatable member, lazy);
  `s3_1_scripts_and_l_do_not_count_in_rule_2`; `s3_1_rule_3_fallback`,
  `s3_1_rule_3_load_order` (searches before the first ELF file in load
  order take their first candidate, after it skip; under every schedule);
  `s3_1_no_target`; `s3_2_check_target_runs_when_no_m` (with the key);
  `s3_2_output_format_checked_late`; `s3_3_incompatible_object`,
  `s3_3_incompatible_lazy_member`, `s3_3_note_names_the_decider`,
  `s3_3_explicit_path_never_skipped` (a `Path` input and an absolute
  script entry of another target);
  `s3_4_flags_hooks_called_in_fileid_order` (a recording `Arch`).
- **§4**: `s4_1_paths_not_searched`, `s4_1_dash_is_a_file`,
  `s4_1_equals_path_refused`; `s4_2_l_after_L`,
  `s4_2_search_dir_applies_later_only`, `s4_2_no_builtin_dirs`,
  `s4_2_nostdlib_drops_search_dir` (command-line `-l` and the script's
  own `INPUT(-lq)`);
  `s4_3_sysroot_table` (each row), `s4_3_sysroot_spelling` (`--sysroot=/`
  gives `/usr/…`; `--sysroot=S/` gives `S/lib`), `s4_3_empty_L`;
  `s4_4_shared_before_static_per_directory`, `s4_4_bstatic`,
  `s4_4_l_colon`, `s4_4_candidate_stat_table` (dangling link, directory,
  loop), `s4_4_skip_table` (one case per search form × candidate form:
  `-l`, `-l:`, a script's relative entry through the script directory,
  the working directory and the search path, a closure probe; × a shared
  object, an archive, a relocatable object, an `ET_EXEC`, a script with
  another `OUTPUT_FORMAT`; the warning text, with and without `-l`),
  `s4_4_not_found_notes` (directories listed; skipped candidates; the
  `-Bstatic` note; the GNU ld phrase);
  `s4_5_found_path_is_not_normalized` (`-L./d/../d`, `-Ld/`);
  `s4_6_probes_recorded` (every probe in load order with its outcome),
  `s4_6_new_candidate_changes_a_probe` (a `libf.so` created where
  `libf.a` was taken; a library in an earlier `-L`; a file in the
  working directory shadowing a script entry);
  `s4_7_environment_is_ignored` (`LIBRARY_PATH`, `LD_LIBRARY_PATH`,
  `LD_RUN_PATH` set to directories that would change the result);
  `s4_7_rpath_link_is_not_an_l_directory`; `s4_8_r_directory_is_rpath`,
  `s4_8_r_file_refused`.
- **§5**: `s5_1_commands` (each row); `s5_1_system_scripts` (the
  `libc.so`, `libm.so`, `libgcc_s.so` of the three targets under `/usr`,
  expanded to the expected files; `libgcc_s.so.1` found through the
  search path, past an x86-64 `-L` on the i686 line); `s5_2_state_table` (as-needed inside
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
  `s8_4_no_interpreter_refused`; `s8_5_sonameless_spellings` (three
  spellings, three files); `s8_6_when_loaded` (executable, PIE, shared
  with and without `--no-allow-shlib-undefined`, `-r`);
  `s8_6_closure_search_order` (each of the five steps shadowing the
  next), `s8_6_rpath_link_colon_split`, `s8_6_origin`,
  `s8_6_sysroot_applied`, `s8_6_skip_applies`,
  `s8_6_as_needed_objects_have_closures`, `s8_6_not_found_warning`,
  `s8_6_numbering` (breadth first, `(needing FileId, index)`, dedup by
  soname); `s8_6_system_lines` (the four lines of [E29]: the file found
  equals GNU ld's `found … at` by `realpath`).
- **§9**: `s9_1_mentions` (object, archive and shared object twice;
  symbolic and hard links); `s9_3_read_once` (a counting store), `s9_3_error_keyed_to_lowest_mention`;
  `s9_4_identity` (mention ordinals; the identity survives replacing a file
  by rename).
- **§10**: `s10_1_walk_reference` (a reference implementation of the
  pseudocode in the test, compared with `load` on generated command lines:
  objects, archives with object and non-object members, shared objects
  with repeated sonames, nested scripts, groups, lib regions);
  `s10_1_errors_do_not_stop_the_walk` (one fixture per bullet of the
  error rule: the set of diagnostics is equal under every schedule);
  `s10_2_group_ordinals`; `s10_3_rows_are_per_mention` (a row is equal
  when another mention or another script changes); `s10_3_exclude_libs`;
  `s10_4_link_table` (merged `as_needed`, `copy_dt_needed`, groups),
  `s10_4_opened_and_dependency_file`.
- **§11**: every walk and `load` test runs under both stores and asserts
  equal tables (`tests/common`: `run_plan`, `s1_load::load_in`; the
  tables as text without content hashes and parts shapes;
  `MORTAR_TEST_STORE=resident|staged` runs one);
  `s11_1_staged_resident_and_bulk` (a large `-g` object: the structure
  resident, `.debug_info` and `.rela.debug_info` not, the parts view
  parsing as the whole file); `s11_1_both_stores_agree_on_large_inputs`
  (objects, a regular and a thin archive with large members);
  `s11_1_staged_huge_sparse_archive` (a 1 GiB member of zeros: under 4
  MiB read); `s11_3_parts_and_copy_to` (parts disjoint and at least 4 KiB
  apart, `resident` only inside one part, `copy_to` of straddling ranges
  equal to the file, past the end refused); `s11_3_member_view`;
  `s11_3_copy_to_from_64_threads` (every section of every file of a
  link, from 64 workers at once); `s11_4_descriptor_bound` (5 000
  inputs, both stores: at most twice the thread count open while
  reading); `s11_4_staged_descriptor_budget` (a budget of 2 through
  `StoreConfig::max_open`: 6 of 8 files fully resident, the same bytes);
  `s11_4_staged_emfile_falls_back` (run again under `ulimit -n 48` with a
  budget above it: `EMFILE` makes held files fully resident);
  `s11_4_staged_many_small_files`; `s11_4_rename_between_stages`;
  `s11_5_changed_size`, `s11_5_short_read` (a file truncated between
  `fstat` and read, through a test hook in the store), `s11_5_grew`;
  `s11_5_staged_truncated_between_stages` (the `after_head` hook);
  `s11_5_staged_changed_after_load` (`copy_to`, `hash` and `recheck`
  give `Changed`); `s11_5_staged_fifo`;
  `s11_2_chunked_read_equals_whole_read`; `s11_6_staged_hash_on_demand`
  (equal to the resident store's, files and members; `File::hash` 0
  unless asked); `s12_staged_determinism` (`check_schedules`: tables and
  parts).
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
| `incompatible-skip.test` | `-L<x86 dir> -L<native dir> -lf` links and warns, also for `-l:crti.o` and a script with another `OUTPUT_FORMAT` (GNU ld, mold; `skip(lld)`); runs on one architecture with the other's library |
| `multiarch-i686.test` | `i686-linux-gnu-gcc imm.o -L/usr/x86_64-linux-gnu/lib -lm` links an i386 program (GNU ld; `skip(lld,mold)`); i386 only |
| `nostdlib-search-dir.test` | `-nostdlib` drops a script's `SEARCH_DIR` (`exit: fail`, GNU ld, lld; `skip(mold)`) |
| `dev-null-input.test` | `/dev/null` as an input and with `-r` (GNU ld, lld; `skip(mold)`) |
| `closure-export.test` | the `hook` case: a program symbol referenced only by an indirect dependency is exported and the program prints 42 (GNU ld; `xfail(lld,mold)`: not exported) |
| `incompatible-object.test` | an object of another architecture fails (`exit: fail`, all three) |
| `executable-input.test` | an `ET_EXEC` and a PIE as inputs fail (`ET_EXEC`: all three; PIE: GNU ld, `skip(lld,mold)`) |
| `copy-dt-needed.test` | `--copy-dt-needed-entries` is refused by name (`spec/dyn.md` D-15, test `copy-dt-needed-refused`) |
| `rpath-origin.test` | a needed library found through `$ORIGIN` in `DT_RUNPATH` (GNU ld; others skip) |
| `rpath-link-colon.test` | `-rpath-link /nonexist:dir` (GNU ld; others skip) |

## 17. Requested changes to other specs

Each row: the section, its current text, the proposed text, the reason.
None of these specs is edited by WP-021.

**`spec/model.md`**

| section | current text | proposed text | reason |
|---|---|---|---|
| §1, "Input bytes belong to the input store" | "A phase obtains the bytes of a file or a section from `mortar_input`'s store by `(FileId, range)` or `SectionId`. Whether the store holds a file for the whole link, reads a range again when asked ... is the store's business and invisible to the tables." | "A phase obtains the bytes of a file or a section through `Inputs` by `(FileId, range)` or `SectionId`: as a slice when the range lies in one resident part (`Inputs::resident`, `Inputs::bytes`), otherwise copied into a buffer of the caller (`Inputs::copy_to`). The store is created before `mortar_input` runs, outlives every table, and is shared and immutable to its users (`&'s Store`); which ranges are resident is a function of the file's bytes and the store policy (`spec/input.md` §11)." | §1, §11: two stores, one interface; no `&mut Store` |
| §2, `ArchiveId` row | "in order of first appearance on the command line" | "one per archive mention, in load order (`spec/input.md` §10.1)" | each mention is an archive of its own in all three references (§9.1); positional state differs per mention |
| §3.2 `target` | "the first object decides when `-m` did not, and every later file must agree or is diagnosed" | "decided by `spec/input.md` §3.1 when `-m` did not; every ELF file must agree (§3.3), except candidates of a search, which are skipped (§4.4)" | the rule is two-tier, counts shared objects and archive members, and is stated in load order |
| §3.2 `files` | "the positional state copied from it (as-needed, whole-archive, lazy, static, exclude-libs)" | "the positional state of its own mention (as-needed, whole-archive, lazy, static, copy-dt-needed, exclude-libs; a shared object: its first mention's)" | §10.3 |
| §3.2 `files` | "the parsed reader (`mortar_elf::Object` or the shared-object view) over the store's bytes" | "the parsed reader over the file's resident parts (`spec/input.md` §11.3)" | a staged file is not wholly in memory |
| §3.2, parts table | — | add the part `link` (`LinkInputs`, `spec/input.md` §10.4): group ordinals, the soname units with merged `as_needed` and `copy_dt_needed`, the closure edges, `opened`, `probes`, `script_entries`, `directives`, `rpath_from_r` | §10.4 |
| §3.2, I-file | "the rows of a file ... are a function of that file's bytes and its `Plan` input alone" | "the rows of a file ... are a function of that file's bytes and its own mention (the `Plan` input or script entry that named it, with that position's state) alone. Properties merged over several mentions or numbered over the load order (`spec/input.md` §10.4) are in `Inputs.link`, which I-file does not cover; an incremental link recomputes it whole" | the merged `as_needed`, `copy_dt_needed` and `group` depend on other mentions (§8.2, §10.2) |
| §3.2, "Immutable once the phase ends" | — | add: "The `DT_NEEDED` closure (`spec/input.md` §8.6) is loaded by this phase; no later phase adds a file" | §8.6 removes the circularity with resolution |
| §7.1 file | "the path as resolved, plus member name and ordinal ..." | "the found path (`spec/input.md` §4.5) and the mention ordinal among files with that path, plus member name and ordinal ...; with a content hash of the whole file (`spec/input.md` §11.6) and the probes that chose it (§4.6), re-checked by one `stat` each" | two mentions of one path are two files (§9.4); a new candidate earlier in a search changes neither path nor hash |
| §8, input row, `seq` | "byte offset of the construct in the file as given" | "`k × 2^40 + offset` (`spec/input.md` §12.2)" | one input reaches several files (a script, a thin archive, the closure) |
| §9 | "names are slices of input bytes, hashed once" | "names are slices of resident input bytes (`&'s [u8]` borrowed from the shared store, `spec/input.md` §11), hashed once; names always lie in resident parts" | the staged store keeps string tables resident |
| §10 | `pub struct Inputs<'store> { pub target: LinkTarget, pub files: Column<FileId, File<'store>>, /* §3.2 */ }` | add `pub fn load<'s>(plan: &Plan, env: &LoadEnv, store: &'s Store, diag: &Collector) -> Option<Inputs<'s>>` (in `mortar_input`), `pub link: LinkInputs<'store>`, and the `Store` interface of `spec/input.md` §1 | §1 |

**`spec/elf.md`**

| section | current text | proposed text | reason |
|---|---|---|---|
| §6 (WP-006) | — | the shared-object reader provides what §8.1 lists (`DT_SONAME`, `DT_FLAGS_1`, `DT_NEEDED` in table order, `DT_RUNPATH`, `DT_RPATH`, dynamic symbols with versions, presence of section headers); `SharedObject::parse` with section headers required is the entry `mortar_input` uses | §8.1, §8.6 |
| §3 (later stage) | `Object` borrows the whole file | a reader over a parts view: every structure read through `resident(range) -> Option<&[u8]>`; a structure not inside one part is `malformed-input`; a section body not resident is reported to the caller as `NotResident`, which copies it with `copy_to`; parts are disjoint and non-adjacent, so no structure spans two | §11.3 |

**`spec/image.md`** (WP-021b)

| section | current text | proposed text | reason |
|---|---|---|---|
| I-93, the resident figure | the staged store's resident share of `cxx-medium-g` is 59.9 MB | 129.8 MB: the 59.9 MB plus the non-alloc `SHF_MERGE` sections (`.debug_str` and its kin, 64.6 MB) and the other merge, `.eh_frame` and note sections, which §11.1 keeps resident because phase 60 splits and interns them by their bytes (`spec/synth.md` S-5, S-28) | measured (§11.7); accepted for M1 |
| the M2 memory items | — | add: "stream the non-alloc `SHF_MERGE` sections through `copy_to` in phase 60, keeping only the representatives' bytes (about 10 MB of `cxx-medium-g`'s 64.6 MB), and make them not resident in §11.1" | the largest resident share left after M1 |

**`spec/cli.md`**

| section | current text | proposed text | reason |
|---|---|---|---|
| options table (`crates/mortar_cli/src/rows.rs`), `-nostdlib` | Refused, "Search only the -L directories" | accepted: "Do not search the `SEARCH_DIR` directories of linker scripts (`spec/input.md` §4.2)"; sets `Plan.nostdlib` | GNU ld and lld drop script `SEARCH_DIR`s [E32] |
| options table, `-rpath-link` | Refused, "Search DIR for the DT_NEEDED entries of shared objects" | accepted (M2): "Search DIR for the DT_NEEDED entries of shared objects; a `:`-separated list (`spec/input.md` §8.6)"; `Plan.rpath_link` keeps each argument as written, split by `mortar_input` | §8.6; GNU ld splits at `:` [E31]; CMake emits lists |
| options table, `--allow-shlib-undefined`, `--no-allow-shlib-undefined` | Refused | accepted (M2), into the `Plan` | §8.6 decides when the closure loads by it |
| §2.9 | "The phase that reads it calls `check_target` (follow-up for WP-021)." | "`mortar_input` calls it as `spec/input.md` §3.2 says; its diagnostics take phase 20" | §3.2 |
| §7.2, `-R` row | "the directory case needs the file system, and is the input phase's to decide (follow-up)" | "`spec/input.md` §4.8" | §4.8 |

**`spec/script.md`**

| section | current text | proposed text | reason |
|---|---|---|---|
| §2.3 `STARTUP` | "(manual: like `INPUT`, but the file becomes the first input of the link). lld and mold do not implement it" | add: "`mortar_input` refuses it (`spec/input.md` §5.1)" | no system script uses it |
| §2.6 | "GNU ld applies a script's `SEARCH_DIR`s to all of that script's `-l` entries, also to those before it ...; lld only to later ones" | add: "GNU ld and lld also apply them to every command-line `-l` after the script, neither to one before it; under `-nostdlib` neither applies them at all (`spec/input.md` §4.2)" | [E10], [E32] |
| §2.4 rule 2 | "mortar MUST make that an error naming both readings" | "... the error `ambiguous-sysroot-path` (`spec/input.md` §4.3), shared with `-L`" | one kind for one condition |
| §2.4 rule 3 | the relative entry is looked for in the script's directory, then the current directory, then the library search path | add: "Each of these probes is a search: a candidate of another target is skipped with `skipped-incompatible` and the search continues (`spec/input.md` §4.4); every probe is recorded (§4.6)" | GNU ld skips in all three places [E27, E28] |
| §2.5 `OUTPUT_FORMAT` | — | add: "a script found by a search whose `OUTPUT_FORMAT` names another target is skipped (`spec/input.md` §4.4)" | [E27] line (c) |

**`spec/archive.md`**

| section | current text | proposed text | reason |
|---|---|---|---|
| §5, §6.5 | `parse` fails with `BadIndex` for the problems marked "parse" | `parse` succeeds and `symbol_index()` returns `Option<Result<SymbolIndex, Error>>`; or a `parse_without_index` that skips the index member | §7.1: the index is unused; lld and mold link such archives |
| §7.2 | "`name()` is the same path, which is what `ar t` lists" | "... which is what `ar t` lists when run in the archive's directory; run elsewhere, `ar t` joins it with the archive's directory (`sub/objs/f.o` for `sub/libt.a`)" | [E13]; the display rule of §7.2 |

**`spec/diag.md`**

| section | current text | proposed text | reason |
|---|---|---|---|
| §3.1 | — | the new kinds of §13: `ambiguous-sysroot-path`, `duplicate-soname`, `ignored-member`, `incompatible-file`, `input-changed`, `needed-not-found`, `no-target`, `script-cycle`, `skipped-incompatible`, `static-link-of-shared-object` | §13 |
| §3.1, `library-not-found` row | `unable to find library -l{name}` (lld) | `cannot find -l{name}: No such file or directory` (GNU ld) | the wording policy |
| §3.1, `unknown-file-type` row | `unknown file type` | `{path}: file not recognized: file format not recognized` (GNU ld) | the wording policy |
| §8.2 | argues for lld's wording of `library-not-found` | replace by the **wording policy**: "where GNU ld has a phrase for a condition that configure scripts and build tools match, mortar's message for the same condition contains that phrase verbatim", with the table of `spec/input.md` §13 (`cannot find -l{name}`; `skipping incompatible {path} when searching for …`; `file not recognized: file format not recognized`; `No such file or directory`; `attempted static link of dynamic object`; `needed by …, not found (try using -rpath or -rpath-link)`), and the rule that a refusal of a format GNU ld does not recognize carries the phrase after its own text | the operator's ruling; tools grep these phrases |

**`spec/resolve.md`** (not yet written; requirements for its M2 part)

| section | current text | proposed text | reason |
|---|---|---|---|
| the M2 part, shared objects | — | "The `DT_NEEDED` closure is in `Inputs` (`spec/input.md` §8.6). A closure file is never `DT_NEEDED` of the output and defines nothing the program binds to (`--copy-dt-needed-entries` is refused, `spec/dyn.md` D-15). Only the part of the closure reachable from shared objects that end up needed is used: (1) a program symbol defined in a regular object and referenced by a used closure file is exported; (2) under `--no-allow-shlib-undefined`, the undefined references of command-line shared objects and of used closure files are judged; a reference whose definer was `needed-not-found` is judged undefined" | §8.6: GNU ld's behaviour [E30, E31]; keeps `Inputs` immutable |

## 18. Open questions

1. **Empty input files.** Accepted as empty scripts (GNU ld, lld,
   script.md §3.4), `/dev/null` included. An empty `.o` is more often a
   crashed compiler than an intended script. Recommendation: keep, as two
   references and the script spec agree; revisit with a warning kind if
   users are bitten.
2. **Skipping incompatible candidates** (§4.4) is a warning, so
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
6. **Closure search steps 4 and 5** (§8.6) find libraries GNU ld does not
   (it warns `not found` and fails on the undefined reference). The link
   is correct; the program may still not find the library at run time.
   Recommendation: keep, and decide with `spec/resolve.md` whether a
   library found only by steps 4-5 deserves a note.
7. **Staged store** (§11.1). Recommendation: version one is the resident
   store; measure peak RSS against lld and mold on the bench set at M1,
   and cut the staged store and the parts reader as one work package if
   mortar's peak exceeds lld's.
8. **LLVM fat LTO objects** (`.llvm.lto` sections, `clang
   -ffat-lto-objects`): treated as ordinary objects like GCC's fat
   objects; *not verified* against the references.
9. **`copy_dt_needed` over mentions** of one soname (§8.2): "any" is
   *not verified*.
10. **Non-empty character devices, sockets, block devices** (§2.5): the
    references' behaviour is *not verified*; mortar's errors are its own.
11. **lld's `//usr/…` spelling under `--sysroot=/`** (§4.3): reported by
    the review, not reproduced with lld 22: *not established*.
