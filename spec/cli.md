# `mortar_cli`: the command line

Status: normative (WP-008).

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
  - `args`, the command line after response files are expanded (§2.6).
- `Parsed::failed()` is true when any diagnostic is an error.
- `Parsed::refused_uses()` and `Parsed::refusals()` cover the refused
  options (§4.4).
- `check_target(&Plan, Target)` reports the options of other targets
  (§2.9).
- `rows()` is the table (§4).
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
  - a choice of words: `--hash-style=sysv|gnu|both|none`,
    `--color-diagnostics=auto|always|never`, `--icf=all|safe|none`, and
    the others in `docs/status/options.md`;
  - a decimal count: `--error-limit`, `--thread-count`;
  - a build-ID style, or `0x` followed by hex digits.

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
- They are read whatever the target. Each use is recorded in
  `plan.target_options`.
- When `-m` names the target, `check_target` runs at the end of the line.
  An option of another target is `unknown-option`, with the note
  "X is an option of T only; this link is for U".
- When the line has no `-m`, the target comes from the first input. The
  phase that reads it calls `check_target` (follow-up for WP-021).
- GNU ld warns about another target's `-z` keyword and goes on; it
  rejects another target's long option. mortar rejects both.

### 2.10 Keys and order

- The diagnostics of the grammar have the key `Key::new(0, arg, seq)`:
  - phase 0;
  - `arg` is the index of the argument in the expanded line;
  - `seq` is a counter that keeps the order within one argument.
- Errors that only show at the end of the line use the index one past
  the last argument: `no-input-files` and `conflicting-options`.
- The refusals of options (§4.4) have the key
  `Key::new(0, arg, u64::MAX)`: they come after any grammar error of the
  same argument.
- The facade's own refusal of linking has phase 1 (§4.3).

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
  rejects both orders as well. `-r` with `-no-pie` is relocatable.
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
- `rpath_link`;
- `dynamic_linker` (`Path` or `None`);
- `new_dtags`;
- `hash_style`;
- `pack_dyn_relocs`;
- `export_dynamic` (`-E`, `--export-dynamic`, `--no-export-dynamic`);
- `bsymbolic`, one of the four `-Bsymbolic` forms.

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
- the sizes `max-page-size`, `common-page-size` and `stack-size`, as
  text (§2.7).

Each one is `None` when the line does not use it. Every other keyword
lands in `plan.other` (§3.12).

### 3.8 Output shaping

`plan.shaping` holds:
- `gc_sections`;
- `icf`;
- `build_id` (`None` means no note; `--build-id` alone means `Default`);
- `strip` (`-s`, `-S`, `--strip-*`);
- `discard` (`-x`, `-X`, `--discard-*`);
- `compress_debug_sections`;
- `eh_frame_hdr`;
- `magic` (`-n`, `-N`, `--omagic`, ...);
- `image_base`;
- `starts`: `-Ttext`, `-Tdata`, `-Tbss`, `-Ttext-segment`,
  `-Trodata-segment`, `-Tldata-segment` and `--section-start SEC=ADDR`.
  A `--section-start` without `=` is `invalid-option-value`.
- `emit_relocs`;
- `gdb_index`.

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
- `noinhibit_exec`.

The facade does two things with these:
- It applies `error_limit`, `fatal_warnings` and `no_warnings` to the
  collector's `Policy` (`spec/diag.md` §6). An absent `error_limit` is
  the default, 20.
- It returns `color` to the executable. The executable colors when the
  choice is `Always`, or `Auto` with standard error a terminal and
  `TERM` not `dumb`.

### 3.10 Bookkeeping

`plan.bookkeeping` holds:
- `map` (`-Map`);
- `print_map` (`-M`);
- `cref`;
- `reproduce`;
- `threads`: `--threads[=N]` (N is given) and `--thread-count N` set a
  count; `--threads` alone means `All`; `--no-threads` is a count of 1;
- `time_phases` and `verify`, which are mortar's own options;
- `verbose`;
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

## 4. The table and the status of options

### 4.1 Rows

There is one row per option, in `crates/mortar_cli/src/rows.rs`. The
table has 471 rows. A row has:

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
- two options of mortar's own, `--time-phases` and `--verify`.

| origin | rows |
|---|---|
| GNU ld (with or without lld and mold) | 292 |
| lld only | 117 |
| mold only | 21 |
| lld and mold, not GNU ld | 39 |
| mortar's own | 2 |

### 4.2 Status

Every row has one status:
- `supported`: mortar does what the option asks.
- `noop(reason)`: the option needs nothing done. The reason says why
  (GNU ld ignores it too; a memory hint for BFD; every target is
  little-endian; mortar loads no plugin).
- `refused`: read and recorded, but not carried out yet. This is the
  default.

The status today:
- 10 rows are `supported`:
  - `--error-limit`, `--fatal-warnings`, `--no-fatal-warnings`, `-w`;
  - `--color-diagnostics`, `--no-color-diagnostics`;
  - `--version`, `-v`, `-V`, `--help`.
- 17 rows are `noop`.
- 444 rows are `refused`.

A work package that implements an option changes the row's status in the
same change.

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
     (`Refused{Feature, "linking"}`, phase 1), which stays until M1.

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
- `refused_uses()` returns the first use of each refused row, in argument
  order. GCC passes `-plugin-opt=` seven times, and a row is reported
  once.
- `refusals()` turns them into `Refused{Option, spelling}`, for example
  `unsupported option: -o` or `unsupported option: -z relro`.

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

### 5.2 `--help`

- `--help` stops the parse and prints the table:
  - `Usage: <prog> [options] file...`;
  - three lines on what `[refused]` and `[noop]` mean;
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
`-plugin` and `-plugin-opt` are noops today; the rest are refused.

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
| `-R` | `--just-symbols`, or `-rpath` when it names a directory | rpath | rpath | `JustSymbols` | GNU's reading; the directory case needs the file system, and is the input phase's to decide (follow-up) |
| `-entry=main` | the entry option | the entry option | `-e ntry=main` | the entry option | GNU's and lld's reading |
| `-m` inside another option's value | taken as the emulation (pre-scan) | not | not | not | §2.8 |
| lld-only and mold-only options | error | accepted | accepted | read (refused unless supported) | one command line for every driver |
| `@missing` | literal argument | error | error | literal argument | GNU's reading |

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

Each test names the section it checks in a `// spec: cli.md §...`
comment.
