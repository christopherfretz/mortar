# spec: mortar_script

Status: §1 to §7 normative for stage 1 (WP-009, crate
`crates/mortar_script`): the lexer, the commands of implicit linker scripts,
recognizing a script. Later stages (`SECTIONS`, `MEMORY`, `PHDRS`,
expressions and assignments, version scripts, dynamic lists) extend §1 and
§2 and are planned (§8). "MUST" and "SHOULD" carry their RFC 2119 meanings.
Every test names the section it checks with a `// spec: script.md §N`
comment; §7 maps sections to tests.

The language is GNU ld's, as its manual defines it (chapter "Linker
Scripts", cited by section name). Where the manual is silent or wrong, the
reference is the behavior of GNU ld 2.44 (`ld.bfd`), established by
experiment: a script handed to the linker as an input file of a minimal link
(`ld.bfd -o out t.ld e.o`), the result read from the diagnostics (`cannot
find <file>` shows a file-name token, `cannot find entry symbol <s>` the
`ENTRY` token, `nm` of a `-r` output the `EXTERN` symbols). lld 19 and 22
and mold 2.37 are the second and third witnesses. The current online manual
is newer than 2.44 (it documents `LIB`, which 2.44 does not know); 2.44's
behavior wins. GNU ld's source was not consulted.

## 1. Lexing

### 1.1 Bytes and positions

A script is bytes, not text: `parse` takes `&[u8]` (§4). The language's
tokens are ASCII; bytes outside ASCII may appear only in comments and quoted
strings (§1.3, §1.6), and a quoted file name is kept as the bytes it holds,
so a path that is not UTF-8 (legal on Linux) survives.

Every token carries its byte range (`Span { start, end }`). A position is
derived from an offset when a diagnostic needs one: the line is 1 + the
number of LF bytes before the offset, the column 1 + the number of bytes
since the last LF (`spec/diag.md` §2.3: columns count bytes). CR is
whitespace, not a line end. GNU ld reports a line only, and line 0 for an
error at the end of the file; mortar gives line and column, the end of the
file included.

### 1.2 Modes

GNU ld's lexer is modal: whether `a-b`, `=x`, `a,b` or `ah` is one token,
several or an error depends on the construct being read. mortar's lexer
takes the mode with each call (`Lexer::next(mode)`): the parser knows which
construct it is in, no token is lexed twice, and no state survives the
parser's knowledge of it.

| mode | GNU ld state | entered for | stage |
|---|---|---|---|
| `Script` | SCRIPT | statement level; the arguments of `ENTRY`, `OUTPUT`, `SEARCH_DIR`, `OUTPUT_FORMAT`, `OUTPUT_ARCH`, `TARGET`, `STARTUP`; the name after `INCLUDE`; the `(` after `INPUT`/`GROUP` | 1 |
| `InputList` | INPUTLIST | the tokens after the `(` of `INPUT`, `GROUP` and `AS_NEEDED`, through the matching `)` (the `(` after `AS_NEEDED` included) | 1 |
| `Expression` | EXPRESSION | the tokens after the `(` of `EXTERN`, through its `)`; in later stages every expression | 1 |
| wildcard | WILD | input section descriptions inside `SECTIONS` (`*(.text .text.*)`, `EXCLUDE_FILE`, `SORT*`, `KEEP`) | later (§1.10) |
| version | VERS_START, VERS_SCRIPT, VERS_NODE | `VERSION { }` and version-script files; dynamic lists | later (§1.10) |
| MRI | MRI | `-c` scripts | refused (`-c` is `spec/cli.md`'s) |

### 1.3 Whitespace and comments

- Whitespace in every mode: space, tab, CR, LF. Form feed and vertical tab
  are invalid characters (§1.9), as in GNU ld.
- `/*` to the next `*/` is a comment in every mode, wherever a token may
  start. Comments do not nest. An unterminated comment is an error at its
  `/*` (GNU ld: `EOF in comment`). `/*` inside a name does not start a
  comment: the name takes the `/` (`a/*c*/` is the name `a/` and then an
  invalid `*`), as in GNU ld. `//` is no comment (in `Script` mode it is
  the name `//`).
- `#` to the end of the line (or the file) is a comment in `Script` and
  `Expression` mode, also directly after a name (`a#b` is `a`). It is an
  error in `InputList` mode (§2.4): GNU ld reads `#` there as a stray byte
  and the text after it as file names; lld and mold read a comment.
- A NUL byte is an error everywhere, in comments too: GNU ld's handling of
  NUL is not observable as anything but "file not recognized", and no
  script needs one.
- Comments may hold any other byte, UTF-8 included (GNU ld and lld accept
  `/* é */`).

### 1.4 Names

| mode | first byte | following bytes |
|---|---|---|
| `Script`, `InputList` | `A-Z a-z _ / . \ $ ~` | the first-byte set, digits, and `+ : [ ] , = -` |
| `Expression` | `A-Z a-z _ . \ $` | the first-byte set, digits, and `/ ~` |

(Measured by a sweep of every printable ASCII byte as the first and as a
middle byte of `ENTRY(...)` and `EXTERN(...)`.) Consequences, all as in
GNU ld:

- In `Script` mode `,` and `=` are name bytes: `ENTRY(a,b)` is one symbol
  `a,b` to GNU ld, `x=1;` is the name `x=1` followed by `;` (a syntax
  error), and `x = 1;` is an assignment. §2.1 makes a comma inside an
  unquoted name an error.
- In `Expression` mode `-`, `,`, `:`, `+`, `=` end a name: `EXTERN(a-b)` is
  `a`, `-`, `b`, a syntax error; `EXTERN(a,b)` is two symbols.
- `InputList` mode reads two more one-token forms: `=` followed by one or
  more name bytes (the sysroot form, spelling kept: `=/x.so`, `=x`; `= x`
  is no token, §2.4), and `-l` followed by one or more name bytes (a
  library: `-lfoo`, `-l:bar.a`, `-l=x`; `-l foo`, `-L/x` and a lone `-` are
  no tokens).
- A name is a keyword when its spelling is one of the mode's (§1.7).

### 1.5 Integers

Integer tokens exist in `Script` and `Expression` mode, never in
`InputList` mode (`INPUT($abc)` names a file).

Where an integer starts:

- at a digit: the token is the whole run of ASCII letters and digits;
- at `$`: `$` + one or more hex digits + an optional `K`/`k`/`M`/`m` is an
  integer unless the name read from the same place is longer (`$ab` and
  `$10K` are integers, `$zz`, `$ab_` and `$1g` names, `$` alone a name);
- in `Expression` mode only, at a letter: a name that consists of one or
  more hex digits followed by one of `h H x X` is an integer
  (`EXTERN(BX)`, `EXTERN(ah)`, `EXTERN(ffx)` are syntax errors in GNU ld;
  `ab`, `add`, `bad`, `fb`, `fd`, `ao`, `aB`, `ahx`, `gh` are names). The
  `b`/`d`/`o` base letters do not make a letter-initial name an integer.

The forms and their values (`parse_integer`):

| form | base | example |
|---|---|---|
| `0x`/`0X` + hex digits | 16 | `0x10` = 16 |
| `$` + hex digits | 16 | `$ff` = 255 |
| `0` + digits | 8 | `010` = 8 |
| digits not starting with `0` (or `0` alone) | 10 | `10` |
| any of the above + `K`/`k` or `M`/`m` | x 1024, x 1024 * 1024 | `0x10K` = 0x4000, `010K` = 0x2000 |
| digits (hex digits for `h`) + `h H x X` | 16 | `10h`, `0ffh`, `FFH` (`Expression`: `ffh`) |
| digits + `o O` | 8 | `10o` = 8 |
| digits + `b B` | 2 | `10b` = 2 |
| digits + `d D` | 10 | `10d` = 10 |

A base letter does not combine with `0x` or with a multiplier (`0x10h`,
`10bK` are errors); there is one multiplier (`1KK`, `1MK`) and no `G`.
Not numbers: `1_000`, `1e3`, `1.5`, `10KB`, `0x1g`, `1a`.

GNU ld is silent where mortar is not (`docs/ARCHITECTURE.md`, principle 1):

| input | GNU ld 2.44 | mortar |
|---|---|---|
| a digit outside the base: `08`, `019`, `0778`, `08K` | stops at the digit without a word: 0, 1, 0x3f, 0 | error `malformed number '08': '8' is not an octal digit` |
| a value above 2^64 - 1 | saturates to 2^64 - 1 | error `... does not fit in 64 bits` |
| a multiplier that overflows: `17592186044416M` | wraps to 0 | error |
| `0x` alone | 0 | error `no digits` |

GNU ld lexes `1a` as `1` then `a`; mortar reads the run `1a` and rejects it
as one malformed number. Both are errors wherever stage 1 reads a token;
the later expression stage must keep it so (no grammar position accepts an
integer followed directly by a name).

In stage 1 an integer is never valid: it appears only in error messages
(`ENTRY(123)` is a syntax error in GNU ld; lld accepts it as a symbol). Its
value is computed so that the later stages inherit a checked lexer.

### 1.6 Quoted strings

`"` to the next `"`, in every mode. No escapes: `"a\"` is the two bytes
`a\`. The content may be empty (§2.1 rejects that), may hold newlines (GNU
ld, lld and mold all keep `"a\nb.o"` as a name with a line feed in it) and
any byte but NUL (an error at the NUL). An unterminated string is an error
at the opening quote (GNU ld: an invalid `"`, "file not recognized").
Quoting defeats keywords (`ENTRY("ENTRY")`, `INPUT("AS_NEEDED")`) and the
name-byte rules (`INPUT("a,b")` is one file `a,b` in all three linkers). A
string directly followed by a name is two tokens (`INPUT("a.o"b.o)` is two
files in all three linkers).

### 1.7 Keywords

Keywords are case-sensitive (`entry` is a name). Each mode has its own set
(measured: in `Script` mode `ENTRY(<word>)` is a syntax error for exactly
these words, in `Expression` mode `EXTERN(<word>)`):

- `Script`: ABSOLUTE ADDR AFTER ALIGN ALIGNOF ALIGN_WITH_INPUT ASSERT AT
  BEFORE BIND BLOCK CONSTANT DATA_SEGMENT_ALIGN DATA_SEGMENT_END
  DATA_SEGMENT_RELRO_END DEFINED ENTRY EXTERN FLOAT FORCE_COMMON_ALLOCATION
  FORCE_GROUP_ALLOCATION GROUP HIDDEN HLL INCLUDE INHIBIT_COMMON_ALLOCATION
  INPUT INSERT LD_FEATURE LENGTH LOADADDR LOG2CEIL MAP MAX MEMORY MIN NEXT
  NOCROSSREFS NOCROSSREFS_TO NOFLOAT ONLY_IF_RO ONLY_IF_RW ORIGIN OUTPUT
  OUTPUT_ARCH OUTPUT_FORMAT OVERLAY PHDRS PROVIDE PROVIDE_HIDDEN
  REGION_ALIAS SEARCH_DIR SECTIONS SEGMENT_START SIZEOF SIZEOF_HEADERS
  SPECIAL STARTUP SUBALIGN SYSLIB TARGET VERSION, and the lower-case `l`,
  `len`, `o`, `org` (`lenx` is a name). Not keywords here: AS_NEEDED, LIB,
  COPY, DSECT, INFO, NOLOAD, READONLY, TYPE, KEEP, FILL, SORT*, BYTE,
  `sizeof_headers`.
- `Expression`: ABSOLUTE ADDR ALIGN ALIGNOF ALIGN_WITH_INPUT ASSERT AT
  BIND BLOCK CONSTANT COPY DATA_SEGMENT_ALIGN DATA_SEGMENT_END
  DATA_SEGMENT_RELRO_END DEFINED DSECT HIDDEN INCLUDE INFO LENGTH LOADADDR
  LOG2CEIL MAX MIN NEXT NOCROSSREFS NOCROSSREFS_TO NOLOAD ONLY_IF_RO
  ONLY_IF_RW ORIGIN OVERLAY PROVIDE PROVIDE_HIDDEN READONLY SEGMENT_START
  SIZEOF SIZEOF_HEADERS SPECIAL SUBALIGN TYPE. (`EXTERN(ENTRY)`,
  `EXTERN(len)` name symbols.)
- `InputList`: AS_NEEDED only (`INPUT(GROUP INCLUDE)` names two files).

`Keyword` is one enum over all three sets; `Keyword::in_mode` is the table.

### 1.8 Operators and separators

- `Script` mode: `( ) { } ; , : ! > = + -`, `==`, `!=`, `>=` and the
  assignment operators `= += -= *= /= <<= >>= &= |= ^=`. `/` starts a name,
  so `/=` is the operator only when no name byte follows (`/=x` is a name):
  an operator wins a tie in length, a longer name wins. `<`, `*`, `&`, `|`,
  `^`, `%`, `?`, `@`, `` ` ``, `'` alone are invalid characters (§1.9);
  `%=` is no operator (GNU ld rejects `x %= 1;`).
- `Expression` mode: the operators of the manual's "Operators" section,
  `( ) { } ; , : ? ! ~ + - * / % & | ^ < > = << >> <= >= == != && ||`, and
  the assignment operators above. `@ [ ] ` `` ` `` `'` are invalid.
- `InputList` mode: `(`, `)`, `,` only.

### 1.9 Invalid characters

Any byte that starts no token of the mode is an error at that byte,
`invalid character 'x'` (printable ASCII) or `invalid character byte 0xNN`
(anything else). This includes every byte above 0x7f and every control byte
but the whitespace of §1.3 outside comments and strings.

GNU ld does three different things with such a byte, none of which mortar
copies:

- in an implicit script (an input file), SCRIPT and EXPRESSION state: the
  file is "not recognized" and the link fails;
- under `-T`, the same byte is `ignoring invalid character '@' in script`
  and the link succeeds (exit 0);
- in INPUTLIST state, the byte is echoed to standard output and skipped
  without a diagnostic: `INPUT(1.o)` looks for `.o`, `INPUT(a.o;b.o)` for
  `a.o` and `b.o`, `INPUT(a/*c*/b)` for `a/`, `c` and `/b`.

### 1.10 Modes of later stages

Recorded so that the lexer is extended, not rewritten; not implemented,
not yet measured, open for the stage that needs them:

- **wildcard** (input section descriptions): names with glob bytes `* ? [ ]`
  and `-`; `EXCLUDE_FILE`, `SORT`, `SORT_BY_NAME`, `SORT_BY_ALIGNMENT`,
  `SORT_BY_INIT_PRIORITY`, `SORT_NONE`, `REVERSE`, `KEEP`,
  `INPUT_SECTION_FLAGS` are keywords there (none is a keyword of the three
  modes above: measured). The exact byte sets are to be swept as in §1.4.
- **version** (version scripts, `VERSION { }`, dynamic lists): `global:`,
  `local:`, `extern "C++"`, glob patterns, `;`-terminated nodes.
- **Expression** beyond `EXTERN`: the full operator set is lexed already
  (§1.8); the grammar and the values are the later stage's.

## 2. Commands of implicit scripts

A script parses to `Script { commands: Vec<Command> }`, each `Command` a
`CommandKind` and the span from its keyword to its closing parenthesis (or,
for `INCLUDE`, its name). The AST records spelling; what a command means for
a link is the consumer's (`mortar_input`, `mortar_cli`), and §2.3 to §2.8
state the rules the consumer applies.

### 2.1 Words

An argument (a symbol, a file, a directory, a format or architecture name)
is a `Word`: the bytes (an unquoted name as written, a string without its
quotes), whether it was quoted, its span. Every argument is read in the
command's mode and MUST be a name or a string:

- a keyword is an error, with a note to quote it (`ENTRY(ALIGN)`,
  `ENTRY(len)`: GNU ld's syntax error);
- an integer is an error, with the same note (`ENTRY(123)`, `EXTERN(BX)`);
- an empty string is an error (`ENTRY("")`: GNU ld accepts it and sets no
  entry, §5.2);
- an unquoted name containing `,` is an error at the comma, in every mode
  where `,` is a name byte (`ENTRY(a,b)`, `OUTPUT_FORMAT(a,b,c)`,
  `INPUT(a.o,b.o)`): GNU ld reads one name with a comma in it, lld and mold
  split it (§5.2). The note says to put a space before the comma or quote
  the name.

### 2.2 Statements

A script is a sequence of statements. `;` is the empty statement and may
appear any number of times between statements (`ENTRY(a);;;ENTRY(b)`); it
is not kept in the AST. A statement starts with a keyword that is a command
of §2.3 to §2.8 (parsed), a command of §2.10 (refused), a name or string
followed by an assignment operator (refused, §2.10), or it is an error at
its first token:

- a name: `unknown linker script command name 'FROB'` (GNU ld: syntax
  error; lld: `unknown directive`). A note explains `AS_NEEDED` outside a
  list, and a name containing `=` (`x=1`: put spaces around the operator);
- another keyword: `keyword 'ALIGN' cannot start a linker script command`;
- anything else: `expected a linker script command, found ...`.

Commands follow one another with or without whitespace (`INPUT(a)ENTRY(b)`).

### 2.3 INPUT, GROUP, AS_NEEDED, STARTUP

```
INPUT ( list )     GROUP ( list )     STARTUP ( word )
list  = entry *( [","] entry )
entry = file / "AS_NEEDED" "(" list ")"
```

- The keyword is read in `Script` mode, the list in `InputList` mode.
- Entries are separated by whitespace, by one `,` standing as its own token
  (`a ,b`, `a , b`), or both. A leading comma, two commas in a row, a comma
  before `)`, and an empty list are errors (GNU ld: syntax errors; lld and
  mold accept `INPUT()`; lld and mold do not accept a standalone comma at
  all: they look for a file named `,`).
- `AS_NEEDED` is a keyword only in a list and must be followed by `(`. It
  nests in itself (GNU ld and mold accept `AS_NEEDED(AS_NEEDED(x))`; lld
  does not). Nesting deeper than `MAX_AS_NEEDED_DEPTH` (16) is an error, so
  that recursion is bounded. Outside a list `AS_NEEDED` is a name and the
  statement an error (all three linkers).
- The AST is `CommandKind::Input(Vec<InputItem>)`, `Group(..)`, with
  `InputItem::File(InputFile)` and `InputItem::AsNeeded { items, span }`,
  in source order.
- `STARTUP ( word )` takes one file name in `Script` mode (manual: like
  `INPUT`, but the file becomes the first input of the link). lld and mold
  do not implement it (`unknown directive`).

What the consumer does (manual, "File Commands"; GNU ld 2.44):

- `INPUT`: the files join the link as if named on the command line at the
  position of the script ("In an implicit linker script, the files join
  the link at the point where that script is included", which affects
  archive searching).
- `GROUP`: as `INPUT`, and the files form an archive group, searched
  repeatedly until no new undefined references are created (as
  `--start-group`/`--end-group`).
- `AS_NEEDED`: as if the entries were listed directly, with `--as-needed`
  in effect for them and restored afterwards; it changes the treatment of
  ELF shared objects only.

### 2.4 File names in lists

An `InputFile` is `{ form, name, spelling }`: `spelling` is the token as
written (a `Word`), `name` the part the form looks up.

| spelling | form | name | example |
|---|---|---|---|
| a name or string not matching the rows below | `Path` | the spelling | `/lib/libc.so.6`, `libgcc_s.so.1`, `"a b.o"` |
| `=` + name, quoted or not | `Sysroot` | after `=` | `=/usr/lib/c.so` |
| `$SYSROOT/` + rest, quoted or not | `Sysroot` | after `$SYSROOT` | `$SYSROOT/lib/x.a` |
| `-l` + name (unquoted) | `Library` | after `-l` | `-lgcc` |
| `-l:` + name (unquoted) | `LibraryVerbatim` | after `-l:` | `-l:libx.a` |

Errors at the token (each a spelling the references read differently or
silently misread, §5.2):

- a comma in an unquoted name (§2.1);
- a name that starts with a digit (GNU ld skips the digit: `INPUT(1.o)`
  looks for `.o`; lld and mold for `1.o`). Quote it;
- any byte that starts no token of `InputList` mode, `#` included (§1.9);
- `-l` with no name after it (`-l foo`), `-l:` alone;
- `-l=name`: GNU ld prepends the sysroot to the *library name*
  (`--sysroot=sr -L... INPUT(-l=foo)` looks for `-lsrfoo`); lld and mold
  look for a library named `=foo`;
- a quoted name starting with `-l`: GNU ld reads a file name, lld and mold
  a library;
- `=` with no name (`= x`, `"="`), `$SYSROOT` not followed by `/`.

How `mortar_input` resolves an `InputFile` (rules measured on GNU ld 2.44,
lld 19 and mold 2.37; where they agree this is the behavior, where they
differ the choice is stated):

1. **Sysroot of an absolute path.** If a sysroot is in effect, the name
   starts with `/`, and the script file is inside the sysroot (its path,
   made absolute and with symbolic links resolved, has the sysroot's such
   path as a prefix by whole components), the file is the sysroot joined
   with the name, and only that: an absolute path missing inside the
   sysroot is an error, never looked for outside it (all three: `cannot
   find /x inside sr`). A script outside the sysroot uses the absolute
   path as written. (Measured with the script named by a relative path,
   by `./`-prefixed path, and with an absolute sysroot and a relative
   script; symbolic links on the way are not measured: §8.)
2. **`Sysroot` form.** The file is the sysroot joined with `name`,
   wherever the script is. With no sysroot (GNU ld's default here,
   `--print-sysroot` is empty) the name alone (`INPUT(=a.o)` finds `a.o`
   in all three). If a sysroot is set and `name` does not start with `/`,
   GNU ld and mold concatenate (`=a.o` with `--sysroot=sr` is `sra.o`) and
   lld joins with `/` (`sr/a.o`): mortar MUST make that an error naming
   both readings. `$SYSROOT` is GNU ld's alone (lld and mold look for a
   file literally named `$SYSROOT/...`); mortar follows GNU ld, as the
   manual documents it.
3. **Relative `Path`.** Tried in order, first found wins (all three
   agree): the directory containing the script (the script's path as
   given, its directory part: `sd/s.ld` with `INPUT(q.o)` finds `sd/q.o`);
   the current directory; then each directory of the library search path
   in order (§2.6).
4. **`Library` and `LibraryVerbatim`.** As `-lname` and `-l:file` on the
   command line at the script's position: the library search path, with
   the `-Bstatic`/`-Bdynamic` state then in effect choosing `libname.so`
   before `libname.a` (`spec/input.md`).
5. Diagnostics about a file (not found, unreadable) carry the script
   position of the entry as `NamedBy` context (`spec/diag.md` §2.4).

### 2.5 OUTPUT_FORMAT, OUTPUT_ARCH, TARGET

```
OUTPUT_FORMAT ( word )     OUTPUT_FORMAT ( word "," word "," word )
OUTPUT_ARCH ( word )       TARGET ( word )
```

All in `Script` mode. The three-name form is default, big-endian,
little-endian (manual, "Format Commands": the second is used with `-EB`,
the third with `-EL`, the first otherwise). The commas MUST be tokens of
their own: `OUTPUT_FORMAT(a , b , c)` and the quoted form parse;
`OUTPUT_FORMAT(a, b, c)` is a syntax error in GNU ld (the commas glue to
the names) and an error in mortar (§2.1); `OUTPUT_FORMAT(a,b,c)` is one
name to GNU ld and three to lld, an error in mortar. Two names is an error.

The AST keeps the names. The names the three targets use (GNU ld's own
default scripts):

| target | `OUTPUT_FORMAT` | `OUTPUT_ARCH` |
|---|---|---|
| AArch64 | `elf64-littleaarch64` (three-name form: `"elf64-littleaarch64", "elf64-bigaarch64", "elf64-littleaarch64"`) | `aarch64` |
| x86-64 | `elf64-x86-64` (all three) | `i386:x86-64` |
| 32-bit x86 | `elf32-i386` (all three) | `i386` |

A name that is not the link's target is the consumer's diagnostic, and it
MUST be an error: GNU ld 2.44 accepts any name in an implicit script without
a word (`OUTPUT_FORMAT(elf64-x86-64)` and `OUTPUT_FORMAT(elf64-frob)` in an
AArch64 link exit 0), lld rejects both (`e.o is incompatible with
elf64-x86-64`, `unknown output format name`), mold ignores the command.
GNU ld rejects a foreign `OUTPUT_ARCH` (`cannot represent machine`), lld
ignores it. `TARGET` sets the input format of later inputs (manual); any
name but the link's ELF format is a refusal of class `file-format` by the
consumer (`TARGET(binary)` reads raw files in GNU ld). lld does not support
`TARGET` names it cannot map to a target; mold implements neither
`OUTPUT_ARCH` nor `TARGET`.

### 2.6 SEARCH_DIR

`SEARCH_DIR ( word )`, one path, `Script` mode. An unquoted `=` cannot
start the path (`SEARCH_DIR(=/lib)` is a syntax error in GNU ld: `=` is an
operator in SCRIPT state), so GNU ld's default scripts quote it:
`SEARCH_DIR("=/usr/lib")`; the `=` stays in the `Word`.

What the consumer does: the path is added to the library search path after
every `-L` of the command line (manual: "Paths specified using the
command-line option are searched first"); a leading `=` or `$SYSROOT` is
the sysroot prefix as for `-L`. GNU ld applies a script's `SEARCH_DIR`s to
all of that script's `-l` entries, also to those before it
(`INPUT(-lfoo) SEARCH_DIR(L)` finds `L/libfoo.a`); lld only to later ones;
mold does not implement `SEARCH_DIR`. mortar takes GNU ld's rule: it never
makes a link lld accepts resolve differently, because both search the
command line's `-L` first.

### 2.7 OUTPUT, ENTRY, EXTERN

```
OUTPUT ( word )     ENTRY ( word )     EXTERN ( symbols )
symbols = symbol *( [","] symbol )
```

- `OUTPUT` and `ENTRY` read their argument in `Script` mode, so `ENTRY(a=b)`
  is the symbol `a=b` (as GNU ld and lld) and `ENTRY(a-b)` the symbol `a-b`.
- `EXTERN` reads its list in `Expression` mode: names (§1.4 `Expression`
  set) and strings, separated by whitespace or one comma; empty lists,
  leading, doubled and trailing commas are errors (GNU ld: syntax errors).
  `Expression` keywords and the integers of §1.5 (`BX`, `ah`) are errors
  with a note to quote them.
- Consumer: `OUTPUT` is `-o` unless `-o` is given ("the command-line option
  takes precedence"). `EXTERN` is `-u` for each symbol. For `ENTRY` given
  more than once, GNU ld takes the last and lld the first: the consumer
  MUST make two different `ENTRY` symbols in the scripts of one link an
  error (`-e` on the command line overrides both and is not a conflict).

### 2.8 INCLUDE

`INCLUDE word`, no parentheses (`INCLUDE(x)` is a syntax error in GNU ld),
`Script` mode; a `;` after it is the empty statement. The AST keeps the
node (`CommandKind::Include`); `mortar_script` opens no file. The consumer
expands it by parsing the named file as a complete statement list (a list
cannot span files: `INPUT ( zz.o` in the included file with `)` in the
includer is a syntax error in GNU ld) and splicing its commands in place.
The file is searched in the current directory, then the library search
path (manual; measured: not the including script's directory, in GNU ld
and lld). Nesting is limited to 10 levels (manual). A missing file is an
error at the `INCLUDE` (GNU ld: `cannot open linker script file`). mold
does not implement `INCLUDE`.

### 2.9 Errors

The parse stops at the first error or refusal and returns it: one
diagnostic, placed at the offending byte (`Place::Script { path, line,
column }`, §1.1). A syntax error is kind `malformed-input`
(`Kind::Malformed { what }`): `what` is lower case, without a final period,
in the form `expected <what>, found <token>` or a statement of the defect
(`unterminated comment`, `',' inside an unquoted name`, `malformed number
'08': ...`); where the cause is a difference between linkers or a likely
typing error, a note says what to write instead. Tokens are shown as
`name 'x'`, `keyword 'X'`, `number '1'`, `string "x"`, `'('`, `end of
file` (names cut at 40 bytes).

Why `malformed-input` and no new kind: `spec/diag.md` §3.1 defines it as
"a reader rejected the bytes of a file", which is what a script syntax
error is, and the `Place::Script` form already tells a tool that the file
was read as a script. A tool that needs to tell script errors from object
errors uses the place form.

### 2.10 Refused commands

Every statement GNU ld 2.44 accepts at the top level and stage 1 does not
implement is refused, `Diagnostic::refused(RefusalClass::ScriptCommand,
name)`, placed at its first byte. The set was measured: each `Script`
keyword at statement start in the forms `K`, `K(x)`, `K { }`, `K(x=1)`,
`K x`, plus the manual's syntax for the rest; GNU ld accepts exactly these:

| name | GNU ld accepts, e.g. |
|---|---|
| `SECTIONS`, `MEMORY`, `PHDRS`, `VERSION` | `SECTIONS { }`, `MEMORY { ram : ORIGIN = 0, LENGTH = 1M }` |
| `PROVIDE`, `PROVIDE_HIDDEN`, `HIDDEN` | `PROVIDE(foo = 1);` |
| `ASSERT` | `ASSERT(1, "m");` |
| `INSERT` | `INSERT AFTER .text;` |
| `NOCROSSREFS`, `NOCROSSREFS_TO` | `NOCROSSREFS(.text .data);` |
| `REGION_ALIAS` | `REGION_ALIAS("r", ram);` |
| `LD_FEATURE` | `LD_FEATURE("SANE_EXPR")` |
| `FORCE_COMMON_ALLOCATION`, `INHIBIT_COMMON_ALLOCATION`, `FORCE_GROUP_ALLOCATION` | the keyword alone |
| `HLL`, `SYSLIB`, `MAP` | `HLL(a)` (GNU ld: `HLL ignored`), `SYSLIB(a)`, `MAP(x)` |
| `FLOAT`, `NOFLOAT` | the keyword alone |
| `symbol assignment` | a name or string followed by `= += -= *= /= <<= >>= &= |= ^=`: `foo = 1;`, `. = 1;`, `"x" = 1;`, `a-b = 1;` (manual, "Assigning Values to Symbols") |
| `LIB` | not GNU ld 2.44 (syntax error there); the current manual's `LIB ( files )`: refused so that a script written for a newer GNU ld is told the command, not a syntax error |

The refusal is decided by the head of the statement (the keyword, or the
name and the operator): stage 1 cannot read the rest, so `x = 1` without
its `;`, `HLL` without its parentheses and `SECTIONS {` without its `}` are
refused although GNU ld rejects them. Either way the link stops. Keywords
of §1.7 that start no command in GNU ld (`ALIGN`, `len`, `OVERLAY`, ...)
are syntax errors (§2.2), as there.

`CommandKind` is deliberately not `#[non_exhaustive]`: a consumer matches
every command and refuses, by name, what it does not do; a command a later
stage adds breaks the build of every consumer instead of being skipped by a
`_` arm.

## 3. Recognizing a script

How a linker decides that an input file is a script and not an object, an
archive or a shared object.

### 3.1 GNU ld

Every input that BFD does not recognize as an object or archive is tried as
a script (the note `file format not recognized; treating as linker script`
is printed only when that fails). In such an *implicit* script an invalid
character in SCRIPT or EXPRESSION state ends the attempt with `<file>: file
not recognized: file format not recognized` (this is how a binary file
fails); a grammar error is `<file>:<line>: syntax error`. The empty file is
an empty script (exit 0). A UTF-8 byte-order mark is an invalid character.

### 3.2 lld

Any file whose magic `identify_magic` does not know is read as a script:
binary bytes give a script error (`\0\1\2\3` is `unknown directive`), the
empty file is an empty script, a form feed is whitespace.

### 3.3 mold

A file is a script when it is at least 4 bytes long and each of its first
4 bytes is printable ASCII or tab or LF (measured: `   ;` and `;\n\n\n`
are scripts; the empty file, `;`, `\r\n\r\n;`, `\v   ;` and `/* é */` are
`unknown file type`). The rest of the file is then lexed.

### 3.4 mortar

`looks_like_script(bytes)` is false for the formats with readers of their
own (`\x7fELF`, `!<arch>\n`, `!<thin>\n`) and for any file containing a NUL
or a control byte other than tab, LF and CR (0x00-0x08, 0x0b, 0x0c,
0x0e-0x1f, 0x7f); true otherwise, the empty file included. `mortar_input`
sniffs ELF and archives first, then asks this, and reports a false answer
as `unknown-file-type`; a true answer goes to `parse`, whose diagnostic
says what is wrong and where.

Reasons: the question is answered from the bytes alone and in one pass
(what is not text is not a script, so LLVM bitcode, other object formats
and random binaries are `unknown file type` rather than a confusing
script error at some byte); bytes above 0x7f are allowed because a comment
may hold UTF-8, and the parser then rejects them outside comments and
strings; the empty file and short files are scripts as in GNU ld and lld
(mold's 4-byte rule rejects `;`, which the other two accept). Form feed and
vertical tab, which GNU ld rejects and lld accepts, are not script text.

## 4. API

```rust
pub fn parse(source: &[u8], path: &str) -> Result<Script, Box<Diagnostic>>;
pub fn looks_like_script(bytes: &[u8]) -> bool;
pub fn position(source: &[u8], offset: usize) -> (u32, u32);   // line, column
pub const MAX_AS_NEEDED_DEPTH: usize;                          // 16
pub const ASSIGNMENT: &str;                                     // "symbol assignment"

pub struct Span { pub start: usize, pub end: usize }           // fn of(&self, &[u8]) -> &[u8]
pub struct Word { pub text: Vec<u8>, pub quoted: bool, pub span: Span }
pub struct Script { pub commands: Vec<Command> }
pub struct Command { pub kind: CommandKind, pub span: Span }
pub enum CommandKind {
    Input(Vec<InputItem>), Group(Vec<InputItem>),
    OutputFormat { default: Word, big: Option<Word>, little: Option<Word> },
    OutputArch(Word), Target(Word), SearchDir(Word), Output(Word), Entry(Word),
    Extern(Vec<Word>), Startup(Word), Include(Word),
}
pub enum InputItem { File(InputFile), AsNeeded { items: Vec<InputItem>, span: Span } }
pub struct InputFile { pub form: FileForm, pub name: Vec<u8>, pub spelling: Word }
pub enum FileForm { Path, Sysroot, Library, LibraryVerbatim }

pub mod lex {
    pub enum Mode { Script, InputList, Expression }
    pub struct Lexer<'a>;   // new(&[u8]), next(Mode) -> Result<Token, LexError>, offset()
    pub struct Token { pub kind: TokenKind, pub span: Span }
    pub enum TokenKind { Name, String, Integer(u64), Keyword(Keyword), Punct(Punct), Library, Eof }
    pub enum Keyword { /* §1.7 */ }   pub enum Punct { /* §1.8 */ }
    pub struct LexError { pub offset: usize, pub what: String, pub notes: Vec<String> }
    pub fn parse_integer(text: &[u8]) -> Result<u64, String>;
}
```

- **`&[u8]`, not `&str`.** A script is a file's bytes. Requiring UTF-8 would
  reject scripts GNU ld and lld accept (a Latin-1 comment) and lose file
  names that are not UTF-8; the language is ASCII, so nothing is gained
  by decoding. `Word::as_str` and `Word::to_string_lossy` serve callers
  that want text.
- **`Box<Diagnostic>`**: one located diagnostic (§2.9), boxed because it is
  large next to the `Ok` value. `path` is the script's name as the user, a
  search directory or another script gave it (`spec/diag.md` §2.3).
- The AST is plain data, `Debug + Clone + PartialEq + Eq + Hash`, and owns
  its bytes (no lifetime): it outlives the buffer it was read from.
- **No panic on any bytes.** Non-UTF-8, NUL, unterminated comments and
  strings, absurd nesting (bounded by `MAX_AS_NEEDED_DEPTH`; nothing else
  in stage 1 recurses), huge inputs: an `Err`, never a panic, never
  unbounded recursion. Lexing is linear in the input.
- Every `Err` carries a `Place::Script`.

## 5. Survey and kept disagreements

### 5.1 Method

`tests/differential.rs` hands each script of a table to `ld.bfd`, and where
a column is recorded to `ld.lld` and `mold`, as an input of a minimal link
whose named files all exist. GNU ld "accepts" unless it says `syntax
error`, `file not recognized`, `EOF in comment` or `ignoring invalid
character`; lld and mold are judged by exit status. The rows are classified:
valid (both accept), refused (mortar refuses by name, GNU ld MUST accept:
asserted, so a refusal is never a syntax error in disguise), refused by the
head (§2.10), refused-newer (`LIB`), invalid (both reject), and kept
disagreements (mortar rejects, GNU ld accepts).

### 5.2 Kept disagreements (mortar rejects, GNU ld accepts)

| script | GNU ld 2.44 | lld | mold | why mortar rejects |
|---|---|---|---|---|
| `INPUT(a.o,b.o)`, `INPUT(a.o, b.o)` | one file `a.o,b.o`; `a.o,` and `b.o` (contradicts the manual's `INPUT(file, file, ...)`) | looks for a file `,` | the same | the three disagree; a name with a swallowed comma is never what was meant |
| `ENTRY(a,b)` | symbol `a,b` | `) expected` | error | as above |
| `OUTPUT_FORMAT(x,y,z)` | one format name | three names | ignored | as above |
| `INPUT(1.o)` | skips `1`, looks for `.o` | `1.o` | `1.o` | GNU ld silently misreads; quoting gives one meaning everywhere |
| `INPUT(a.o;b.o)`, `INPUT(-x)`, `INPUT(= a.o)`, `INPUT(a.o é)`, `INPUT(a.o "b.o)`, `INPUT(a.o/*x*/b.o)` | skips the byte (echoed to standard output) | various | various | silent dropping of bytes (§1.9) |
| `INPUT(a.o # c.o` LF `)` | `#` skipped, `c.o` a file | comment | comment | the meaning differs |
| `INPUT(-l=foo)` | `-l<sysroot>foo` | library `=foo` | the same | the meaning differs, GNU ld's is unusable |
| `INPUT("-l:b.o")` | the file `-l:b.o` | the library `b.o` | the same | the meaning differs |
| `INPUT("")`, `ENTRY("")` | accepted (finds nothing, sets nothing) | error / accepted | error | an empty name is never meant |

Also differing from GNU ld, not in the table because both reject: GNU ld
reports `syntax error` with a line (line 0 at end of file) and "file not
recognized" for invalid characters; mortar reports line, column and what
is wrong. Under `-T` GNU ld ignores invalid characters (exit 0); mortar
(when `-T` is implemented, through the same parser) does not.

### 5.3 Where lld and mold differ from GNU ld (recorded, not mortar's problem)

- lld: no nested `AS_NEEDED`; accepts `INPUT()`, `ENTRY(123)`, `len = 1;`,
  `x == 1;`; rejects `STARTUP`, `INSERT` (as a whole command outside
  `SECTIONS`), `LD_FEATURE`, `HLL`, `SYSLIB`, `MAP`, `FLOAT`, `NOFLOAT`,
  the `*_ALLOCATION` commands, and `x = 1,` (comma terminator); `ENTRY`
  twice: the first wins.
- mold reads only `OUTPUT_FORMAT`, `INPUT`, `GROUP`, `AS_NEEDED` and
  `VERSION` in a script given as an input; every other command is
  `unknown linker script token`. It crashed (SIGSEGV) on
  `VERSION{V1{};}`.

## 6. Corpus (L1)

`tests/corpus.rs` finds the text libraries of the machine by content, not
by name: every `lib*.so` and `lib*.a` under 64 KiB in `/usr/lib`,
`/usr/lib64`, `/usr/lib32`, `/usr/lib/<triple>`, `/usr/<triple>/lib*`,
`/usr/lib/gcc/<triple>/<version>` and `/usr/lib/gcc-cross/<triple>/<version>`
for which `looks_like_script` holds. Each MUST parse. On the reference
machine (Debian, glibc 2.41, GCC 14, cross sysroots for x86-64 and i686)
there are ten, and their structure is asserted:

| file | content |
|---|---|
| `/usr/lib/aarch64-linux-gnu/libc.so` | `OUTPUT_FORMAT(elf64-littleaarch64)`, `GROUP ( /lib/aarch64-linux-gnu/libc.so.6 /usr/lib/aarch64-linux-gnu/libc_nonshared.a AS_NEEDED ( /lib/ld-linux-aarch64.so.1 ) )` |
| `/usr/lib/aarch64-linux-gnu/libm.so` | `OUTPUT_FORMAT(elf64-littleaarch64)`, `GROUP ( /lib/aarch64-linux-gnu/libm.so.6 AS_NEEDED ( /lib/aarch64-linux-gnu/libmvec.so.1 ) )` |
| `/usr/lib/aarch64-linux-gnu/libm.a` | `OUTPUT_FORMAT(elf64-littleaarch64)`, `GROUP ( .../libm-2.41.a .../libmvec.a )` |
| `/usr/lib/gcc/aarch64-linux-gnu/14/libgcc_s.so` | `GROUP ( libgcc_s.so.1 -lgcc )` |
| `/usr/x86_64-linux-gnu/lib/libc.so` | `OUTPUT_FORMAT(elf64-x86-64)`, `GROUP ( .../libc.so.6 .../libc_nonshared.a AS_NEEDED ( /usr/x86_64-linux-gnu/lib64/ld-linux-x86-64.so.2 ) )` |
| `/usr/x86_64-linux-gnu/lib/libm.so`, `libm.a` | as AArch64's, `elf64-x86-64` |
| `/usr/i686-linux-gnu/lib/libc.so` | `OUTPUT_FORMAT(elf32-i386)`, `GROUP ( .../libc.so.6 .../libc_nonshared.a AS_NEEDED ( /usr/i686-linux-gnu/lib/ld-linux.so.2 ) )` |
| `/usr/lib/gcc-cross/{x86_64,i686}-linux-gnu/14/libgcc_s.so` | `GROUP ( libgcc_s.so.1 -lgcc )` |

Each starts with a `/* GNU ld script ... */` comment. There is no text
`libstdc++.so` or `libc++.so` on the machine (libc++'s, where it is one, is
`INPUT(libc++.so.1 -lc++abi)`: a later corpus entry). A known file that is
missing fails the test under `MORTAR_REQUIRE_TOOLS=1` and is a note
otherwise (a machine without a cross sysroot).

The second corpus is GNU ld's own default scripts (`<libdir>/ldscripts/*`,
655 files here): each MUST give `Ok` or a refusal, never a syntax error.
652 are refused at `SECTIONS` after their `OUTPUT_FORMAT` (three names),
`OUTPUT_ARCH`, `ENTRY` and `SEARCH_DIR("=...");` parse; 3 parse whole.

The hostile-input tests (`tests/hostile.rs`): every prefix of each corpus
file, 20000 mutations from a fixed-seed xorshift generator (byte changes,
deletions, inserted fragments that switch modes, splices), every single
byte alone and inside `INPUT(...)` and `EXTERN(...)`, 200000 nested
`AS_NEEDED(`, a megabyte of `;`, of a name, of an unterminated comment and
string: `Ok` or a placed `Err`, never a panic.

## 7. Tests

Files under `crates/mortar_script/`.

| section | test |
|---|---|
| §1.1 | `tests/lex.rs` `offsets_lines_and_columns`, `eof_repeats` |
| §1.2 | `tests/lex.rs` `mode_boundaries` |
| §1.3 | `tests/lex.rs` `whitespace`, `block_comments`, `hash_comments` |
| §1.4 | `tests/lex.rs` `script_names`, `input_list_names`, `expression_names` |
| §1.5 | `tests/lex.rs` `integer_values`, `integer_errors`, `integer_tokens_by_mode` |
| §1.6 | `tests/lex.rs` `strings` |
| §1.7 | `tests/lex.rs` `keywords_by_mode`; `src/lex.rs` `keyword_table_is_sorted` |
| §1.8 | `tests/lex.rs` `script_operators`, `expression_operators` |
| §1.9 | `tests/lex.rs` `invalid_bytes`; `tests/parse.rs` `file_form_errors` |
| §2.1 | `tests/parse.rs` `output_entry_extern`, `formats_and_architectures`, `file_form_errors` |
| §2.2 | `tests/parse.rs` `statements_and_empty_statements`, `syntax_errors_are_located` |
| §2.3 | `tests/parse.rs` `input_and_group`, `startup`, `input_list_errors` |
| §2.4 | `tests/parse.rs` `file_forms`, `file_form_errors` (resolution: `spec/input.md`'s tests) |
| §2.5 | `tests/parse.rs` `formats_and_architectures` |
| §2.6 | `tests/parse.rs` `search_dir` |
| §2.7 | `tests/parse.rs` `output_entry_extern` |
| §2.8 | `tests/parse.rs` `include` |
| §2.9 | `tests/parse.rs` `syntax_errors_are_located` and every `malformed(..)` call (exact line and column) |
| §2.10 | `tests/parse.rs` `refused_commands`; `tests/differential.rs` (GNU ld accepts each) |
| §3.4 | `tests/recognize.rs` |
| §4 | `tests/parse.rs` `ast_is_plain_data`; `tests/hostile.rs`; the doctest of `src/lib.rs` |
| §5 | `tests/differential.rs` `acceptance_matches_the_reference_linkers`, `system_text_library_links_with_gnu_ld` |
| §6 | `tests/corpus.rs` `system_text_libraries`, `gnu_default_scripts_refuse_never_fail`; `tests/hostile.rs` |

## 8. Open questions and later stages

- The byte sets of the wildcard and version modes (§1.10) are unmeasured.
- The `Expression` integer rule is pinned for letter-initial names (hex
  digits + `h H x X`); its interaction with names containing `$` and `.`
  in full expressions is the expression stage's to sweep.
- Sysroot containment (§2.4 rule 1) through symbolic links is unmeasured;
  mortar compares resolved absolute paths.
- `-T` scripts, `--defsym`, `-c` (MRI) and `--version-script` use this
  crate later; their differences from implicit scripts (§3.1: invalid
  characters ignored under `-T`) are `spec/cli.md`'s to state.
- libc++'s text `libc++.so` (`INPUT(libc++.so.1 -lc++abi)` on some
  distributions) joins the corpus when the machine has one.
