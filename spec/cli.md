# `mortar_cli`: the command line

Status: normative (WP-008); amended by WP-020d for `spec/input.md` §17
and `spec/resolve.md` §18 (`spec/landed/model/WP-020d.md`).

`mortar_cli` reads a command line in GNU ld's dialect and turns it into a
`Plan`, which is plain data. It also produces the texts that come from the
option table: `--version`, `--help` and `docs/status/options.md`.

It depends on `mortar_diag` only. It reads response files but no other
file. It never fails: every problem it finds becomes a diagnostic
(`spec/diag.md`). The facade, `crates/mortar`, applies the result (§4.3).

The behaviour here was established by experiment on this machine with:
- GNU ld 2.44: native `ld.bfd` for AArch64, and the cross linkers
  `x86_64-linux-gnu-ld.bfd` and `i686-linux-gnu-ld.bfd`;
- lld 22.1.8 (`ld.lld-22`);
- mold 2.37.1.

It also follows the public documentation of glibc's `getopt_long_only`
and libiberty's `buildargv`. Where mortar differs from GNU ld, §7.2 says
so and why.

## 1. Interface

- `parse(&[OsString]) -> Parsed` takes the arguments without the program
  name. `Parsed` holds:
  - `plan`;
  - `uses`, every option used, in argument order (§4.4);
  - `diagnostics`, the errors the grammar found, with their keys (§2.10);
  - `args`, the command line after response files are expanded (§2.6);
  - `response_files`, each response file read, with its device and
    inode (§2.6.4).
- `Parsed::failed()` is true when any diagnostic is an error.
- `Parsed::refused_uses()` and `Parsed::refusals()` cover the refused
  options (§4.4).
- `check_target(&Plan, Target)` reports the options of other targets
  (§2.9).
- `rows()` is the table (§4); `table::WRITES` and `table::READS`
  classify every row whose value is a path (§3.14).
- `version_line`, `version_text`, `print_version_text`, `help_text(prog)`
  and `status_markdown` are the texts (§5, §4.5).

## 2. Grammar

### 2.1 Arguments

- An argument is a sequence of bytes (`OsString`), not text. Values keep
  their bytes exactly. Messages show them lossily, with U+FFFD for bytes
  that are not UTF-8.
- An argument that does not start with `-`, or is `-` alone, is an input
  path. A plain `-` is standard input for GNU ld; a later phase decides
  what to do with it.
- An argument that starts with `-` is an option.
- Everything is read in one pass from left to right. The only exception
  is `-m`, which is not scanned ahead of the rest (§2.8).
- Diagnostics are reported in argument order (§2.10).

### 2.2 Names

An option is written after one or two dashes. Each spelling in the table
has a form:

| form | written as | example |
|---|---|---|
| short | `-x` (one letter) | `-o`, `-L`, `-l` |
| single | `-name` or `--name` (shown with one dash) | `-shared`, `-Bstatic`, `-pie` |
| long | `--name` or `-name` (shown with two) | `--gc-sections`, `--entry` |
| long-two | `--name` only | `--output`, `--library`, `--omagic` |
| z | `-z name` (§2.7) | `-z now` |

A long name that starts with `l`, `m` or `o` takes two dashes only. With
one dash, the argument is read as the short option `-l`, `-m` or `-o`
followed by its value:
- `-omagic` is `-o magic`;
- `-library=c` is `-l ibrary=c`;
- `-output x` is `-o utput`, and then `x` is an input.

GNU ld, lld and mold all agree here. A test checks that no one-dash
spelling in the table starts with one of these letters.

An option is looked up in this order:
1. An exact name, from any row (GNU, lld or mold), written with a dash
   count its form allows. A value may follow after `=`.
2. A name with its value glued on, for the rows whose value has no
   separator (`--lto-O2`, `--plugin-opt=...`).
3. One dash and one letter that is a short option: that short option.
4. An abbreviation of a GNU long name (§2.4).
5. One dash, then a short option with its value joined to it (`-ofile`,
   `-L/lib`, `-lc`).
6. Otherwise: `unknown-option`.

### 2.3 Values

- A required value is either joined (`--name=v`, or `-xv` for a short
  option) or the next argument, whatever that argument is.
- If the value is missing at the end of the line, the error is
  `missing-argument` with the option as it was completed (`--hash-style`).
  GNU ld sometimes says "unrecognized option" instead; mortar always uses
  `missing-argument`.
- An optional value takes only the `=` form (`--build-id[=STYLE]`).
  `--build-id sha1` makes `sha1` an input.
- An option that takes no value but is given one (`--shared=x`) is
  `unknown-option`, with a note.
- A short option that takes no value, followed by more letters (`-sx`),
  is `unknown-option` (§2.5).
- The "rest" form takes every argument that follows and ends the line.
  One row has it: mold's `-run COMMAND...`.
- The value of each row is checked when the row says how:
  - a choice of words: `--hash-style=sysv|gnu|both|none` (at M2 the row
    is `partial`: only `gnu` is carried out, `spec/dyn.md` D-20; `sysv`,
    `both` and `none` are refused values, §4.4),
    `--color-diagnostics=auto|always|never`, `--icf=all|safe|none`, and
    the others in `docs/status/options.md`;
  - a decimal count: `--error-limit`, `--thread-count`;
  - a build-ID style, or `0x` (case-sensitive) followed by one or more
    pairs of hex digits (`spec/image.md` I-70). `0x` with an odd count,
    no digits or a non-hex digit has the note `expected an even number
    of hexadecimal digits`; `0X…` is not a style (GNU ld ignores it with
    a warning, lld 22 errors `unknown --build-id style`; mold alone
    accepts it — checked by WP-027a). `Hex` keeps the whole text, `0x`
    included.

  A value that fails its check is `invalid-option-value`, with a note
  listing what is accepted. Other values (paths, symbols, addresses,
  sizes) are kept as bytes. The phase that uses them checks them.

### 2.4 Abbreviations

This follows glibc's `getopt_long_only`, as GNU ld uses it.

- When no exact name matches (§2.2, step 1), a prefix of a GNU long name
  is accepted if it is the prefix of exactly one GNU spelling that takes
  that dash count: `--gc-sec`, `-entr=main`, `-Td 0x2000` (`-Tdata`).
- A prefix of two or more spellings is `unknown-option`, with the note
  "ambiguous abbreviation: it could be ..., ..." naming every candidate.
  This holds even when the candidates are spellings of one row. Examples:
  - `-Tte` could be `-Ttext` or `-Ttext-segment`;
  - `-st` could be `-static`, `-stats`, `-start-group`, `-strip-all`,
    and others.
- An ambiguous abbreviation does not fall back to a short option: `-st`
  is not `-s -t`. When there are no candidates at all, it does fall back
  (§2.2, step 5).
- Only GNU spellings abbreviate. lld and mold accept no abbreviations, so
  an option that only lld or only mold has must be written in full.
- mortar resolves abbreviations over the GNU names of all three targets
  together. A prefix that one GNU ld configuration accepts can therefore
  be ambiguous to mortar (§7.2).
- An exact name always wins over an abbreviation. `-j` is not a short
  option, so `-j FILE` is the unique abbreviation of `--just-symbols`, as
  it is in GNU ld.

### 2.5 What mortar rejects that GNU ld accepts

- `--` is `unknown-option`. GNU ld silently drops every argument after
  it; lld and mold reject it.
- Grouped short options (`-sx`) are `unknown-option`. GNU ld accepts them
  with a deprecation warning; lld and mold reject them.
- GNU ld accepts some ambiguous abbreviations (it resolves `--a=x` and
  `--f=x` in ways it does not document). mortar rejects every ambiguous
  abbreviation (§2.4).

### 2.6 Response files

Before anything else is parsed, an argument `@file` is replaced in place
by the arguments that `file` contains. The replacement is recursive: the
file's own `@file` arguments are expanded too. Argument indexes (§2.10)
count positions in the expanded line, and `Parsed::args` is that line.

#### 2.6.1 Quoting

This is libiberty's `buildargv`, as GNU ld uses it.
- Arguments are separated by space, tab, LF, CR, VT and FF.
- Outside quotes, a backslash takes the next byte literally. A backslash
  before a newline removes both.
- Inside single quotes, every byte is literal.
- Inside double quotes, a backslash escapes only `$`, `` ` ``, `"`, `\`
  and newline (the newline is removed). Before any other byte, both bytes
  are kept.
- Quoted and unquoted parts join into one argument. `''` is an empty
  argument. An unterminated quote runs to the end of the file.
- Nothing else is special: no `#` comments, no `$VAR`, no `~`, no globs.

#### 2.6.2 Files that cannot be read

- A file that cannot be opened leaves the `@file` argument as it is, so
  it becomes an input with that name. This is GNU ld's behaviour; lld 22
  and mold report an error instead.
- `@` alone is an ordinary argument.
- An empty file expands to no arguments.
- A directory is `cannot-open-file` with the reason "is a directory". The
  argument is dropped.

#### 2.6.3 The limit

- At most 1999 response files are expanded, as in GNU ld.
- The 2000th is `response-file-limit`, and expansion stops.
- A file that names itself therefore ends with that error, and not with
  a hang.

#### 2.6.4 What is recorded

Each response file expanded is recorded in `Parsed::response_files`:
the path as the `@file` argument spelled it, and the device and inode
of the file whose bytes were read (one `open`, one `fstat` of that
descriptor, then the read; the `stat` of the directory check is made
only when the `open` fails). A file the link writes must not be one of
them (§3.14).

### 2.7 `-z` keywords

- A keyword is written `-z keyword`, `-zkeyword` or `-z keyword=value`.
  Keywords are rows of their own.
- `-z` at the end of the line is `missing-argument`.
- A keyword that is in no row is `unknown-option` with the text
  `-z keyword`. GNU ld, lld and mold warn and go on. mortar does not
  ignore a keyword it cannot vouch for (principle 1).
- A value given to a keyword that takes none (`-z now=1`) is
  `unknown-option`.
- A keyword that needs a value but has none (`-z max-page-size`) is
  `missing-argument`.
- Sizes (`max-page-size`, `common-page-size`, `stack-size`) are kept as
  text. Their values are checked by the phase that uses them.

### 2.8 Emulations

- `-m EMULATION`, `-mEMULATION`, or the abbreviation of a GNU long name
  that begins with `m`, which takes two dashes (§2.2).
- The emulations are:

  | emulation | target |
  |---|---|
  | `aarch64linux` | AArch64 |
  | `aarch64elf` | AArch64 |
  | `elf_x86_64` | x86-64 |
  | `elf_i386` | i386 |

- The last `-m` wins.
- Any other name is `refused` with class `emulation`, for example
  `elf32_x86_64` (x32) or `aarch64linuxb` (big-endian). The note lists
  the four emulations above.
- GNU ld scans the whole line for `-m` before anything else; mortar does
  not. In particular `-m` is never taken from the value of another option.
  lld agrees with mortar.

### 2.9 Options of one target

- Some GNU rows belong to some targets only:
  - `--fix-cortex-a53-843419` and `-z force-bti` (AArch64);
  - `--ld-generated-unwind-info` and `-z ibt` (x86);
  - `-z lam-u48` (x86-64).

  The table says which.
- The CET rows (`spec/x64.md` §11, X-43), honoured on x86-64 [M1]:
  `-z ibt` (force IBT in `FEATURE_1_AND`; IBT IPLT), `-z force-ibt`
  (lld's spelling, an alias of `-z ibt`), `-z shstk` (force SHSTK),
  `-z ibtplt` (IBT IPLT without the property), `-z
  cet-report=none|warning|error` (default `none`), each `spec/x64.md`
  §11. `-z ibt`, `-z shstk`, `-z ibtplt` and `-z cet-report` are GNU's
  rows of x86-64 and i386 (GNU ld takes them for both; i386 is
  `spec/x86.md`'s), so on AArch64 each is `unknown-option` as above.
  `-z force-ibt` is lld's, and only GNU rows have targets (§4.1): it is
  read on every target and refused on AArch64 by §4.4. They land in
  `plan.other` (§3.12) with their value, and stay `refused` (§4.4) until
  `mortar_x64` (x86-64) or `mortar_x86` (i386) honours them. On i386
  they will be honoured by `mortar_x86` with the same meaning
  (`spec/x86.md` X86-55, X86-49, R-11); lld 19/22 ignore `-z ibt` on
  i386 with a warning ('unknown -z value: ibt'), which mortar does not
  follow.
- They are read whatever the target. Each use is recorded in
  `plan.target_options`.
- When `-m` names the target, `check_target` runs at the end of the line.
  An option of another target is `unknown-option`, with the note
  "X is an option of T only; this link is for U".
- When the line has no `-m`, the target comes from the inputs.
  `mortar_input` calls `check_target` as `spec/input.md` §3.2 says; its
  diagnostics take phase 20 there (the key `(20, arg, 0)`), not the
  phase of this section.
- GNU ld warns about another target's `-z` keyword and goes on; it
  rejects another target's long option. mortar rejects both.

### 2.10 Keys and order

- The diagnostics of the grammar have the key `Key::new(10, arg, seq)`:
  - phase 10, the command line's ordinal (`spec/model.md` §8;
    `mortar_cli::PHASE`); response-file errors (§2.2) too;
  - `arg` is the index of the argument in the expanded line;
  - `seq` is a counter that keeps the order within one argument.
- Errors that only show at the end of the line use the index one past
  the last argument: `no-input-files` and `conflicting-options`.
- The refusals of options (§4.4) have the key
  `Key::new(10, arg, u64::MAX)`: they come after any grammar error of the
  same argument.
- The facade's own refusal of linking has phase 11 (§4.3): after the
  command line, before any later phase.
- A malformed `MORTAR_SCHED_SEED` is `mortar_core::seed`'s diagnostic,
  key `(10, 0, 0)` (`spec/model.md` §6). The grammar does not read the
  environment; the driver reports it.

## 3. The plan

### 3.1 Shape

`Plan` is plain data, cheap to clone and compare. It holds no file
contents.

- Every field that comes from an option has a default. The default is
  what the line means when it does not use the option.
- Fields that a later option overrides hold the last value written.
- Lists keep argument order. Each entry records its argument index
  (`arg`), so a later phase can point a diagnostic at it.

### 3.2 The output kind

- `-shared` (also `-Bshareable`), `-pie` (also `--pic-executable`) and
  `-no-pie`: the last one written wins. GNU ld does the same.
- `-r` (also `-i`, `--relocatable`) with `-shared` or `-pie` is
  `conflicting-options`, in whichever order they are written. GNU ld
  rejects both orders as well. With `-no-pie`, GNU ld 2.44 is not
  consistent: `-no-pie -r` links an `ET_REL`, while `-r -no-pie` links an
  `ET_EXEC` with two program headers and the warning `cannot find entry
  symbol _start` (WP-154, `tests/relocatable-design/work/options.log`,
  row `-no-pie`). lld 22 and mold 2.37.1 give `ET_REL` in both orders.
  mortar: `-r` with `-no-pie` is `Relocatable` in both orders, and
  `-no-pie` has no effect on it (C-R2) [M4-r].
- The interpreter decides between dynamic and static. An executable is
  dynamic when the line names a program interpreter
  (`-dynamic-linker PATH`) and no later `--no-dynamic-linker` cancels it.
- `-static` and `-Bstatic` do not decide the kind. They only make
  `-l` search for archives (`link_static`, §3.3). rustc's `crt-static`
  line ends with `-Bdynamic`, and is static all the same.
- The kinds are:

  | line | kind |
  |---|---|
  | `-r` | `Relocatable` |
  | `-shared` | `Shared` |
  | `-pie -dynamic-linker P` | `PieExecutable` |
  | `-pie` and no interpreter | `StaticPie` (`gcc -static-pie`) |
  | `-dynamic-linker P` | `Executable` |
  | neither | `StaticExecutable` |

- GNU ld gives a non-PIE executable its default interpreter when a
  shared object is linked in. That decision belongs to the phase that
  reads the inputs; it is not made here.
- [M4; planned: WP-117] `-shared` with `-dynamic-linker P` is `Shared`:
  the interpreter is accepted and not written (no `PT_INTERP`,
  `spec/dyn.md` D-100). `-shared -static` is `Shared` with
  `link_static` (`-static` is `-Bstatic`, as above; D-100).

### 3.3 Inputs and positional state

`plan.inputs` lists, in order:
- the paths;
- `-l name`, which is `Library`;
- `-l :file`, which is `LibraryFile`;
- `-R FILE` and `--just-symbols`, which are `JustSymbols`.

Each input carries the positional state in force where it appears:

| state | set by | cleared by |
|---|---|---|
| `as_needed` | `--as-needed` | `--no-as-needed` |
| `whole_archive` | `--whole-archive` | `--no-whole-archive` |
| `link_static` | `-Bstatic`, `-static`, `-dn`, `-non_shared` | `-Bdynamic`, `-dy`, `-call_shared` |
| `copy_dt_needed` | `--copy-dt-needed-entries` | `--no-copy-dt-needed-entries` |
| `format` | `-b FORMAT` (`binary`, `elf`/`default`, other names kept) | the next `-b` |
| `group` | `--start-group`, `-(` | `--end-group`, `-)` |
| `lib` | `--start-lib` | `--end-lib` |

Groups:
- Groups nest. An inner group is part of the outermost one, whose
  ordinal is the one recorded (counted from 1).
- A `--start-lib` inside a lib region is `unbalanced-option`.
- A group and a lib region do not nest (WP-152): `--start-group` inside a
  lib region and `--start-lib` inside a group are refused by name
  (`unsupported feature: --start-group inside a --start-lib region`,
  `unsupported feature: --start-lib inside a group`), as lld 22 refuses
  both (mold accepts both; GNU ld has no lib regions). An unclosed region
  at the end of the line is accepted (all three).

`--push-state` and `--pop-state`:
- `--push-state` saves the four switches: `as_needed`, `whole_archive`,
  `link_static` and `copy_dt_needed`.
- `--pop-state` restores them. It does not restore the group, the lib
  region or `-b`.

Unbalanced regions:
- A stray `--end-group`, `--end-lib` or `--pop-state` is
  `unbalanced-option`, naming the option it needs before it.
- A region left open at the end of the line is accepted. GNU ld and lld
  accept it too.

Search:
- `library_paths` keeps each `-L` in order; `sysroot` is the last
  `--sysroot` (`=DIR` or `DIR`), and an empty value is no sysroot, as in
  GNU ld, lld and mold (`spec/input.md` §4.3 applies it; `supported`
  since WP-027d, `crates/mortar/tests/sysroot.rs`).
- `nostdlib` is `-nostdlib`: the `SEARCH_DIR` directories of linker
  scripts are not searched (`spec/input.md` §4.2). mortar has no
  built-in directories to drop.

### 3.4 Scripts and lists

`plan.scripts` records each file with a role:

| role | options |
|---|---|
| `Script` | `-T`, `--script` |
| `DefaultScript` | `-dT`, `--default-script` |
| `VersionScript` | `--version-script` |
| `DynamicList` | `--dynamic-list` |
| `ExportDynamicSymbolList` | `--export-dynamic-symbol-list` |
| `RetainSymbolsFile` | `--retain-symbols-file` |

A line that has `-T` or `-dT` needs no input file.

### 3.5 Symbols

`plan.symbols` records each symbol option with a role:

| role | options |
|---|---|
| `Undefined` | `-u` |
| `RequireDefined` | `--require-defined` |
| `Defsym` | `--defsym` (value `sym=expr`, kept as written) |
| `Wrap` | `--wrap` |
| `ExportDynamicSymbol` | `--export-dynamic-symbol` |
| `ExcludeLibs` | `--exclude-libs` |
| `TraceSymbol` | `-y` |
| `IgnoreUnresolved` | `--ignore-unresolved-symbol` |

### 3.6 Dynamic linking

`plan.dynamic` holds:
- `soname`;
- `rpath` (each `-rpath`, in order);
- `rpath_link` (each `-rpath-link`, in order, as written: a
  `:`-separated list is split by `mortar_input`, `spec/input.md` §8.6);
- `dynamic_linker` (`Path` or `None`);
- `new_dtags`;
- `hash_style`;
- `pack_dyn_relocs`;
- `export_dynamic` (`-E`, `--export-dynamic`, `--no-export-dynamic`);
- `bsymbolic`, one of the four `-Bsymbolic` forms; [M4; planned: the
  shape WP-116] as `spec/dyn.md` D-112 resolves it: the last of
  `-Bsymbolic` (`All`), `-Bsymbolic-functions` (`Functions`) and
  `-Bno-symbolic` (`None`, also the default) wins; beside it
  `has_dynamic_list` (some `--dynamic-list`; the list then decides
  alone, whatever form comes before or after it); the forms D-112
  refuses (`-Bsymbolic-non-weak`, `-Bsymbolic-non-weak-functions`,
  `--dynamic-list-data`, `--dynamic-list-cpp-new`,
  `--dynamic-list-cpp-typeinfo`) never reach the plan (§4.2);
- `allow_shlib_undefined`: `--allow-shlib-undefined` (`true`),
  `--no-allow-shlib-undefined` (`false`), the last wins; `None` is the
  default for the output kind (`spec/input.md` §8.6, `spec/resolve.md`
  §5.2).
- `init`, `fini`: `-init SYMBOL` and `-fini SYMBOL` (also `=SYMBOL`), the
  last of each wins; `None` means `_init` and `_fini` (`spec/dyn.md` D-30
  rows 3-4; WP-042).

### 3.7 Typed `-z` keywords

`plan.z` holds the keywords that a link changes most often:
- `now`/`lazy`;
- `relro`/`norelro`;
- `execstack`/`noexecstack`;
- `text`/`notext`/`textoff`;
- `separate-code`;
- `defs`/`undefs` (also `--no-undefined`);
- `pack-relative-relocs`;
- `copyreloc`;
- `combreloc`;
- `sectionheader`;
- `memory_seal` (`-z memory-seal`, `-z nomemory-seal`; the last wins;
  `spec/synth.md` S-95's `MEMORY_SEAL`, R-C3);
- the sizes `max-page-size`, `common-page-size` and `stack-size`, as
  text (§2.7).

Each one is `None` when the line does not use it. Every other keyword
lands in `plan.other` (§3.12).

### 3.8 Output shaping

`plan.shaping` holds:
- `gc_sections`;
- `icf`;
- `build_id` (`None` means no note; `--build-id` alone means `Default`);
- `strip` (`-s`, `-S`, `--strip-*`; the last wins, `--strip-discarded`
  is accepted and has no effect, `spec/synth.md` S-84);
- `discard` (`-x`, `-X`, `--discard-none`; the last wins; with none of
  them the default of `spec/synth.md` S-78 applies, which is not `-X`);
- `compress_debug_sections`;
- `eh_frame_hdr`;
- `magic` (`-n`, `-N`, `--omagic`, ...);
- `image_base`;
- `starts`: `-Ttext`, `-Tdata`, `-Tbss`, `-Ttext-segment`,
  `-Trodata-segment`, `-Tldata-segment` and `--section-start SEC=ADDR`.
  A `--section-start` without `=` is `invalid-option-value`.
- `emit_relocs`;
- `gdb_index` (`--gdb-index`, `--no-gdb-index`; the last wins;
  `spec/synth.md` S-G1).
- `sort_common` (`--sort-common` is `Descending`, `=descending` too,
  `=ascending` is `Ascending`; the last use wins; `spec/layout.md` L-88a,
  WP-111);
- `sort_section` (`--sort-section=name|alignment`; the last use wins;
  `spec/layout.md` L-53a, WP-111).

### 3.9 Diagnostic switches

`plan.diagnostics` holds:
- `error_limit`;
- `fatal_warnings`;
- `no_warnings` (`-w`);
- `color`: `Auto` by default; `--color-diagnostics` alone is `Always`;
  `=auto|always|never`; `--no-color-diagnostics` is `Never`;
- `unresolved_symbols`;
- `warn_unresolved_symbols`;
- `allow_multiple_definition` (also `-z muldefs`);
- `noinhibit_exec`;
- `warn_backrefs`: `--warn-backrefs` until `--no-warn-backrefs`
  (`archive-backref`, `spec/resolve.md` §4.3);
  `--warn-backrefs-exclude` stays in `other`;
- `warn_common`: `--warn-common` until `--no-warn-common` (the common
  kinds, `spec/resolve.md` §3.3).
- `odr` (M6, `spec/odr.md` O-40, O-41): the mode, `--odr=warn|error|
  strict|off` (default `warn`; `--no-odr-check` is `off`);
  `check_archives` (`--odr-check-archives`); `table`
  (`--odr-table=off|exported|full`, default `off`); `stats`
  (`--odr-stats`); `keep_meta` (`--keep-mason-meta`); `runtime`
  (`--odr-runtime=off|report|fatal`, default `off`, the last one wins;
  WP-198).

The facade does two things with these:
- It applies `error_limit`, `fatal_warnings` and `no_warnings` to the
  collector's `Policy` (`spec/diag.md` §6). An absent `error_limit` is
  the default, 20.
- It returns `color` to the executable. The executable colors when the
  choice is `Always`, or `Auto` with standard error a terminal and
  `TERM` not `dumb`.

### 3.10 Bookkeeping

`plan.bookkeeping` holds:
- `map` (`-Map`) and `print_map` (`-M`): GNU's text map
  (`spec/image.md` I-120, I-121);
- `mortar_map` (`--mortar-map`): `mortar-map 2`, the verifier's map
  (`spec/image.md` I-77);
- `cref`;
- `reproduce`;
- `threads`: `--threads[=N]` (N is given) and `--thread-count N` set a
  count; `--threads` alone means `All`; `--no-threads` is a count of 1.
  Without `threads`, the phases after input use at most one worker per
  4 MiB of resident input (`spec/model.md` §6); any `--threads` form,
  `--threads` alone included, sets the count exactly;
- `mortar_map`, `time_phases` and `verify`, which are mortar's own
  options;
- `verbose` (`--verbose[=N]`: §5.1);
- `trace` (`-t`);
- `dependency_file`.

### 3.11 The plugin

`plan.plugin` holds:
- `path` (`-plugin`);
- `options`, every `-plugin-opt` / `--plugin-opt=` value in order.

mortar loads no plugin (§4.2). An LTO object is refused by name when the
phase that reads it meets it.

### 3.12 Everything else

- Every option that has no typed field lands in `plan.other` as
  `OtherOption { arg, name, value }`. `name` is the row's name.
- `plan.target_options` is §2.9.
- `plan.exit` and `plan.print_version` are §5.

### 3.13 Incremental linking

`plan.incremental` (`spec/incr.md` IN-10, mortar's own spellings):
`mode` is `None` (`--no-incremental`, the default), `Auto`
(`--incremental`), `Full` (`--incremental=full`) or `Update`
(`--incremental=update`); the last use wins. `state` is
`--incremental-state=PATH`; `report` is `--incremental-report[=FILE]`
(`Some(None)`: standard error). `--incremental-write`,
`--incremental-sync`, `--incremental-verify` and `--incremental-changed`
are parsed and refused (reserved for the update, WP-064). None of them
is part of the normalized plan (IN-18b).

#### The report (`--incremental-report[=FILE]`)

The report is text, one record per line, written to FILE or to
standard error. Its first line is IN-10's:

    incremental: full (<reason>) changed=<files> refilled=<units> bytes=<written> time=<ms>

When the link found and checked a state of its output that could be
updated (the reason is `update-not-implemented` or `small-output`:
`spec/incr.md` IN-10a), the dry run (WP-063) follows. After any other
reason the report is its first line alone. It is IN-22's change detection, IN-25..29's delta and
IN-27's refill set computed against the old state. The link then runs
in full. The new side is the full link's own layout, so the set says
which units an update laid out that way would rewrite. It is not yet
the set of a slack-preserving update. `changed=` counts the files whose
units are all refilled (IN-27 row 1); `refilled=` counts the units.

Each dry-run line starts with `delta`. The words are separated by one
space. A free-form field (a path, a name, a detail) is percent-encoded:
every byte outside `0x21..0x7e`, and `%`, becomes `%XX` with upper-case
hex digits. An empty field is `-`. The lines appear in this order:

| line | when, and what |
|---|---|
| `delta inputs clean=N hashed-same=N changed=N added=N removed=N files-changed=N appeared=N extracted=N gone=N unextracted=N` | once. The first five count FILES paths by IN-22: clean by the stamp rule, hashed and equal, hashed and different, new, gone. The rest count the link's files (archive members included): changed, newly loaded, newly extracted from an archive; old files no longer loaded or no longer extracted. |
| `delta path <state> <path>` | per path that is not clean. `<state>` is `hashed-same`, `changed`, `added` or `removed`. |
| `delta stat-missed <path>` | per path whose stamp tuple was equal but whose content differed (`stat_missed`, IN-24). |
| `delta file <state> <path> <member>` | per file whose units are all refilled (`changed`, `appeared`, `extracted`), then per old file that left (`gone`, `unextracted`). `<member>` is the archive member's name or `-`. |
| `delta probe <path> <old> <new>` | per IN-23 probe whose answer changed: `present`, `absent` or `-`. |
| `delta symbols new=N gone=N definer=N value=N size=N kind=N attrs=N comdat=N commons=N live=N dead=N tls=N` | once: IN-26's classes. |
| `delta symbol <what> <name>` | per changed global key. `<what>` is one of the classes above. The keys of the new link come first, in its `RESOLVED` order (by `SYMBOLS` row); then the `gone` keys, in the old link's `RESOLVED` order. A key with an `attrs` line and another class has the `attrs` line first. |
| `delta fallback <reason> <detail>` | per correctness fall-back (IN-16) the delta found. An update would link in full. |
| `delta policy too-much-changed <detail>` | when IN-16's policy limit would send an update to a full link. |
| `delta refill units=N holes=N chunks=N bytes=N` | once. `bytes` is the units' total size. |
| `delta unit <reasons> <path> <member> <shndx> <name>` | per refilled unit, in the new link's section order. `<shndx>` is the section's index in its file (as in `--mortar-map`). |
| `delta hole <reasons> <path> <member> <shndx> <old address> <size> <name>` | per old unit whose bytes are not where the new link puts that unit. `<shndx>` is the index in the new link's file, or `-` when the file left. The address and size are hex; the address is `-` when the old unit was not allocated. |
| `delta chunk <reasons> <tag> <chunk> <output section>` | per refilled chunk (headers, synthetic tables, merged sections, patches, a body's fill between units). |
| `delta elided <kind> listed=10000 total=<n>` | after the last line of a kind above (`path`, `stat-missed`, `file`, `probe`, `symbol`, `fallback`, `unit`, `hole`, `chunk`) of which more than 10,000 were found: the bound below. |
| `delta listed bound=10000 elided=<n>` | once: the bound, and the number of lines of every kind left out (0 when none). |
| `delta time detect=<ms> delta=<ms>` | the cost of detection, and of the delta and the report. |
| `delta end` | last. A report without it is truncated. |

`<reasons>` is a comma-separated list of these names:

| name | meaning |
|---|---|
| `file` | IN-27 row 1: the unit's file changed, appeared or was extracted. |
| `gone` | Row 2: its file left. |
| `global` | Row 3: a global it names was defined elsewhere, moved, or changed value, size or kind. |
| `comdat`, `comdat-file` | Row 4: a COMDAT keeper changed. These are the group's members and its symbols' referrers. The `-file` reason covers the old and new keepers' `.eh_frame` and non-allocated units. |
| `slots` | Rows 5 and 7: it uses a GOT, PLT, IPLT or TLS slot, and the slot tables moved or changed. |
| `moved`, `moved-ref`, `moved-file` | Row 6: the unit of an unchanged file moved. These are the unit itself, its referrers through its file's symbols, and its file's non-allocated units. |
| `frag` | A fragment of a merged section of an unchanged file moved. This covers the section's referrers and the file's non-allocated units. |
| `synthetic` | Row 7: a synthetic table (chunks). |
| `patch` | Row 8: a patch or thunk was added, moved or removed. |
| `liveness`, `liveness-file` | Row 9: `--gc-sections` liveness flipped. This covers the section, its globals' referrers, and its file's `.eh_frame` and non-allocated units. |
| `eh-frame` | IN-34a: an `.eh_frame` contribution after the first record that differs. |
| `eh-ref` | Row 10: a reference into an input `.eh_frame`. |
| `headers`, `gaps` | The file, program and section headers, and a body's fill (chunks). |

**The bound** (`spec/incr.md` IN-10b, WP-106). Of each kind of line
that names one item (`path`, `stat-missed`, `file`, `probe`, `symbol`,
`fallback`, `unit`, `hole`, `chunk`) the report writes the first 10,000
in the order above (`mortar_incr::delta::REPORT_BOUND`) and counts the
rest, which its `delta elided` line states. The count lines (`delta
inputs`, `delta symbols`, `delta refill`) are not bounded. A report is
the same bytes for the same inputs, options and state, but for `time=`
and the `delta time` line.

The fall-back reasons are IN-16's. The dry run raises
`new-output-section`, `property-changed`, `options-changed`,
`inputs-reordered`, `tls-layout-changed`, `symbol-kind-changed` and
`link-order-moved`. Their triggers are listed in
`spec/landed/incr/WP-063.md`.

A link without `--incremental-report` computes no delta.

### 3.14 Written paths (WP-087)

**The rule.** No file the link writes is a file the link read, or a
file it writes for another option. A clash is `written-file-clash`
(`spec/diag.md` §3.1), naming the written file by its option and path
as given (`-Map=a.o`) and the other by its role and path (`the input
a.o`, `the archive libb.a`, `the linker script s.ld`, `the shared object
./libc2.so`, `the response file r.rsp`, `the output out`, `the map x`):
`-Map=a.o names the input a.o; nothing is written to it`. One per
written file that clashes, key `(95, 0, n)` with `n` its place in the
registry (`spec/model.md` §8). The link fails (`spec/diag.md` §7), and
nothing is written: the check runs after the front (every file read is
known, including the ones a search or a script found) and before the
image phase creates anything. A failed link removes a stale output as
`spec/image.md` I-66 says, never a file the link read (a response file
included).

**The registry.** `mortar_cli::table::WRITES` lists every row whose
value names a file (or a directory) the link writes, honored or not,
with its role; `READS` lists the other rows whose value is a path.
`tests/table.rs` fails while a row whose value is a path (`FILE`,
`PATH`, `DIR` in its meta) is in neither list, so a new row is
classified when it is added. The facade (`crates/mortar/src/written.rs`)
builds a link's written files from the registry: the output (the last
`-o`, else `a.out`), then the last use of each other row of `WRITES`
that is not `refused` (a refused row never reaches the link, §4.3), in
the table's order, then incremental mode's state and report when the
link is in incremental mode (`spec/incr.md` IN-17; their rows alone
depend on the mode). Its unit test fails while an honored row of
`WRITES` is missing from the facade's list: an option that becomes
honored is checked without being remembered. Today: `-o`, `-Map`,
`--mortar-map`, `--dependency-file`, `--incremental-state`,
`--incremental-report=FILE`.

**The files read.** Every file the input phase read (objects, archives,
thin archives' members, linker scripts named as inputs or found through
them, shared objects): the store's identities (`Store::inodes`), known
already, so no file read costs a `stat` more. The response files
(§2.6.4). The script files the command line names (`-T`, `-dT`,
`--version-script`, ...; refused today, and read through the store when
they are not).

**Identity.** A file is identified by its device and inode, following
symbolic links (std's `MetadataExt`): the same path, `./a.o`, a symbolic
link and a hard link to it all clash. A written file that does not exist
yet has no identity; two of them are the same file when their names
are equal and either their paths are equal or their directories have
one identity (the directories are `stat`ed only then).

**The names the writer touches.** A file written through
`spec/image.md` §7's protocol also writes its temporary name
`.<base>.mortar-tmp` (I-59: reused or removed when stale) and its
set-aside name `.<base>.mortar-old` (I-113: replaced). Both count as
written: an input named `.out.mortar-old` clashes with `-o out` (`the
temporary name ./.out.mortar-old of -o out names the input
.out.mortar-old`). The `T0.<pid>` names are created with `create_new`
and can replace nothing.

**The output is no exception.** `-o` naming a file the link read, by
any spelling, is `written-file-clash` like every written file
(`spec/image.md` I-62, ruled by WP-087's review): `-o a.o names the input
a.o; nothing is written to it`. GNU ld 2.44 refuses it too (by name or
symbolic link; `ld -r -o a.o a.o b.o` exits 1 with `input file 'a.o' is
the same as output file` and leaves `a.o` unchanged); lld accepts it and
replaces the input; mold 2.37.1 aborts on an assertion after replacing
it. A failed link never removes the input as a stale output (I-66).
Before WP-087 mortar accepted it, as lld does.

**Files outside the rule.** A written path that names, following
symbolic links, a directory (I-57's `cannot-open-output`), a character
device, a FIFO or a socket is outside the rule: writing a device or a
stream destroys no file, and `-o /dev/null -Map=/dev/null
--dependency-file=/dev/null` is an idiom (GNU ld, lld and mold accept
it). Such a file is written in place (`spec/image.md` I-58, I-79a), so
its temporary names are not touched either. `-Map=/dev/stdout` names a
terminal or a pipe (outside the rule) or, redirected, a regular file:
then the rule applies to that file (`> a.o` with `a.o` an input
clashes; the shell truncated it before mortar ran).

**A guard against mistakes, not against races.** The check reads the
file system once, before anything is written. A file renamed, replaced
or created between the check and the write (by another process) is not
seen; mortar does not lock its inputs. What it rules out is a command
line that names one file twice.

**What a plain link pays.** One `stat` per written file and two for its
temporary names (`-o` alone: 3), two more when two written names not yet
there have one name in different directories' spellings, one `stat` per
script the command line names; nothing per input. A file written whole
that is a regular file costs one `stat` more when it is written (the
stream test of `spec/image.md` I-79a). Measured on protoc
(`spec/landed/cli/WP-087.md`).

**The references** (`spec/landed/cli/WP-087.md` has the table): GNU ld
2.44 refuses `-o` naming an input by its path or a symbolic link (not a
hard link; `input file 'a.o' is the same as output file`), and checks
nothing else: `-Map` and `--dependency-file` overwrite any input, and a
map over an archive is written before the archive is read (the link
then fails, the archive destroyed). lld 19 and 22 check nothing:
`-Map=a.o` exits 0 with the object replaced by the map. mold 2.37.1
checks nothing and writes in place: `-o a.o` with `a.o` an input fails
with the input destroyed, `-Map=a.o` dies of `SIGBUS` with the input
replaced by the map.

### 3.15 The standard streams (WP-095)

**What goes where.** Standard error: the report (`spec/diag.md` §5,
§7), `--print-gc-sections`' lines (`spec/image.md` I-10),
`--time-phases`' lines (I-88), incremental mode's lines
(`--incremental-report` without a file, a fall-back's line;
`spec/incr.md` IN-10) and `MORTAR_STEPS`' lines (`mortar_core::
wp032b_steps`). Standard output: the answers of §5 (`--version`, `-v`,
`-V`, `--help`) and `-M`'s map (I-120). Nothing else.

**The rule.** A build system reads a linker's verdict from its exit
status and its product from the files it named; it does not promise to
read the linker's messages to the end (`ld ... 2>&1 | head`, `ld --help
| grep -q`, a log on a full disk).

- **S-1 (messages).** Every byte on standard error, and the answers of
  §5 on standard output, are messages. A message that cannot be written
  (the reader of a pipe has gone, `EPIPE`; a full device, `ENOSPC`; any
  other error) is dropped, and so is the rest of that write: mortar does
  not panic, raises no signal, dumps no core, and says nothing about it
  (there is nowhere to say it). The link goes on to its own verdict, and
  the exit status is the link's: 0 for a link that succeeded whose
  warnings nobody read, 1 for a failed link whose errors nobody read, 0
  for `--version` and `--help` whose answer nobody read.
- **S-2 (outputs).** A file the link was asked to write is an output
  wherever it points: `-o`, `-Map`, `--mortar-map`, `--dependency-file`
  naming a pipe, a terminal, `/dev/stdout` or a full device (written in
  place, `spec/image.md` I-58, I-79a), and `-M`'s map, written to
  standard output. An output that cannot be written is
  `cannot-write-output` (`spec/diag.md` §3.1) naming it (`/dev/stdout`
  as given; `standard output (-M)` for `-M`): the link fails, exit 1,
  and leaves no output file (`spec/diag.md` §7, `spec/image.md` I-66).
  `-M`'s map is written after every other output that can fail the
  link and before the incremental state (`spec/incr.md` IN-17), so a
  map that cannot be written leaves no state either. Part of the map may
  have reached the stream before the failure.
- **S-3 (nothing half-written).** Since no write to a stream can end
  the process, the output protocol (`spec/image.md` §7: a temporary
  name, then a rename or a removal) and the state's (IN-17, IN-46) run
  to their end on every path: no `.<out>.mortar-tmp` is left by a
  message nobody read. Before WP-095 a `MORTAR_STEPS` line to a closed
  pipe or a full device aborted the process (`SIGABRT`) wherever it was
  printed, the image phase's temporary file included.
- **S-4 (closed descriptors).** A descriptor 0, 1 or 2 closed when
  mortar starts (`2>&-`) is opened on `/dev/null` by Rust's start-up
  code before `main`: what mortar writes there vanishes (S-1, a success
  for `-M` and `-Map=/dev/stdout`), and no file mortar opens can take
  descriptor 1 or 2 (GNU ld 2.44 under `2>&-` writes its warnings into
  the output file it opened there).
- **S-5 (one writer).** Every write to standard error or standard output
  by a shipped crate goes through `mortar_diag::stream` (`to_stderr`
  drops a failure, `to_stdout` returns it for S-1 or S-2). It lives in
  `mortar_diag` because every crate that prints depends on it already,
  the verifier included, and it depends on nothing.
  `crates/mortar_diag/tests/streams.rs` fails on any `print!`,
  `println!`, `eprint!`, `eprintln!`, `dbg!`, `stdout()`/`stderr()` call
  or `io::Stdout`/`io::Stderr` type in `crates/*/src` elsewhere: those
  panic on a failed write, and the release profile aborts on a panic.
  mortar ignores `SIGPIPE`, as every Rust program does at start-up: it is
  never killed by a pipe, it sees `EPIPE`.

**Why S-2 covers `-M` and S-1 covers `--help`.** `-M` is `-Map` to
standard output, and `-Map=/dev/stdout` that cannot be written has been
an error since WP-087: the two spellings of one request agree, and a map
asked for and lost on a full disk is not a success (lld 22 makes both an
error; GNU ld 2.44 and mold 2.37.1 exit 0 with the map lost). The answers
of §5 are read by scripts that stop at the first match (libtool's
`$LD --help | grep -q ...`); under `set -o pipefail` a status other than
0 would fail them, and the references with `SIGPIPE` ignored all exit 0
(lld 19 exits 74).

**The references** (`spec/landed/cli/WP-095.md` has the tables): with
`SIGPIPE` at its default, a pipe whose reader has gone kills GNU ld
2.44, lld 22 and mold 2.37.1 at their first write to it (GNU ld leaving
an empty output file behind); lld 19 exits 74. With `SIGPIPE` ignored,
and on `/dev/full`, all three keep the link's status for their
messages; for the map on standard output lld fails the link (exit 1),
GNU ld and mold exit 0.

### 3.16 Relocatable output (`-r`) [M4-r; planned: WP-154's cut, record `spec/landed/layout/WP-154.md` §F]

Until the join of the first `-r` package, `-r` stays `refused`
(`unsupported option: -r`, as today). The rules below are those the
packages implement. "At `-r`" means `Plan.kind == Relocatable`. The
evidence for each row is in `tests/relocatable-design/work/options.log`
and `census-lines.txt` (record §A, §B).

**C-R1 (the kind).** `-r`, `-i` and `--relocatable` set
`kind = Relocatable`. `-Ur` (GNU ld: "-r, and resolve C++ constructors")
is also `Relocatable`, with `plan.ur = true` recorded for the map, and
it conflicts as `-r` does (its spelling is the conflict's `first`). GNU
ld 2.44 writes the same sections for `-Ur` as for `-r` (record B.1). lld
22 and mold reject `-Ur` as unknown, and lld and mold also reject `-i`.
mortar accepts all four spellings, because a line that GNU ld links must
link. `-r` with `-shared`, `-pie` (`--pic-executable`) or `-static-pie`
is `conflicting-options` in either order (GNU ld and lld refuse both;
mold accepts `-pie` and `-shared` and writes an `ET_REL` anyway).
*Test:* `r-kind-spellings`, `r-conflicting-pie-shared`.

**C-R2 (options that do nothing at `-r`).** These options are accepted
at `-r` and change no byte of the output. In each case the three
references were measured to leave the output unchanged, or the option
acts only on an executable or shared output. `-no-pie`; `-static`,
`-Bstatic`, `-Bdynamic` (they still pick `.a` or `.so` for `-l`, and a
`.so` is refused, C-R5); `-dynamic-linker`, `--no-dynamic-linker`;
`--eh-frame-hdr`; `--hash-style`; `--as-needed`, `--no-as-needed`;
`--no-build-id`, `--build-id=none`; `-X`,
`--discard-locals` (no visible change in any reference, B.3); `-EL`;
`-m <emulation>` (checked as today: a wrong emulation is still refused
by `emulation`); `--fix-cortex-a53-843419` (patches need addresses);
`--emit-relocs`, `-q`; the `-z` keywords that C-R2a marks no-op;
`--no-undefined`, `-z defs` (nothing is undefined at `-r`, as all
three references do); `--warn-common`; `--sort-common`,
`--sort-section` (a final link's order; GNU ld does not change the
`-r` output); `-O<n>`; `-Map`, `--mortar-map` (they are written as in
any link); `--threads`; `--no-relax`; `-plugin`, `-plugin-opt` (GCC's
`-r` line carries them; an LTO object is still refused by name,
RV-R9); `--push-state`, `--pop-state`; `--start-group`, `--end-group`;
`--whole-archive`, `--no-whole-archive` (they act on extraction,
RV-R2); `-L`, `-l`; `--version-script`, `--exclude-libs` (no reference
changes the `-r` output: record B.3).
Status: each row keeps its final-link status, and `docs/status/
options.md` gains a column "at `-r`" with `noop(-r: …)`.
*Test:* `r-noop-options`: each row against the line without it, byte
identical.

**C-R2b (every row has a class at `-r`; ruled on WP-180's Q-O1 to
Q-O7).** The list above is the drivers' and the measured rows; it is
not the whole table. Every row of §4.1 that a final link accepts or
refuses has an `at_r` class (acts by a rule, no-op, as in a final link,
`conflicting-options`, refused by name), kept on the row
(`crates/mortar_cli/src/rows.rs`) and shown in the "at `-r`" column of
`docs/status/options.md` (generated since RL-1, WP-181, which deleted
the hand-written `docs/status/options-at-r.md` it replaced). A row without a class is a defect
of the table (*Test:* `every_row_has_a_class_at_r`, RL-1). The classes
of the rows C-R2, C-R2a, C-R3 and C-R4 do not name:
- **No-op, measured**: every row that GNU ld 2.44 or lld 22 leaves the
  `-r` output byte-identical for
  (`tests/relocatable-design/exp/rl0/opts.sh`, `work/rl0-opts.log`; the
  rows were `options-at-r.md`'s "noop: measured, unlisted"). They join
  `r-noop-options`. How many rows each class has is what the generated
  column of `docs/status/options.md` counts; this text gives no number
  (WP-182, ruling (a) of WP-181's record §B).
- **No-op, by what the option is**: `--call-graph-profile-sort`, the
  `--bp-*` family, `--lto*`, `--ctf-share-types`,
  `-z execute-only-report`, `--unresolved-symbols` (nothing is
  undefined at `-r`, like `--no-undefined`), `--orphan-handling` (there
  is no script while `-T` is refused; RL-11 measures it with `-T`),
  `--keep-mason-meta` (MT-30 always writes the container) and
  `--odr-table` (the table belongs to a loadable output; the units are
  carried by MT-30 and the final link writes it: a line that carries
  the option for every link must not fail at its `-r` step, as
  `--eh-frame-hdr` does not), and `--odr-runtime` for the same reason
  (the runtime belongs to the final link: nothing is added to a `-r`
  output and nothing is checked on the line, WP-198).
- **`--start-lib`, `--end-lib`**: lazy objects, as in a final link
  (RV-R2; lld's reading, GNU ld does not know the option).
- **`--require-defined=SYM`**: a root for extraction like `-u`
  (RV-R2), nothing written for it (RV-R6 is `-u`'s alone). With `SYM`
  undefined at the end of a `-r` it is `required-symbol`, as in a final
  link: GNU ld 2.44 refuses the `-r` the same way (`required symbol
  `SYM' not defined`, exit 1); lld 22 does not know the option and
  mold 2.37.1 accepts it and writes nothing (WP-182, record
  `spec/landed/resolve/WP-182.md` §B).
- **Dynamic-output options C-R4 does not name**: the "no" forms
  (`--no-export-dynamic`, `-Bno-symbolic`, `--disable-new-dtags`) are
  no-ops; `-init`, `-fini`, `--pack-dyn-relocs`,
  `--android-memtag-mode` are `conflicting-options` with `-r` as
  `first`, like C-R4's.
- **`--incremental-state`, `--incremental-report`** (every
  `--incremental-*` row): IN-12's warning and no effect, as
  `--incremental`; `--no-incremental` is a no-op.
- **A row that C-R2 or C-R2a's first row lists and a final link
  refuses** (the `-z` keywords `combreloc` to `noreloc-overflow`;
  `--emit-relocs`, `-q`; `--no-relax`): a no-op at `-r`. The class at
  `-r` wins over the final-link status: no byte of an `ET_REL` depends
  on any of them (measured, C-R2a), and the drivers' lines carry
  several.
- **Rows that are about the run, not the output**: `--version`, `-v`,
  `-V`, `--help`, `--time-phases`, `--rsp-quoting`: as in a final link.
  The ODR switches `--odr`, `--no-odr-check`, `--odr-stats`,
  `--odr-check-archives`: as in a final link, by `spec/meta.md` MT-30a
  (the checks run over the units the `-r` reads).
  `--detect-odr-violations` (gold's spelling, a no-op in a final link):
  a no-op.
- **An option that acts at `-r` "until" a package lands** (`-d`,
  `-dc`, `-dp`, `-s`, `-S`, `-x`, `-b binary`,
  `--force-group-allocation`, `--defsym`, `--unique`, `-T`, and
  `-z unique-symbol`): until then `Refused{Option, "<option> with -r"}`
  (`-z unique-symbol with -r`), the form of C-R4. `<option>` is the
  option as the parse completes its spelling (`Use::spelling`: `-s`,
  `--strip-all`, `--script` for `--script=x`); a `-z` keyword with its
  value (`-z stack-size=4096`, `-z gcs=always`) and `--format` with its
  value (`--format=binary`) (WP-182). `-r` and `-Ur` are the exception:
  until the join they keep their final-link refusal,
  `unsupported option: -r`.

**C-R2a (`-z` keywords at `-r`, by measurement).** Every `-z` keyword
that GNU ld 2.44 lists in `--help` for each target was run at `-r`
over two GCC objects, and its output compared with the plain `-r`
(`tests/relocatable-design/exp/review/zkw.sh`, `work/zkw.log`, 259
cells over three targets). Each keyword falls in one class:

| class | keywords | at `-r` in mortar |
|---|---|---|
| byte-identical on every target where GNU ld knows them | `combreloc`, `nocombreloc`, `common`, `nocommon`, `common-page-size=`, `max-page-size=`, `defs`, `undefs`, `muldefs`, `global`, `globalaudit`, `initfirst`, `interpose`, `lazy`, `now`, `loadfltr`, `nocopyreloc`, `nodefaultlib`, `nodelete`, `nodlopen`, `nodump`, `origin`, `relro`, `norelro`, `separate-code`, `noseparate-code`, `text`, `notext`, `textoff`, `start-stop-gc`, `nostart-stop-gc`, `start-stop-visibility=`, `sectionheader`, `unique`, `nounique`, `nounique-symbol`, `nomemory-seal`; AArch64 `bti-report[=…]`, `gcs-report[=…]`, `gcs-report-dynamic=`, `gcs=never`, `pac-plt`; x86 `call-nop=`, `ibtplt`, `isa-level-report=`, `dynamic-undefined-weak`, `nodynamic-undefined-weak`, `noextern-protected-data`, `noindirect-extern-access`, `pack-relative-relocs`, `nopack-relative-relocs`, `report-relative-reloc`, `mark-plt`, `nomark-plt`, `noreloc-overflow`, `lam-u48`/`lam-u57` and their `-report` forms on i386 (GNU ld warns `ignored`) | no-op (C-R2) |
| the stack note | `execstack`, `noexecstack` | act: `spec/layout.md` L-R6a's table. On x86-64 and i386 `execstack` changes nothing when an input has a note, so the plain comparison showed it identical there |
| add a `.note.gnu.property` (and its section symbol) | AArch64 `force-bti`, `gcs=always`, `stack-size=`; `memory-seal` (all three targets); x86 `ibt`, `shstk`, `indirect-extern-access`, `x86-64-v2/v3/v4` (`x86-64-baseline` makes GNU ld 2.44 abort with an internal error in `_bfd_x86_elf_merge_gnu_properties`); x86-64 `lam-u48`, `lam-u57`; `stack-size=` adds nothing on x86 | refused by name, `Refused{Option, "-z KEYWORD with -r"}`, until a line needs one. Then GNU ld's form: the property merged into L-R1's note |
| renames locals | `unique-symbol`: GNU ld appends `.N` to every duplicated local name, mapping symbols included (`$x` becomes `$x.0`, a static `h` becomes `h.0` and `h.1`); lld and mold do not know it | refused by name until a line needs it |
| fail on the inputs at `-r` | `cet-report=error`, `lam-report=error`, `lam-u48-report=error`, `lam-u57-report=error` (x86-64: "missing … property") | the same check as in a final link (the report keywords' rows of `spec/x64.md`/`a64.md`); `=none`/`=warning` are no-ops |
| refused by GNU ld at `-r` | `nosectionheader` (`-r and -z nosectionheader may not be used together`) | `conflicting-options` |

A keyword this table does not list keeps its final-link status at
`-r` (unknown, or refused by its name); one the first row lists is a
no-op at `-r` even where a final link refuses it (C-R2b). This table
governs the report keywords (`-z bti-report`, `gcs-report`,
`gcs-report-dynamic`, `isa-level-report`, `start-stop-visibility`):
whatever their final-link status, they are no-ops at `-r` (WP-182,
ruling (a) of WP-181's record §B). `-z gcs` is a no-op with `=never`,
refused by name with `=always`, and keeps its final-link status with
`=implicit`.
*Test:* `r-z-noop` (the first row, byte-identical), `r-z-refused` (one
per refused keyword), `r-stack-note`.

**C-R3 (options that act at `-r`).**

| option | at `-r` | rule | package |
|---|---|---|---|
| `-o` | the output | I-R1 | RL-4 |
| `-u SYM` | an undefined global `SYM` is written to `.symtab` when nothing defines it, and it is a root for archive extraction (GNU ld; lld and mold extract but write nothing) | RV-R2, RV-R6 | RL-2 |
| `-e SYM` | accepted. `e_entry` is 0 (mold); GNU ld and lld write `SYM`'s section offset, which means nothing in an `ET_REL` (L-R7) | L-R7 | RL-3 |
| `-d`, `-dc`, `-dp` | commons are allocated: each becomes a definition in `.bss` (in `COMMON` order, L-R4), as GNU ld does; lld accepts `-d` and does nothing, rejects `-dc`/`-dp`; mold rejects `-d` | RV-R5 | RL-11 |
| `-s`, `--strip-all` | GNU ld and mold: debug sections and their relocation sections are dropped, and so is every local symbol that no relocation names. lld ignores the option. mortar does what GNU ld does | I-R9 | RL-11 |
| `-S`, `--strip-debug` | debug sections and their relocation sections dropped (GNU ld, mold; lld ignores) | I-R9 | RL-11 |
| `-x`, `--discard-all` | local symbols that no relocation names are dropped, except `STT_SECTION` and `STT_FILE` (all three) | I-R9 | RL-11 |
| `-T script` | GNU ld and lld honour a `SECTIONS` script at `-r` (record B.1: `.text.*` into `.text`). mold refuses it. The main user is Kbuild's module line (`-T module.lds`). Kernel modules are out of scope for now (`spec/layout.md` L-R1), and the refusal is lifted when a real line of the target needs it | refused by name (`Refused{Option, "-T with -r"}`) until RL-11 | RL-11 |
| `-b binary`, `--format=binary`, `-b default` | the input file becomes a data section (L-R9) | L-R9 | RL-10 |
| `--defsym SYM=EXPR` | GNU ld and lld define `SYM` in the output: absolute, or relative to a section. mold does nothing. | refused until RL-11, then GNU ld's | RL-11 |
| `--force-group-allocation` | every group is dissolved: its members become ordinary sections merged by name, and no `SHT_GROUP` is written (GNU ld and lld). This is the sound form of the pre-link idiom (record §A.4): `--force-group-allocation` before `objcopy --localize-hidden` | L-R3 | RL-8 |
| `--build-id[=STYLE]` | the note is written over the `-r` output (I-69's styles; GNU ld and lld write one, mold none) | I-R8 | RL-4 |
| `-z execstack`, `-z noexecstack` | GNU ld's stack-note table, per target | L-R6a | RL-7 |
| `--unique`, `--unique=PATTERN` | one output section per selected input section (GNU ld's form; lld has only the bare form, mold misparses it) | L-R11 | RL-11 |

**C-R4 (options refused at `-r`, by name).** Each is
`Refused{Option, "<option> with -r"}` with a note naming the reason:
- `--gc-sections`. GNU ld refuses it without a root. With `-e`/`-u`
  roots it fails on `.debug_info` (`relocation references symbol
  ucommon which was removed by garbage collection`). lld collects; mold
  barely changes anything (record B.3). A partial link cannot know what
  the final link will need. image.md I-11 becomes permanent (I-R10).
- `--icf`. GNU ld does not know it, lld refuses it at `-r`, mold accepts
  it.
- `--unique[=pat]` until RL-11, which accepts it (C-R3, L-R11). The
  first version refused it for good, on two wrong readings:
  - "GNU ld's output does not change". It does not change on inputs
    whose section names are already distinct. On duplicate names GNU
    ld splits them (the review's x06 case and `work/unique-buildid.log`).
  - "mold drops definitions". That was mold's option parse taking the
    next input as its pattern.
- `--relax` (GNU ld refuses it at `-r`).
- `--wrap=SYM`. lld adds an undefined `__wrap_SYM`, GNU ld and mold do
  nothing. The final link is the place for it.
- `--compress-debug-sections=zlib|zstd` (GNU ld and lld compress; mold
  ignores). Accepted later if a line needs it; `=none` is a no-op.
- `--gdb-index` (lld refuses it at `-r`; mold accepts it and writes
  nothing). synth.md S-G8 already says that a relocatable output has no
  index.
- `-Bsymbolic*`, `--dynamic-list*`, `--export-dynamic*`, `-soname`,
  `--enable-new-dtags` (dynamic-output options): with `-r` these are
  `conflicting-options` with `-r` as `first`.
  `-rpath` (`--rpath`) and `-rpath-link` are not of this family: they
  are no-ops at `-r` (class `Noop`). Measured (WP-182, record §B): GNU
  ld 2.44, lld 22 and mold 2.37.1 accept `-rpath DIR`, `--rpath=DIR`,
  `-rpath-link DIR` and `-rpath-link=DIR` at `-r`, and each writes the
  same bytes as without the option (no `.dynamic`, no `DT_RPATH`). A
  build that passes its LDFLAGS to `ld -r` is not refused.
- `--incremental` with `-r`: IN-12's warning and a plain link (as
  today).
- the `-z` keywords that C-R2a refuses.
- A kernel module line is refused by name, through L-37
  (`.gnu.linkonce.this_module`) and `-T`: kernel modules are out of
  scope for now (`spec/layout.md` L-R1).
*Test:* `r-refused-options` (one link per row; the name checked).

**C-R5 (inputs at `-r`).**
- A shared object, given directly or by `-l`, is refused:
  `Refused{Feature, "shared object input with -r"}`. GNU ld and lld
  refuse it (`attempted static link of dynamic object`); mold drops it
  silently.
- `/dev/null` or an empty input list with `-m` is an empty `ET_REL` of
  the emulation's machine, which is what GNU ld writes. Without `-m` and
  with no object to take the machine from, it is `Refused{Feature,
  "relocatable output with no input of a known machine"}` (lld: `target
  emulation unknown`; mold: `-m option is missing`).
- An archive is read by RV-R2's extraction rule.
*Test:* `r-shared-input-refused`, `r-dev-null-m`, `r-dev-null-no-m`.

**C-R6 (the drivers' lines, record §A).** Every `gcc -r`, `g++ -r`,
`gfortran -r` and `clang -r` line of the census (three targets,
`-nostdlib` or not) consists of rows of C-R2 and C-R3 only. GCC
writes `-plugin … --eh-frame-hdr --hash-style=gnu --as-needed
-dynamic-linker P [-X -EL -maarch64linux --fix-cortex-a53-843419 | -m
elf_x86_64 | -m elf_i386] -o OUT -r OBJ…`; clang writes `[-EL]
--hash-style=gnu --build-id --eh-frame-hdr -m EMUL -o OUT -r OBJ…`.
Neither adds crt files or libraries. Without `-nostdlib`, a driver line
with `-Wl,-r` (or `-no-pie -Wl,-r`) does add `crt1.o crti.o crtbegin.o …
crtend.o crtn.o` and the default libraries, with `-pie` when PIE is the
default. That line is `conflicting-options` (`-r` and `-pie`), as GNU ld
says. With `-no-pie` it is a `-r` of the startup files, which has a
`crtend.o` terminator in the middle of the output's `.eh_frame`. mortar
links it as written, as GNU ld does, and warns (record §G Q7, ruled;
RL-11): when an input's `.eh_frame` zero terminator would stand before
the last record of the output's `.eh_frame`, the warning
`eh-frame-terminator` names that input and the number of records after
it ("unwinders stop at the terminator; is this a `-r` of the startup
files without `-nostdlib`?"). The terminator's place is the only
trigger: an input that defines `_start` is an ordinary `-r` input.
*Test:* `r-driver-lines` (the census lines replayed, the plan compared).

## 4. The table and the status of options

### 4.1 Rows

There is one row per option, in `crates/mortar_cli/src/rows.rs`. The
table has 479 rows (WP-072 added the seven of M6 below). A row has:

- **spellings**: each with a form (§2.2) and a flag saying whether GNU ld
  has that spelling. The flag decides which spellings abbreviate (§2.4).
  The first spelling is the row's name.
- **arg**: none, required, optional (`=` only), glued, or rest. Values
  are of these kinds: any, a choice of words, a count, or a build-ID.
- **meta**: the value's name in `--help` (`FILE`).
- **linkers**: which reference linkers accept the row, found by running
  each linker:
  - GNU ld for every target;
  - `ld.lld-22`, where a row is accepted unless the answer is "unknown
    argument";
  - `mold`.
- **targets**: empty means every target (§2.9). Only GNU rows have
  targets.
- **group**: the `--help` section.
- **status**: §4.2.
- **lands**: the `Plan` field the row sets.
- **help**: one line.

Each spelling names exactly one row; `tests/table.rs` checks this.

The table includes:
- every option in GNU ld 2.44's `--help` for the three targets;
- the hidden GNU spellings that count for abbreviations (`-dc`, `-dp`,
  `-Qy`, `-qmagic`, `--add-needed`, ...);
- every option in `ld.lld-22 --help`;
- every option in `mold --help`;
- the `-z` keywords of all three linkers;
- three options of mortar's own, `--mortar-map`, `--time-phases` and
  `--verify`.

| origin | rows |
|---|---|
| GNU ld (with or without lld and mold) | 292 |
| lld only | 117 |
| mold only | 21 |
| lld and mold, not GNU ld | 39 |
| mortar's own | 3 |

### 4.2 Status

Every row has one status. A status never says more than the pipeline
does: a user may rely on every use the status admits.
- `supported`: mortar does what the option asks, for every value.
- `partial(values, note)`: supported, except for the values listed; a use
  with one of them is refused at the command line (§4.4) with the note,
  which names what is supported. A row whose status would depend on its
  value is `partial`, never `supported`.
- `noop(reason)`: the option needs nothing done. The reason says why
  (GNU ld ignores it too; a memory hint for BFD; every target is
  little-endian; mortar loads no plugin; the option acts only on inputs
  or outputs that are refused by name).
- `refused`: read and recorded, but not carried out yet. This is the
  default.

The status today (WP-042 and WP-034; `docs/status/options.md` has the
list, and counts 54 supported, 4 partial, 44 noop and 370 refused).
WP-034 made `--no-dependent-libraries` `noop`: mortar never loads the
libraries `.deplibs` sections name (`spec/resolve.md` §10);
`--dependent-libraries` stays `refused`. The counts in
this list are those of WP-027e; WP-031, WP-027d's later rows and WP-042
(the M2 paragraph below) changed them since:
- 49 rows are `supported`; each has its evidence in
  `crates/mortar/tests/options.rs`, an observable effect checked against
  GNU ld's output where GNU ld has the option, or the test that checks
  it. WP-027e added `--push-state`, `--pop-state`, `-z now`, `-z lazy`
  (`spec/layout.md` L-60 row 7), `--dependency-file`
  (`spec/input.md` §10.4) and `--mortar-map`.
- 3 rows are `partial`: `--build-id` (`=md5`, `=uuid`; `spec/synth.md`
  S-91), `--fix-cortex-a53-843419` (`=adr`, `=adrp`; `spec/a64.md`
  §11.2) and `--compress-debug-sections` (only `=none`, WP-027e).
- 44 rows are `noop`; the audit links each to the same bytes as the line
  without it and checks its reason where it can be checked.
  `--as-needed`, `--no-as-needed` (only shared objects, refused by name)
  and `--hash-style` (only dynamic outputs, refused by name) were
  `supported` before WP-027d. WP-027e added the rows that act only on a
  dynamic output or a shared-object input: `-rpath`, `-rpath-link`,
  `--enable-new-dtags`, `--disable-new-dtags`, the five `-Bsymbolic`
  rows, `-E`, `--no-export-dynamic`, `--no-dynamic-linker`, `-z text`,
  `-z notext`, `--no-undefined`, `-z defs`; and `--no-gdb-index` (the
  default).
- 376 rows are `refused`, `--gc-sections` among them (`spec/image.md`
  I-5): it was `supported` while every link that used it was refused by
  liveness. `--warn-common` stays refused (WP-027e: it needs a resolve
  warning with GNU's three texts). WP-111 accepts it: GNU's five texts
  map to mortar's two named diagnostics (`spec/resolve.md` §3.3).

WP-097 (`spec/synth.md` §17) changed three rows of the output group:
- `--gdb-index` is `supported`: it writes `.gdb_index` (S-G1), or the
  link is refused by one of S-G's names (§17.6). With `--incremental`
  it stays refused (`spec/incr.md` IN-58), by the facade.
- `--no-gdb-index` stays `noop`, with the reason "the default: no
  .gdb_index is written; the last of --gdb-index and --no-gdb-index
  wins" (the two share `shaping.gdb_index`, last wins).
- `--no-debug-names` is `noop` ("the default: no merged .debug_names is
  written"); it was `refused`, which refused a request for the
  default.
- `--debug-names` stays `refused`, and its refusal carries a note:
  "gdb 16 ignores a .debug_names it did not write; use --gdb-index
  (spec/synth.md S-G9)". A refused row's reason is in
  `rows::REFUSED_NOTES` by row name (`Row::refused_note`); the refusal
  of §4.4 adds it as a note and the generated table prints it after
  `refused:`.

A work package that implements an option changes the row's status in the
same change, and adds the row's evidence to the audit, which fails while
a row that is not `refused` has none.

Rows of mortar's own for M6 (`spec/odr.md` O-41; amendment A-5). They
are added to `crates/mortar_cli/src/rows.rs` by WP-072 (WP-076 for the
opt-ins), `refused` until the package that honors each lands with its
test, and `docs/status/options.md` is regenerated from the table, never
edited:
- `--odr=warn|error|strict|off` (default `warn`) and `--no-odr-check`
  (= `--odr=off`): WP-072;
- `--odr-stats`: WP-072;
- `--detect-odr-violations` (GNU gold's spelling; not a reference
  linker's row): `noop`, printing nothing; its reason is the sentence
  of `spec/odr.md` O-41; WP-072;
- `--odr-check-archives`, `--odr-table=off|exported|full` (default
  `off`), `--keep-mason-meta`: WP-075 and WP-076.
- `--odr-runtime=off|report|fatal` (default `off`): WP-198.

WP-072 landed the first four: `--odr`, `--no-odr-check` and `--odr-stats`
`supported`, `--detect-odr-violations` `noop` (its reason is O-41's sentence;
nothing is printed, since `spec/diag.md` §2.1 has no note severity); the other three are
`refused` with their fields in `plan.diagnostics.odr` (§3.9).
WP-075 made `--odr-table` `partial`: `=off` and `=exported` are
accepted (the table is written in dynamic executables, `spec/odr.md`
O-50; a static executable exports nothing and gets none), `=full` is
refused by name until WP-076.
WP-076 made `--odr-check-archives`, `--odr-table` (all three values)
and `--keep-mason-meta` `supported` (`spec/odr.md` O-23, O-50;
`spec/meta.md` MT-32). The first two act in `warn`, `error` and
`strict` and do nothing under `--odr=off`; `--keep-mason-meta` acts in
every mode, `--odr=off` included.
WP-198 added `--odr-runtime` `supported` (C-ODR-RT below).
`docs/status/options.md` has the counts.

**C-ODR-RT (`--odr-runtime=off|report|fatal`; WP-198).** The load-time
one-definition-rule check (`spec/odr.md` O-53a, §10.3). With `report` or
`fatal`, in a dynamic executable (`Executable`, `PieExecutable`) or a
shared object:
- the runtime object of the output's machine and kind (the `exe`
  flavour for an executable, `so` for a shared object; embedded in
  mortar, `runtime/odr/obj/`) is an ordinary input after every input of
  the line, named `<mortar:odr-runtime>` in `-Map`, the diagnostics and
  the tables (it is no file: the dependency file does not list it);
- `--odr-table=exported` is implied (a table named on the line stands:
  `=full` is kept); an explicit `--odr-table=off`, the last one of the
  line, is `conflicting-options` (the runtime reads the table);
- the output is linked `-z now` (`DF_BIND_NOW`, `DF_1_NOW`) unless the
  line says `-z lazy`, which is accepted (the runtime then says what it
  cannot check, `odr-hook-lazy`). What `-z now` changes: every symbol
  is bound when the module is loaded, so a reference to an undefined
  function that is never called, which a lazy binding never resolves,
  now fails the load (the loader's `symbol lookup error`), and the load
  takes the time of every binding;
- `.note.mason` carries, after the table's note(s), the runtime's note
  (`spec/odr.md` §10.2: type `0x4d4f4403`, mode 1 `report` or 2
  `fatal`).

Refused by name: a static executable or static PIE
(`odr-runtime-static`: there is no loader and no binding to check), a
machine the runtime is not linked for (`odr-runtime-unsupported-target`:
i386, whose objects are embedded but not validated, `spec/dyn.md`
D-130), and `--incremental` (`odr-runtime-incremental`: the state has
no row for an input that is no file). At `-r` the option is a no-op
(C-R2b). `-nostdlib` is no refusal: the runtime's references to libc
(`write`, `mmap`, `getenv`, `dl_iterate_phdr`, ...) are undefined as
any input's would be, and the error names `<mortar:odr-runtime>`. `off`
is the line without the option, byte for byte; a link without the
option reads the one field and nothing else.

Rows whose status the M1 and M2 packages change (`spec/input.md` §17,
`spec/resolve.md` §18). Their fields exist (§3); the rows stay `refused`
until the phase that honors them lands with its test:
- WP-021 (`mortar_input`): `-nostdlib`, `-rpath-link`,
  `--allow-shlib-undefined`, `--no-allow-shlib-undefined` (M2).
- WP-022 stage 2 (`mortar_resolve`): `--warn-backrefs`, `--warn-common`,
  `--require-defined`, `-u`, `-e`, `--allow-multiple-definition`,
  `-z muldefs`, `--unresolved-symbols`, `--warn-unresolved-symbols`,
  `--error-unresolved-symbols`, `--ignore-unresolved-symbol`,
  `--start-group`/`--end-group` (no-op for resolution), and
  `--fortran-common`, whose help says "mortar's behavior, always on":
  mortar always takes an archive member for a common symbol
  (`spec/resolve.md` §3.3). `--no-fortran-common` stays `refused`.
  WP-152 made `-u`/`--undefined`, `--require-defined`,
  `--allow-multiple-definition`, `--no-allow-multiple-definition`,
  `-z muldefs`, `--whole-archive`, `--no-whole-archive`, `--start-lib`
  and `--end-lib` `supported` (`spec/resolve.md` §3.8, §4.2, §4.8; the
  evidence in `crates/mortar/tests/options.rs` and `tests/link/resolve`).
  `--unresolved-symbols`, `--warn-unresolved-symbols`,
  `--error-unresolved-symbols` and `--ignore-unresolved-symbol` stay
  `refused`: resolution decides them, but WP-152 found more than the
  status missing (opened for a probe, `ignore-all` gave an ignored
  function a nonzero address in a static executable where GNU ld gives
  0, and `-pie --warn-unresolved-symbols` failed to link).
  `--warn-backrefs` and `--no-warn-backrefs` became `supported` in
  WP-027d (resolve.md §4.6's simulation had landed; the evidence is in
  `crates/mortar/tests/options.rs`); `--warn-backrefs-exclude` stays
  `refused`.
- WP-024 (`mortar_layout`, `spec/layout.md` §20.2): `-z max-page-size`
  (L-74), `-z separate-code`, `-z noseparate-code` (L-71), `-z relro`,
  `-z norelro` (L-64, L-65), `-z execstack`, `-z noexecstack` (L-84).
  `Options::from_plan` honors them; the rows became `supported` in
  WP-027d, with their evidence (`crates/mortar/tests/options.rs`).
  `-z start-stop-visibility` stays `refused`: §20.2 makes only the value
  `protected` a no-op (it could become `partial`). `--image-base` and `-Ttext-segment`
  wait for M2 (L-68); `--no-rosegment`, `--no-warn-execstack`,
  `--error-execstack`, `-z start-stop-gc`, `-Ttext`, `-Tdata`, `-Tbss`,
  `--section-start` and `-z keep-text-section-prefix` stay `refused`.
- M2, dynamic executables (`spec/dyn.md` §2.3 and request R-C1). WP-042
  set what was true while dynamic output was refused (`--hash-style`
  `partial`, `gnu` only; `-z notext` and `-z textoff` `refused`;
  `--no-copy-dt-needed-entries` `noop`; `-no-pie` `supported`; the `Plan`
  fields `dynamic.init` and `dynamic.fini`) and kept the statuses of the
  join in a second table; WP-053, the join (AArch64), moved them into the
  table and deleted that table and the guard that refused the gates
  (`-pie`, `-dynamic-linker`, `-Bdynamic`). The statuses:
  - `supported`: `-pie`, `-dynamic-linker`, `-Bdynamic`, `--as-needed`,
    `--no-as-needed` (`spec/dyn.md` D-11), `-E` (and
    `--export-dynamic`), `-rpath`, `-init`, `-fini`, `-z text`, and
    `--export-dynamic-symbol` (one more source of `exported`, D-21;
    `spec/dyn.md` §15 Q9, closed);
  - `partial`: `--hash-style`, `gnu` only (D-20);
  - `-z norelro` stays `supported`; in a dynamic output it also moves
    what RELRO would hold (D-33, D-42, D-49; plumb P-D4);
  - `refused` by name at M2 (no census line passes them; lld 19 warns
    that it does not know the last two): `-z notext` (D-47),
    `-z nocopyreloc`, `--spare-dynamic-tags`, `--copy-dt-needed-entries`
    (D-15), `--dynamic-list` (M4), `-z nodynamic-undefined-weak` and
    `-z dynamic-undefined-weak` (D-25);
  - `noop`: `--no-copy-dt-needed-entries` (the default; D-15);
  - rows R-C1 does not name whose reason was the refusal (WP-042):
    `-rpath-link` and `--no-dynamic-linker` `supported`; the five
    `-Bsymbolic` rows, `--no-undefined` and `-z defs` stay `noop` with a
    reason that no longer names the refusal (GNU ld and mold write
    nothing for `-Bsymbolic` in a PIE; lld writes `DF_SYMBOLIC`).
- **M4, shared-object output** (`spec/dyn.md` §17, request R-C10; the
  packages are the WP-110 record's §D) [planned]. A row's status is one
  for every output kind; an option whose `Shared` effect a later package
  implements is refused **in a `Shared` link** by the front
  (`Refused{Option, spelling}`, with the note naming the package's
  rule) until that package lands, and keeps its executable status
  otherwise (WP-053's guard is the model). Per package:
  - WP-117 (`Shared` front, resolve and scan, AArch64): `-shared`
    (`-Bshareable`) `supported`, with x86-64 and i386 `Shared` links
    refused by `feature` until WP-127 and WP-130; `-h`/`-soname`
    `supported` (`DT_SONAME`, written by WP-118); `-Bsymbolic-functions`
    and `-Bno-symbolic` `supported` (D-112; Python's `LDSHARED` line
    passes `-Bsymbolic-functions`, the record's §A.4); `-z defs` and
    `--no-undefined` `supported` (D-118: they refuse a `Shared` link's
    undefined references; in an executable nothing changes, which their
    reason already says); `--allow-shlib-undefined` and
    `--no-allow-shlib-undefined` `supported` in every kind (the
    default per kind: D-13 for executables, D-118 for `Shared`); the
    `-shared` status text is the record's §G.4 (written with the row).
    In a `Shared` link until their packages: `-Bsymbolic`,
    `--dynamic-list`, `--exclude-libs`, `-init`, `-fini`,
    `-z nodelete`/`nodlopen`/`initfirst`/`interpose`/`origin` were
    refused (WP-128 landed them; `SHARED_LATER` is empty).
  - WP-118 (`Shared` synth, layout, image): `-z now` in `Shared`
    (`DF_BIND_NOW`, `DF_1_NOW`, D-103; rustc's `cdylib` line passes
    `-z relro,-z now`); the row is `supported` already.
  - WP-125 (versions): `--version-script` (and GNU ld's one-dash
    `-version-script`, an abbreviation, §2.4) `supported`; several
    `--version-script` options are one script, in order (resolve.md
    §9.7 rule 13; all three); `--no-undefined-version` `supported`;
    `--undefined-version` `supported` (the default: D-114 accepts a name no
    file defines, with a warning; the last of the two wins); in an
    executable `--version-script` is refused by name (D-114: the
    references disagree). Landed (WP-125).
  - WP-128 (the option tail): `-Bsymbolic` `supported` (D-112, D-103:
    `DT_SYMBOLIC`, `DF_SYMBOLIC`; in an executable it does nothing, as
    GNU ld and mold), `--dynamic-list` `supported` in every kind (D-112;
    the grammar is WP-125's, `extern "C++"` refused by name; in an
    executable it exports the listed definitions, resolve.md §9.6),
    `--exclude-libs` `partial` (D-110 item 1; an entry naming an archive
    without its `.a` is refused by name), `-z nodelete`, `-z nodlopen`,
    `-z initfirst`, `-z interpose`, `-z origin` `supported` in `Shared`
    only (D-103's flags; in an executable they are refused by name,
    `SHARED_ONLY`: GNU ld drops the first three there and lld and mold
    write them), `-init`/`-fini` in `Shared` (D-102 rows 3-4: an
    explicit name that is not defined is refused,
    `undefined-init-symbol`; one that is not code is refused by name).
    `--whole-archive` is WP-152's. Landed (WP-128).
  - Refused by name, class `option`, in every kind, not planned:
    `--Bsymbolic-non-weak`, `--Bsymbolic-non-weak-functions` (GNU ld
    does not know them; their `noop` status of WP-027e becomes
    `refused`), `--dynamic-list-data`, `--dynamic-list-cpp-new`,
    `--dynamic-list-cpp-typeinfo` (D-112), `-z notext`/`-z textoff`
    (D-47, D-117), `-f`/`--auxiliary`, `-F`/`--filter` (D-102),
    `--spare-dynamic-tags` (D-102); with `--incremental`, `-shared`
    stays IN-12's (`spec/incr.md` IN-12a, request R-N10).
  - Rows whose executable `noop` reason names a shared object
    (`-Bsymbolic`, `-Bsymbolic-functions`, `-Bno-symbolic`,
    `--no-undefined`, `-z defs`): when the row becomes `supported` its
    help says what it does in a `Shared` link and that an executable
    needs nothing (D-112: GNU ld and mold write nothing for
    `-Bsymbolic` in a PIE).

### 4.3 What the facade does with a parse

`mortar::invoke(prog, args)` returns the `Outcome` and the color choice;
`mortar::link(args)` returns the `Outcome`. In order:

1. If the line has no error and asks `--version` or `--help`, answer it
   (§5): the text goes to standard output, with status 0.
2. If the line has no error, asks `-v` or `-V`, and names no input,
   answer it the same way.
3. Otherwise, emit:
   - the grammar diagnostics;
   - the refusals of the refused options (§4.4);
   - if the line has no error, the refusal of linking itself
     (`Refused{Feature, "linking"}`, phase 11), which stays until M1.

   They go into a collector with the policy of §3.9. The result is
   `Outcome::Failed`.

A line with errors therefore never reaches "linking": the pipeline stops
at the first phase that reports an error (`spec/diag.md` §7).

`-v` given with inputs loses its version line while every link fails,
because a failed link prints nothing on standard output
(`spec/diag.md` §7).

### 4.4 Refused uses

- `Parsed::uses` records every option used: the row, the argument index,
  the spelling as completed, and the value.
- `refused_uses()` returns the first use of each refused row, and every
  use of a `partial` row with a refused value, in argument order. GCC
  passes `-plugin-opt=` seven times, and a row is reported once.
- A use whose field is last-wins (`Lands::last_wins`: the plain switches
  such as `--gc-sections`/`--no-gc-sections`, and `--build-id` with
  `--no-build-id`) and is overridden by a later use of the same field is
  not a use for this rule: the plan is the one the line gives without it.
  `--gc-sections --no-gc-sections` links; `--no-gc-sections
  --gc-sections` is refused.
- `refusals()` turns them into `Refused{Option, spelling}`, for example
  `unsupported option: -o` or `unsupported option: -z nodelete`, and a refused
  value into `Refused{OptionValue, spelling=value}` with the row's note,
  for example `unsupported option value: --build-id=md5`.

### 4.5 `docs/status/options.md`

- `status_markdown()` generates the file from the table. It has:
  - the counts;
  - one section per group, with one line per row: spellings, value,
    linkers, targets, status, the `Plan` field, and the help.
- `tests/table.rs` compares the file with the generated text.
- `MORTAR_BLESS=1 cargo test -p mortar_cli --test table` rewrites it.

## 5. Requests

### 5.1 `--version`, `-v`, `-V`

- `--version` stops the parse and prints one line:
  `mortar <version> (compatible with GNU ld)`.
  - "compatible with GNU ld" is what build systems and drivers look for
    to treat a linker as GNU ld's dialect; mold prints the same words.
  - GCC's `-fuse-ld=mortar` runs `ld.mortar`, and the line is the same
    under any name.
- `-v` prints the same line and parsing goes on.
- `-V` prints the line, then `  Supported emulations:` and the four
  names, each indented by three spaces, as GNU ld does.
- `-v` and `-V` (either, any number of times; `-V`'s text if both) print
  on standard output, through descriptor 1:
  - with no input file, the text alone, exit status 0;
  - on a line with inputs, the text once, before the link, which then
    goes on: the exit status is the link's, and a link that fails (or
    whose inputs cannot be opened) has still printed the text. A line
    whose parse fails, or that names a refused option, prints nothing
    and links nothing (§4.3). Measured (WP-112): GNU ld 2.44, lld and
    mold print their version line and link.
- `--verbose` and `--verbose=N` (`N` a decimal number, as the row says,
  else `invalid-option-value`; measured, GNU ld 2.44 also accepts a
  word that is not a number) print `-V`'s text on standard output, through
  descriptor 1 (never by opening a path), WP-111:
  - with no input file, the text alone, exit status 0, and no
    `no-input-files` error (GNU ld's behaviour: exit 0; lld and mold
    fail with no input);
  - on a link, the text after a successful link, before `-M`'s map; a
    failed link prints nothing more than its diagnostics.
  GNU ld also prints its built-in script between `=====` lines and an
  `attempt to open` line per file it tries. mortar has no built-in script
  (it does not print a fake one) and no built-in search directories (so
  no `SEARCH_DIR` lines), and does not trace its opens (`-t` is the
  trace). Measured (WP-111): BusyBox's `scripts/trylink` sends the text
  to a file and reads it only to show a failed link's output; no project
  of the realbuild survey's configure runs (autotools, libtool's
  `ld --verbose`-free probes, CMake 3.31's modules) reads `ld --verbose`.

### 5.2 `--help`

- `--help` stops the parse and prints the table:
  - `Usage: <prog> [options] file...`;
  - four lines on what `[refused]`, `[noop]` and `[partial]` mean;
  - the groups, each with its rows: spellings with their values, the
    help line, `(T only)` for target rows, and the status mark;
  - an `@FILE` line;
  - `<prog>: supported targets: elf64-littleaarch64 elf64-x86-64
    elf32-i386`;
  - `<prog>: supported emulations: aarch64linux aarch64elf elf_x86_64
    elf_i386`.
- libtool looks for `no-whole-archive` and for
  `: supported targets:.* elf` in it.
- `--help` lists every row.

## 6. The census

Real link lines are captured with a recording `ld`, then parsed. The
recorder is reached as follows:
- gcc: `-B<dir>/`;
- clang: `--ld-path=`;
- rustc: `-C link-arg=-B<dir>/`.

The census is `crates/mortar_cli/tests/census.rs`. Every line must parse
without an error and give:
- the expected kind;
- the expected target (from `-m`);
- the expected libraries;
- the compiled object.

### 6.1 Matrix

| driver | variants |
|---|---|
| `gcc`, `g++`, `gfortran` (AArch64) | all ten below |
| `clang-19`, `clang-22`, `clang++-19`, `clang++-22` (AArch64) | all ten |
| `x86_64-linux-gnu-{gcc,g++,gfortran}` | all ten |
| `i686-linux-gnu-{gcc,g++}` | all ten |
| `clang-22 --target=x86_64-linux-gnu`, `--target=i686-linux-gnu` | all ten |
| `rustc` | default, `-g`, `+crt-static`, `cdylib`, `-C lto` |

The variants:

| variant | kind |
|---|---|
| default | `PieExecutable` |
| `-no-pie` | `Executable` (gcc passes no `-no-pie` to ld) |
| `-static` | `StaticExecutable` |
| `-static-pie` | `StaticPie` |
| `-shared` | `Shared` |
| `-r` | `Relocatable` |
| `-pthread` | `PieExecutable` |
| `-Wl,--gc-sections,--as-needed` | `PieExecutable` |
| `-flto` | `PieExecutable` |
| `-g` | `PieExecutable` |

For rustc, `crt-static` gives `StaticExecutable` and `cdylib` gives
`Shared`.

### 6.2 The rows real lines use

The 33 rows the census uses:

`-(`, `-)`, `--as-needed`, `--build-id`, `--eh-frame-hdr`,
`--fix-cortex-a53-843419`, `--gc-sections`, `--hash-style`,
`--no-as-needed`, `--no-dynamic-linker`, `--no-undefined-version`,
`--pop-state`, `--push-state`, `--sysroot`, `--version-script`,
`-Bdynamic`, `-Bstatic`, `-EL`, `-I` (`-dynamic-linker`), `-L`, `-X`,
`-l`, `-m`, `-o`, `-pie`, `-plugin`, `-plugin-opt`, `-r`, `-shared`,
`-z noexecstack`, `-z now`, `-z relro`, `-z text`.

The test fails when a line uses a row outside this list. These rows are
the order in which options should become supported. Of them, `-EL`,
`-plugin`, `-plugin-opt`, `--as-needed`, `--no-as-needed`,
`--hash-style`, `--no-dynamic-linker` and `-z text` are noops today;
`-(`, `-)`, `--build-id`, `--eh-frame-hdr`, `--fix-cortex-a53-843419`,
`--pop-state`, `--push-state`, `--sysroot`, `-Bstatic`, `-L`, `-X`, `-l`,
`-m`, `-o`, `-z noexecstack`, `-z now` and `-z relro` are supported or
partial (WP-027e); the rest are refused.

## 7. Against GNU ld

### 7.1 The test

`crates/mortar_cli/tests/differential.rs` runs each case through the
`ld.bfd` of each target and through `parse`, with a real object built in
a scratch directory.
- GNU ld accepts a case when it exits with status 0.
- mortar accepts it when the parse has no error.

Each case is one of:
- both accept;
- both reject;
- only GNU ld accepts, with a reason;
- only mortar accepts, with a reason.

The test fails when the outcome is not the recorded one.

### 7.2 Disagreements

| case | GNU ld | lld | mold | mortar | why |
|---|---|---|---|---|---|
| `--` | drops the rest of the line | error | error | error | dropping arguments silently is never wanted |
| `-sx` (grouping) | warning, accepted | error | error | error | as lld and mold |
| ambiguous abbreviation (`--a=x`) | sometimes accepted | no abbreviations | no abbreviations | error, naming the candidates | §2.4 |
| unknown `-z` keyword | warning | warning | warning | error | principle 1: no option is ignored unless its row says so |
| `-z now=1` | ignored | warning | warning | error | the same |
| `-z max-page-size` (no value) | ignored | error | warning | error | the same |
| `-z max-page-size=x` | error | error | crashes (2.37.1) | accepted at parse | sizes are checked by the phase that uses them (follow-up) |
| `-z force-bti` on x86-64 | warning | n/a | n/a | error | §2.9 |
| an option of another target with no `-m` | error | n/a | n/a | accepted until the target is known | one mortar links for every target |
| `-m elf_x86_64` on the AArch64 `ld.bfd` | error | n/a | n/a | accepted | the same |
| `-G` with no size | `-shared` | `missing argument` | unknown option | `missing-argument` | `-G` is GNU's size option; the `-shared` reading is undocumented |
| `-R` | `--just-symbols`, or `-rpath` when it names a directory | rpath | rpath | `JustSymbols` | GNU's reading; the directory case is `spec/input.md` §4.8 |
| `-entry=main` | the entry option | the entry option | `-e ntry=main` | the entry option | GNU's and lld's reading |
| `-m` inside another option's value | taken as the emulation (pre-scan) | not | not | not | §2.8 |
| lld-only and mold-only options | error | accepted | accepted | read (refused unless supported) | one command line for every driver |
| `@missing` | literal argument | error | error | literal argument | GNU's reading |
| `--sysroot DIR` (separate value) | ignored (only `--sysroot=` is read, before the parse) | sysroot | sysroot | sysroot | ignoring a sysroot silently is never wanted; lld's and mold's reading |

## 8. Tests

| file | covers |
|---|---|
| `crates/mortar_cli/tests/grammar.rs` | §2.1-§2.5, §2.7-§2.10, §3, §5 |
| `crates/mortar_cli/tests/response.rs` and the unit tests in `src/response.rs` | §2.6 |
| `crates/mortar_cli/tests/table.rs` | §2.2, §4.1, §4.2, §4.5, §5 |
| `crates/mortar_cli/tests/census.rs` | §6 |
| `crates/mortar_cli/tests/differential.rs` | §7 |
| `crates/mortar/src/lib.rs` (unit tests) | §4.3, §3.9, §5 |
| `crates/mortar/tests/cli.rs` | §2.1 (non-UTF-8), §5.1 (through a symlink), §6 with `spec/diag.md` §7 (gcc running mortar as `ld`) |
| `crates/mortar/tests/written.rs`, `crates/mortar/src/written.rs` (unit tests), `crates/mortar_cli/tests/table.rs` (`every_path_valued_row_is_classified`) | §2.6.4, §3.14 |
| `crates/mortar/tests/streams.rs` (one test per row of `spec/landed/cli/WP-095.md`'s table), `crates/mortar_diag/tests/streams.rs` (the source scan) | §3.15 |

Each test names the section it checks in a `// spec: cli.md §...`
comment.
