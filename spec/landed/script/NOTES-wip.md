# WP-009 handover notes (work in progress; delete when WP-009 lands)

State at handover: reading and black-box experiments only. No code, no
`spec/script.md`, no tests written yet. Everything below was observed on
this machine with GNU ld 2.44 (`ld.bfd`, AArch64) unless marked
"unverified". lld and mold were NOT yet probed.

Method: a script handed as an input (`ld.bfd -o out s.ld e.o`, `e.o` a
one-`ret` object) and the stderr text read: `cannot find <name>` shows the
file-name token, `cannot find entry symbol <name>` the ENTRY token, `nm out`
the value of `x = <number>;`. `syntax error` = token recognised but
grammar rejects; `file not recognized` = invalid character in an implicit
script (see "Recognizing"). The manual was read from
sourceware.org/binutils/docs/ld/ (latest release, not 2.44: it documents
`LIB(...)`, which 2.44 does not have as a keyword). ld's source was
deliberately not read (licence: spec from manual + experiment).

## Corpus (text files a linker meets; found by content, not ELF/`!<arch>`/`!<thin>`)

- `/usr/lib/aarch64-linux-gnu/libc.so`: `OUTPUT_FORMAT(elf64-littleaarch64)`
  `GROUP ( /lib/aarch64-linux-gnu/libc.so.6 /usr/lib/aarch64-linux-gnu/libc_nonshared.a  AS_NEEDED ( /lib/ld-linux-aarch64.so.1 ) )`
- `/usr/lib/aarch64-linux-gnu/libm.so`: `OUTPUT_FORMAT(elf64-littleaarch64)`
  `GROUP ( /lib/aarch64-linux-gnu/libm.so.6  AS_NEEDED ( /lib/aarch64-linux-gnu/libmvec.so.1 ) )`
- `/usr/lib/aarch64-linux-gnu/libm.a`: `OUTPUT_FORMAT(elf64-littleaarch64)`
  `GROUP ( /usr/lib/aarch64-linux-gnu/libm-2.41.a /usr/lib/aarch64-linux-gnu/libmvec.a )`
- `/usr/lib/gcc/aarch64-linux-gnu/14/libgcc_s.so`,
  `/usr/lib/gcc-cross/x86_64-linux-gnu/14/libgcc_s.so`,
  `/usr/lib/gcc-cross/i686-linux-gnu/14/libgcc_s.so`: `GROUP ( libgcc_s.so.1 -lgcc )`
- `/usr/x86_64-linux-gnu/lib/libc.so`: `OUTPUT_FORMAT(elf64-x86-64)`
  `GROUP ( /usr/x86_64-linux-gnu/lib/libc.so.6 /usr/x86_64-linux-gnu/lib/libc_nonshared.a  AS_NEEDED ( /usr/x86_64-linux-gnu/lib64/ld-linux-x86-64.so.2 ) )`
- `/usr/x86_64-linux-gnu/lib/libm.so`: `OUTPUT_FORMAT(elf64-x86-64)`
  `GROUP ( /usr/x86_64-linux-gnu/lib/libm.so.6  AS_NEEDED ( /usr/x86_64-linux-gnu/lib/libmvec.so.1 ) )`
- `/usr/x86_64-linux-gnu/lib/libm.a`: `OUTPUT_FORMAT(elf64-x86-64)`
  `GROUP ( /usr/x86_64-linux-gnu/lib/libm-2.41.a /usr/x86_64-linux-gnu/lib/libmvec.a )`
- `/usr/i686-linux-gnu/lib/libc.so`: `OUTPUT_FORMAT(elf32-i386)`
  `GROUP ( /usr/i686-linux-gnu/lib/libc.so.6 /usr/i686-linux-gnu/lib/libc_nonshared.a  AS_NEEDED ( /usr/i686-linux-gnu/lib/ld-linux.so.2 ) )`
  (i686 has no text `libm.so`/`libm.a`.)
- Every one starts with a `/* GNU ld script ... */` comment.
- Not present: `libstdc++.so` and `libc++.so` as text (none under
  `/usr/lib/llvm-*/lib`). The brief's `/usr/lib/gcc/...` cross directories
  are really `/usr/lib/gcc-cross/<triple>/14`.
- Second corpus worth using: `/usr/lib/{aarch64,x86_64,i386}-linux-gnu/ldscripts/*`
  (ld's built-in `SECTIONS` scripts, several hundred): each must give
  `Refused SECTIONS` after `OUTPUT_FORMAT(a, b, c)`, `OUTPUT_ARCH`, `ENTRY`,
  `SEARCH_DIR("=/...");` parse, never a syntax error.

## Lexer: ld is modal; three states matter for stage 1

Whitespace in all states: space, tab, CR, LF. FF and VT are invalid
characters. `/* */` comments in all states, not nested; unterminated is a
fatal `EOF in comment`. `//` is not a comment (lexes as a name `//`).
Quoted string `"[^"]*"` in all states: no escapes (`"a\"` is the name `a\`),
may hold newlines, may be empty; quoting defeats keywords (`ENTRY("ENTRY")`).

**SCRIPT** (top level; arguments of `ENTRY`, `OUTPUT`, `SEARCH_DIR`,
`OUTPUT_FORMAT`, `TARGET`, `INCLUDE` name):
- name: first `[A-Za-z_/.\\$~]`, then `[A-Za-z0-9_/.\\$~+:\[\],=-]*`. Commas
  and `=` are NAME characters: `ENTRY(a,b)` is the symbol `a,b`; `x=1;` and
  `x= 1;` are syntax errors (one name `x=1` / `x=`); `x =1;` and `x = 1;`
  are assignments.
- `#` to end of line is a comment here (and in EXPRESSION), NOT in INPUTLIST.
- integers are tokens: digits, `0x..`, `$hex` (`ENTRY(123)`, `ENTRY($ab)`,
  `ENTRY($10)` are syntax errors; `$zz` is a name: longest match, integer
  wins ties).
- keywords (found by `ENTRY(W)` giving a syntax error over every upper-case
  word of the manual plus a hand list): ABSOLUTE ADDR AFTER ALIGN ALIGNOF
  ALIGN_WITH_INPUT ASSERT AT BEFORE BIND BLOCK CONSTANT DATA_SEGMENT_ALIGN
  DATA_SEGMENT_END DATA_SEGMENT_RELRO_END DEFINED ENTRY EXTERN FLOAT
  FORCE_COMMON_ALLOCATION FORCE_GROUP_ALLOCATION GROUP HIDDEN HLL INCLUDE
  INHIBIT_COMMON_ALLOCATION INPUT INSERT LD_FEATURE LENGTH LOADADDR LOG2CEIL
  MAP MAX MEMORY MIN NEXT NOCROSSREFS NOCROSSREFS_TO NOFLOAT ONLY_IF_RO
  ONLY_IF_RW ORIGIN OUTPUT OUTPUT_ARCH OUTPUT_FORMAT OVERLAY PHDRS PROVIDE
  PROVIDE_HIDDEN REGION_ALIAS SEARCH_DIR SECTIONS SEGMENT_START SIZEOF
  SIZEOF_HEADERS SPECIAL STARTUP SUBALIGN SYSLIB TARGET VERSION and the
  lower-case `l` `len` `o` `org` (so `len = 4;` is a syntax error,
  `lenx = 4;` an assignment). Case-sensitive (`entry` is a name). NOT
  keywords here: AS_NEEDED, LIB, COPY, DSECT, INFO, NOLOAD, READONLY, TYPE,
  KEEP, FILL, SORT*, `sizeof_headers`.
- single characters after `ENTRY(a) `: `! > , ) } ; = ( { :` and digits are
  tokens (syntax error); `@ % ? * & | <`, a lone `"`, control characters,
  bytes >= 0x80 are invalid characters. A full per-byte sweep was planned,
  not done.

**INPUTLIST** (inside `INPUT(`/`GROUP(`, and nested `AS_NEEDED(`):
- only keyword: `AS_NEEDED` (`INPUT(INCLUDE x)`, `INPUT(GROUP)` are files).
- name: same character sets as SCRIPT; no integer tokens (`$abc` is a file).
- `=` + name: one token, spelling kept (`=/x.so`, `=x.so`); `= x.so` is not.
- `-l` + one or more name characters: library token (`-lfoo`, `-l:bar.a`,
  `-l=x` which ld reports as `-lx`: the sysroot rule applied to the `-l`
  name). `-l foo`, `-L/x`, `-` are not tokens.
- commas: a comma glued to a name is PART OF THE NAME: `INPUT(a.o,b.o)`
  looks for one file `a.o,b.o`; `INPUT(a, b)` looks for `a,` and `b`;
  `INPUT(a , b)` and `INPUT(a ,b)` are two files. This contradicts the
  manual's `INPUT(file, file, ...)`. Leading comma, `,,` standing alone:
  syntax error.
- **ld bug class**: any character with no rule in this state is echoed to
  stdout and skipped, silently: `INPUT(1.o)` looks for `.o` (a name cannot
  start with a digit), `INPUT(a.o;b.o)`, `a@b`, `a*b`, `a?b`, `[x`, `+x`,
  `:x`, `# c`, NUL, bytes >= 0x80, an unterminated `"`, `-` not followed by
  `l`: all "accepted". `INPUT(a/*c*/b)` is `a/`, `c`, `/b`.
- grammar: list is non-empty (`INPUT()`, `AS_NEEDED()` syntax errors);
  entries separated by whitespace and/or one comma; `AS_NEEDED` nests in
  itself (`AS_NEEDED(AS_NEEDED(x.so))` accepted); EOF inside a list is
  `s.ld:0: syntax error` (line 0).

**EXPRESSION** (the list of `EXTERN(`; all of later stages' expressions):
- keywords: ABSOLUTE ADDR ALIGN ALIGNOF ALIGN_WITH_INPUT ASSERT AT BIND
  BLOCK CONSTANT COPY DATA_SEGMENT_ALIGN DATA_SEGMENT_END
  DATA_SEGMENT_RELRO_END DEFINED DSECT HIDDEN INCLUDE INFO LENGTH LOADADDR
  LOG2CEIL MAX MIN NEXT NOCROSSREFS NOCROSSREFS_TO NOLOAD ONLY_IF_RO
  ONLY_IF_RW ORIGIN OVERLAY PROVIDE PROVIDE_HIDDEN READONLY SEGMENT_START
  SIZEOF SIZEOF_HEADERS SPECIAL SUBALIGN TYPE. (ENTRY, INPUT, GROUP, ... are
  plain names: `EXTERN(ENTRY)` is accepted.)
- **trap**: hex digits followed by one of `hHxXbBoOdD` are an INTEGER even
  without a leading digit: `EXTERN(BX)`, `EXTERN(C6X)` are syntax errors,
  `x = abh;` is 0xab. Yet `x = add;`/`bad`/`feed`/`dad` gave "undefined
  symbol", i.e. a NAME: the exact rule (suffix letter set vs trailing `d`)
  was NOT pinned down; probe `EXTERN(add)`, `EXTERN(ab)`, `EXTERN(abh)`,
  `EXTERN(ffx)` before specifying.
- name character set in this state: unverified (manual, "Symbol Names":
  letter, underscore or period, then letters, digits, underscores, periods,
  hyphens; expected to be narrower than SCRIPT's: no `, = + : [ ]`).

## Numbers (`x = N;`, value from `nm`)

- decimal `10`; octal `010`=8; `0x10`/`0X10`=16; `$10`/`$ff`=hex (`$fg` a name).
- `K`/`k` x1024, `M`/`m` x1048576, also on hex and octal (`0x10K`=0x4000,
  `010K`=0x2000, `$10K`=0x4000). No `G`. No double suffix (`1KK`, `1MK`).
- suffix bases: `10h`/`10H`/`10x`/`10X`=16, `10o`/`10O`=8, `10b`/`10B`=2,
  `10d`/`10D`=10; `ffh`, `0ffh`, `FFH`, `abh` = hex. Not combinable with
  `0x` (`0x10h` error) or K/M (`10bK` error).
- a digit not of the base ends the scan SILENTLY: `08`=0, `019`=1,
  `0778`=0x3f, `08K`=0; but `18o`, `12b` are syntax errors. `0x` alone = 0.
- overflow saturates silently: `18446744073709551616` and
  `0x1ffffffffffffffff` = 0xffffffffffffffff; the K/M multiply wraps
  (`17592186044416M` = 0).
- not numbers: `0b101`, `0o17`, `1_000`, `1e3`, `1.5`, `10KB`, `1a`, `0x1g`.
- mortar should refuse the silent cases (digit outside the base, overflow)
  rather than copy them; that was the intended decision, not yet written.

## Top-level statements

- `;` empty statement, any number (`ENTRY(a);;;ENTRY(b);`).
- assignment: NAME or quoted name, then `=` `+=` `-=` `*=` `/=` `<<=` `>>=`
  `&=` `|=` `^=` (NOT `%=`: `%` is an invalid character), expression, then
  `;` OR `,` (`x = 1,` accepted); no separator at EOF is a syntax error.
  `. = 1;`, `"x" = 1;`, `a-b = 1;` accepted.
- `ENTRY()`, `ENTRY(a b)`, `ENTRY a`, `ENTRY(a, b)` syntax errors.
- `INCLUDE name` (no parentheses; `INCLUDE(x)` is a syntax error; optional
  `;` after). The included file must be a complete statement list:
  `INPUT ( zz.o` in the file with `)` in the parent is a syntax error, so
  top-level INCLUDE can be an AST node the caller expands. A missing file is
  `cannot open linker script file`.
- ld reports only a line, and line 0 at EOF; mortar gives line:column.

## Recognizing a script (GNU ld side only)

- ld tries every unrecognized input as a script (warning-free note
  `file format not recognized; treating as linker script`). In such an
  implicit script an invalid character in SCRIPT/EXPRESSION state is fatal
  `<file>: file not recognized: file format not recognized` (that is the
  "this was not a script after all" path); a grammar error is
  `<file>:<line>: syntax error`.
- with `-T` the same invalid character is only
  `ignoring invalid character '@' in script` and the link SUCCEEDS (exit 0).
  mortar should reject in both cases (kept disagreement).
- lld's and mold's rules: not yet looked at.

## Decisions intended (not yet written into a spec; the next agent may revisit)

1. Lexer API `next(mode)`, modes Script / InputList / Expression now;
   WILD and the three version-script states specified, not implemented.
2. Implement to AST: INPUT, GROUP, AS_NEEDED, OUTPUT_FORMAT (1 and 3 args),
   OUTPUT_ARCH, SEARCH_DIR, OUTPUT, ENTRY, EXTERN, TARGET, STARTUP, top-level
   INCLUDE (node only), `;`. `Command` not `#[non_exhaustive]`, so the
   consumer must match every command and refuse what it does not do.
3. Refuse by name, at the keyword: MEMORY SECTIONS PHDRS VERSION HLL SYSLIB
   FLOAT NOFLOAT FORCE_COMMON_ALLOCATION FORCE_GROUP_ALLOCATION
   INHIBIT_COMMON_ALLOCATION MAP NOCROSSREFS NOCROSSREFS_TO INSERT
   REGION_ALIAS LD_FEATURE ASSERT PROVIDE PROVIDE_HIDDEN HIDDEN; a name
   followed by an assignment operator as `symbol assignment`; `LIB (` (manual
   only, not 2.44). Other SCRIPT keywords at statement start (ALIGN, `len`,
   AFTER, ...) are syntax errors, as in ld. Parsing stops at the first
   refusal.
4. Where ld silently drops or misreads (the INPUTLIST echo class, a comma
   glued to a file name, which lld is expected to split as two files): a
   located syntax error with a note saying to quote or space, recorded as a
   kept disagreement in the differential table.
5. `-l=x` / `-l$SYSROOT...`: AST keeps the spelling; what the resolver does
   was undecided.

## Not done

lld/mold probes; per-byte character sweeps for SCRIPT and EXPRESSION;
sysroot and search-order experiments (`--sysroot`, script inside the
sysroot, script's own directory, `-L`); `OUTPUT_FORMAT` argument and
three-argument probes; `SEARCH_DIR`/`OUTPUT`/`TARGET`/`STARTUP`/`OUTPUT_ARCH`
probes; valid examples of each refused command against ld.
