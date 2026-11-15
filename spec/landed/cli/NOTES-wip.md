# WP-008 work-in-progress notes (handover)

Not a spec. Findings and decisions of the first session, for whoever
continues. Delete this file when `spec/cli.md` and `WP-008.md` carry what
matters. **No code, no spec text and no test was written yet**: the session
read the documents and the three linkers' `--help`, and ran the experiments
below.

"Verified" means a command was run on this machine (GNU ld 2.44 AArch64
native `ld.bfd`, lld 22.1.8 `ld.lld-22`, mold 2.37.1). "Recalled" means
from memory of glibc's `getopt` and ld's `lexsup.c`, **not checked**: test
before writing it into the spec.

## Environment facts (verified)

- No man or info pages are installed (`/usr/share/man/man1` has no `ld*`):
  the "manual" is not readable locally. Sources of truth are `--help` and
  experiments.
- Native `ld.bfd` knows only AArch64/ARM emulations. x86 options and
  emulations: `x86_64-linux-gnu-ld.bfd` (default `elf_x86_64`; `-m
  elf_i386` for i386) and `i686-linux-gnu-ld.bfd`.
- `ld.bfd --help`: generic options, then "ELF emulations:" (`--build-id`,
  `--hash-style`, `--eh-frame-hdr`, `--exclude-libs`, `-z ...`,
  `--enable-new-dtags`, `--audit`, `-P/--depaudit`, `-Bgroup`,
  `--rosegment`, `--package-metadata`, `--compress-debug-sections`), then
  per-emulation blocks. AArch64: `--no-enum-size-warning`,
  `--no-wchar-size-warning`, `--pic-veneer`, `--stub-group-size=N`,
  `--fix-cortex-a53-835769`, `--fix-cortex-a53-843419[=full|adr|adrp]`,
  `--no-apply-dynamic-relocs`, `-z force-bti`, `-z bti-report[=..]`,
  `-z pac-plt`, `-z gcs=..`, `-z gcs-report[=..]`,
  `-z gcs-report-dynamic=..`. x86 (cross ld): `--ld-generated-unwind-info`,
  `--no-ld-generated-unwind-info`, and `-z` keywords
  `noextern-protected-data`, `indirect-extern-access`,
  `noindirect-extern-access`, `dynamic-undefined-weak`,
  `nodynamic-undefined-weak`, `noreloc-overflow` (x86-64 only),
  `call-nop=PADDING`, `ibtplt`, `ibt`, `shstk`, `cet-report=`,
  `report-relative-reloc`, `x86-64-{baseline|v2|v3|v4}`,
  `isa-level-report=`, `lam-u48`, `lam-u48-report=`, `lam-u57`,
  `lam-u57-report=`, `lam-report=` (x86-64 only), `mark-plt`/`nomark-plt`
  (x86-64 only), `pack-relative-relocs`/`nopack-relative-relocs`. The x86
  ld's defaults differ in the help text (`-z separate-code` default,
  `--warn-textrel` default).
- lld's `--help` does not list its `-z` keywords (only `-z <option>`); they
  are in its man page, which is not installed. They must come from
  experiment (`ld.lld-22 -z <kw>` warns `unknown -z value` for an unknown
  one, status 0).
- `rustc` is on `PATH` only after `source scripts/env.sh`.

## GNU ld grammar (verified, `ld.bfd <args> a.o`)

| command line | result |
|---|---|
| `-o out`, `-oout`, `--output out`, `--output=out` | output `out` |
| `-omagic` | output file `magic` (`-o magic`) |
| `--omagic` | the OMAGIC option |
| `-output out` | `-o utput`, then `out` is an input (`cannot find out`) |
| `-e main`, `-emain`, `-entry main`, `-entry=main`, `--entry main`, `--entry=main` | all set the entry (so `-entry` is the long option, not `-e ntry`) |
| `-o` (last) | `unrecognized option '-o'`, status 1 |
| `--entry` (last) | `--entry: missing argument`, status 1 |
| `--output`, `--format`, `--rpath`, `--sysroot`, `--hash-style`, `--exclude-libs`, `--soname`, `--architecture`, `--mri-script` (last, no value) | `unrecognized option '--x'` (not "missing argument") |
| `--library`, `--script`, `--entry`, `-rpath` (last, no value) | `<as written>: missing argument` |
| `--frobnicate` | `unrecognized option '--frobnicate'` + `use the --help option for usage information`, status 1 |
| `-frobnicate` | parsed as `-f robnicate` (`-f may not be used without -shared`) |
| `--shared=x` | `unrecognized option '--shared=x'` |
| `-sx`, `-xs`, `-nx`, `-Nn`, `-dx`, `-sq`, `-vs` | accepted, `Warning: grouped short command line options are deprecated: -sx` (once per extra letter: `-sxn` and `-Enx` warn twice), status 0 |
| `-ss` | accepted, no warning |
| `-dn` | the long option (`-Bstatic`), no warning |
| `-sofoo`, `-sefoo`, `-sznow`, `-sysmain`, `-nostart`, `-non-start` | `Error: unable to disambiguate: -sofoo (did you mean --sofoo ?)`, status 1 |
| `-snfoo` | the warning, then the "unable to disambiguate" error |
| `-so foo`, `-sy`, `-no`, `-s-x`, `-s=x` | `unrecognized option '-so'` |
| `-sz now`, `-sl c` | `Error: unable to disambiguate: -sz ...` |
| `-` | an input file named `-` (`cannot find -`) |
| `a.o -- b.o`, `a.o -- -o out` | **everything after `--` is silently dropped** (links `a.o` alone to `a.out`, status 0) |
| `-- a.o` | `no input files` |
| `-v a.o` | prints the version line on stdout and links |

So: a missing value is reported as "missing argument" only when the
spelling has exactly the dash count of the option's help form (recalled:
ld compares `argv + control` with the name); otherwise as "unrecognized
option". mortar should say `MissingArgument` in every such case.

lld 22 and mold 2.37.1 on the same lines (verified): `--` is `unknown
argument '--'` / `unknown command line option: --`; `-sx` is unknown (no
grouping); `--entr=main` is unknown in both (no abbreviations; lld adds
`did you mean '--entry=main'`); `-output o2` and `-omagic` are `-o utput`
and `-o magic` in both; `-entry=main` sets the entry in lld, but mold reads
it as `-e ntry=main` (`entry symbol is not defined: ntry=main`).

## Recalled, to verify before specifying

- ld builds `shortopts` + `longopts` and calls `getopt_long_only` (glibc's,
  not libiberty's), then on `?` retries with `getopt_long(.., "-",
  really_longopts)`, where `really_longopts` holds the options that need
  exactly two dashes (the `--o*` ones: `--output`, `--omagic`,
  `--oformat`, `--orphan-handling`, `--out-implib`; find the full set by
  experiment: `-name` rejected or read as a short cluster while `--name`
  is accepted).
- glibc `getopt_long_only`: an exact long match wins; otherwise a unique
  prefix is accepted; in long-only mode *any* two prefix matches are
  ambiguous (even aliases of one option); a single-dash argument that
  matches no long option falls back to short options when its first letter
  is one. `-x` with one letter that is a short option is never tried as a
  long abbreviation.
- Consequences to test: `-Td` (unique prefix of `-Tdata`?) against `-T d`;
  `-Tte` ambiguous (`Ttext`, `Ttext-segment`); `--reloc` ambiguous if the
  hidden alias `--relocateable` still exists; `-li`/`-library` (ld
  special-cases `-l` so that `-lib...` is a library, recalled).
- A pre-pass scans all of argv for `-m`: any argument starting with `-m`
  is an emulation (`-mEMUL`, or `-m` + next argument), independent of
  context; `-m486` and `-mips*` are ignored names.
- `-G` not followed by a digit is turned into `--shared` before parsing.
- Value forms: `required_argument` takes `--o=v` or `--o v` (the next
  argument whatever it is); `optional_argument` only `--o=v`; a short
  option with a required value takes `-ov` or `-o v`. `-O` takes a
  required value in GNU ld (`-O2`, `-O 2`).
- Response files (libiberty `expandargv`/`buildargv`): whitespace
  separates; `'` and `"` quote; a backslash escapes the next character
  **also inside both kinds of quotes**; a file that cannot be opened leaves
  the literal `@file` argument (then an input named `@file`); a directory
  is an error; nesting is expanded, with a limit of 2000 expansions. LLVM's
  GNU tokenizer has the same quoting and also leaves a missing `@file`
  alone; mold errors on a missing response file. **None of this was run
  yet.**
- Hidden GNU long options (not in `--help`) exist and count for
  abbreviation ambiguity: `--add-needed`, `--no-add-needed`,
  `--relocateable`, `--dll-verbose`, `--warn-shared-textrel`, `-dc`,
  `-dp`, `-Qy`, `-qmagic`, `--no-pic-executable`? (unsure).

## Finding the complete GNU long-option set by experiment (half done)

Idea: every option name is a string in the linker binary. Take `strings
-n 1 /usr/bin/ld.bfd`, keep `[A-Za-z][A-Za-z0-9_+.-]*`, and ask the linker
about each candidate `N` with no inputs: `--N`, `--N=zzz`, `-N`,
`-N=zzz`. `N` is an option (or a unique abbreviation of one) when `--N` or
`--N=zzz` does not answer `unrecognized option`; arity follows from which
of the two is accepted; "needs two dashes" from `-N` being rejected or read
as a short cluster. Abbreviations are told from exact names afterwards
(longest first: a name extended by exactly one exact option is suspect;
test `--N` minus its last letter). Run it for the three configurations
(`ld.bfd`; `x86_64-linux-gnu-ld.bfd`; the same with `-m elf_i386`). Run it
in an empty scratch directory: some probes write `a.out` or `zzz`.

The first run used `--N` alone as the acceptance test and so missed every
option that requires a value (see the table above); the corrected run was
stopped at the handover and left no result. The script (about 4000 linker
runs per configuration, several minutes):

```python
import subprocess, sys, re, os, json
os.chdir("probe")   # an empty scratch directory
def strings(path):
    out = subprocess.run(["strings", "-n", "1", path], capture_output=True, text=True, errors="replace").stdout
    return {s.rstrip("=") for s in out.splitlines()
            if re.fullmatch(r"[A-Za-z][A-Za-z0-9_+.-]*", s.rstrip("=")) and len(s) <= 45}
def classify(cmd):
    r = subprocess.run(cmd, capture_output=True, text=True, errors="replace", timeout=10, stdin=subprocess.DEVNULL)
    o = r.stderr + r.stdout
    if "unrecognized option" in o: return "unrec"
    if "missing argument" in o: return "needarg"
    if "unable to disambiguate" in o: return "disamb"
    return "ok"
name, ld, pre = sys.argv[1], sys.argv[2], sys.argv[3:]
res = {}
for n in sorted(strings(ld)):
    two, eq = classify([ld, *pre, "--" + n]), classify([ld, *pre, "--" + n + "=zzz"])
    if two == "unrec" and eq == "unrec": continue
    res[n] = (two, eq, classify([ld, *pre, "-" + n]), classify([ld, *pre, "-" + n + "=zzz"]))
json.dump(res, open("../probe-%s.json" % name, "w"), indent=0, sort_keys=True)
```

The same probe, per table row and spelling, is the cheap differential test
the brief asks for (GNU rows against `ld.bfd`: not `unrecognized option`;
lld rows against `ld.lld-22`: not `unknown argument`; mold rows against
`mold`: not `unknown command line option`). One `ld.bfd` run with no
inputs takes about 1 ms.

## Decisions taken (none implemented)

- **Table row** = one semantic option: spellings (short letter; long name
  with "one or two dashes" or "exactly two"), arity (none / required /
  optional-joined-only), origin (GNU generic, GNU per emulation with the
  targets, lld, mold, mortar), help group, an `Action` (which decides the
  `Plan` field; the "lands in" column of the generated table is derived
  from it), status (`Supported`, `Noop(reason)`, `Refused`). `-z` keywords
  are rows too (`-z relro` refused is `Refused{Option, "-z relro"}`; a
  keyword in no table is `UnknownOption{"-z frob"}`, where GNU ld, lld and
  mold only warn).
- Enumerated values (`--hash-style`, `--build-id`, `--icf`,
  `--compress-debug-sections`, `--unresolved-symbols`, ...) carry a status
  per value, so that a supported option can refuse one value as
  `Refused{OptionValue, "--hash-style=sysv"}`; an unlisted value is
  `InvalidOptionValue`.
- **Abbreviations** are resolved over the GNU rows only (all three
  targets' emulation options together); lld-only and mold-only options
  must be spelled in full, as those linkers require. An ambiguous or
  unknown abbreviation is `UnknownOption` with a note naming the
  candidates. Consequence to record: an abbreviation that one GNU ld
  configuration accepts can be ambiguous to mortar (the union has more
  names).
- **Grouped short options** (`-sx`): planned to be rejected as
  `UnknownOption` with a note (GNU ld accepts with a deprecation warning,
  lld and mold reject). A kept disagreement; reconsider if "drop-in"
  should win (it would need a warning kind in `mortar_diag`, §3.3).
- **`--`**: planned to be rejected (`UnknownOption`), because GNU ld
  silently drops everything after it and lld and mold reject it.
- Emulation-specific options are recognized whatever `-m` says (the target
  is often unknown at parse time); whether one is refused for another
  target is decided where the target is known. Open question for the
  supervisor.
- `parse` returns the `Plan`, the ordered list of option uses (row, value,
  argument index, spelling as written), and the grammar diagnostics with
  their argument index; `refused_uses` is a query over the uses: one
  refusal per refused row, at its first use, in argument order (GCC passes
  `-plugin-opt=` seven times). Options without a typed `Plan` field land
  in `Plan::extra` as `(row name, value)`.
- Diagnostic keys: `Key::new(0, <argument index after response-file
  expansion>, 0)`; the final `Refused{Feature, "linking"}` after them
  (phase 1). It is added only when there is no `UnknownOption`,
  `MissingArgument`, `InvalidOptionValue` or `NoInputFiles`; refused
  options do not suppress it.
- Since `-o` is a refused row today, `crates/mortar/tests/cli.rs`
  `a_failed_link_reports_on_stderr_and_leaves_no_output` (`a.o -o a.out`,
  exact stderr) must gain the line `mortar: error: unsupported option: -o`
  before the "linking" refusal. Flag it in the handback.
- `spec/diag.md` §7 says a failed link prints nothing on stdout, so `-v`
  with inputs loses its version line while every link fails. `-v`/`-V`
  with no inputs: version on stdout, status 0 (GNU ld exits 0 there
  instead of `no input files`, recalled).
- Color: `mortar::link(&[OsString]) -> Outcome` unchanged in shape; a
  second entry (`invoke`) also returns the `--color-diagnostics` choice
  (`auto|always|never`, default auto; `--no-color-diagnostics` = never);
  `main` resolves auto with `std::io::IsTerminal` on stderr and `TERM !=
  dumb`.
- `docs/status/options.md` generated from the table by a golden-file test
  with `MORTAR_BLESS=1`, as hod's
  `crates/hod_ir/tests/classify_support_doc.rs` does for
  `docs/status/support.md`.

## Not started

Everything in the brief's scope list: grammar code, `Plan`, the table,
response files, the census of driver link lines, `--help`/`--version`
texts, adoption in `crates/mortar`, `spec/cli.md`, `spec/landed/cli/WP-008.md`,
the `spec/README.md` cell, all tests. The research on what cmake, meson,
libtool and GCC's configure look for in `--version` was not done; known
only: the three linkers' first lines here are `GNU ld (GNU Binutils for
Debian) 2.44`, `Debian LLD 22.1.8 (compatible with GNU linkers)` and `mold
2.37.1 (compatible with GNU ld)`.

## Trap

Never read `/run/mortar/jobserver.fifo` (a `cat`, a blocking shell read):
it takes tokens out of the machine's pool. This session did so once for
0.1 s by mistake; the refill log showed `deficit pending` on the next pass
(the loop restores a deficit seen twice). Use `scripts/jobserver.sh
status` to look.
