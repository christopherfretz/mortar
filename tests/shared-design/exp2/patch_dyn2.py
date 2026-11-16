import sys
p = sys.argv[1]; s = open(p).read()
a = s.index('**D-112 [M4] (preemption).**'); b = s.index('**D-115 [M4] (`.gnu.version_d`).**')
new = r'''**D-112 [M4] (preemption).** An exported definition is **preemptible**
(its uses go through a record the loader binds, so an executable, an
earlier library or `LD_PRELOAD` can interpose it) or **bound** (inside
the library at link time). `STV_PROTECTED` and version-local (D-114)
definitions are bound on every line; every other exported definition
follows this table, by the line's options and the key's type
(`exp2/bsym2`: 14 definitions, one per type × binding × visibility,
interposed by `LD_PRELOAD`; AArch64 natively, x86-64 under qemu; 17
lines × 3 references × 2 targets; the full answers per reference and
cell are in `exp2/bsym2/result.txt`, summarised in the record §F.2).
`P` preemptible, `B` bound; a weak definition is its type's column
(measured: every weak cell equals its global cell, all three):

| line (in order) | `FUNC` | `OBJECT` | `COMMON` | `NOTYPE` | `TLS` | `GNU_IFUNC` | the references |
|---|---|---|---|---|---|---|---|
| none | P | P | P | P | P | P | GNU ld, lld; mold binds `GNU_IFUNC` (W20) |
| `-Bsymbolic` | B | B | B | B | B | B | all three |
| `-Bsymbolic-functions` | B | P | P | P | P | P | lld; GNU ld also binds `NOTYPE`; mold also binds `GNU_IFUNC` (W20) |
| `--dynamic-list=F`, key not in F | B | B | B | B | B | B | lld on AArch64; GNU ld keeps `TLS` and `GNU_IFUNC` `P`; lld on x86-64 binds `TLS` half (W43); mold |
| `--dynamic-list=F`, key in F | P | P | P | P | P | P | GNU ld, lld; mold binds `GNU_IFUNC` (W20) |
| `--dynamic-list` with `-Bsymbolic` or `-Bsymbolic-functions`, either order | as `--dynamic-list` alone | | | | | | GNU ld (both forms), lld (`-Bsymbolic-functions`); lld binds every key with `-Bsymbolic` and writes `DF_SYMBOLIC` (W31); mold takes whichever comes last for `-Bsymbolic-functions` and ignores `-Bsymbolic` |
| `--dynamic-list` then `-Bno-symbolic` | as `--dynamic-list` alone | | | | | | GNU ld, lld; mold makes every key `P` |
| `-Bsymbolic -Bno-symbolic`, `-Bsymbolic-functions -Bno-symbolic` | P | P | P | P | P | P | all three |
| `-Bno-symbolic -Bsymbolic`, `-Bsymbolic-functions -Bsymbolic` | B | B | B | B | B | B | all three |
| `-Bsymbolic -Bsymbolic-functions` | as `-Bsymbolic-functions` | | | | | | all three (the last form wins) |
| `STV_PROTECTED` (any line) | B | B | — | — | B | B | all three |
| `--dynamic-list-data`, `--dynamic-list-cpp-new`, `--dynamic-list-cpp-typeinfo` | refused by name | | | | | | GNU ld accepts all three (`-data` acts as `-Bsymbolic-functions` binding `NOTYPE` too; `-cpp-*` as a list of `operator new`/`delete` or typeinfo); lld refuses all three (`unknown argument`); mold accepts `-data` and changes nothing, refuses `-cpp-*` |
| `-Bsymbolic-non-weak-functions`, `-Bsymbolic-non-weak` | refused by name | | | | | | GNU ld does not know them (lld and mold accept) |

In words: without `--dynamic-list`, the last of `-Bsymbolic`,
`-Bsymbolic-functions` and `-Bno-symbolic` decides (`-Bsymbolic` binds
every type; `-Bsymbolic-functions` binds `STT_FUNC` only, lld's rule;
`-Bno-symbolic` none). With a `--dynamic-list`, the list decides alone
(every unlisted key bound, of every type, lld's rule; every listed key
preemptible) whatever `-Bsymbolic` form comes before or after it (GNU
ld's rule; mold's order dependence is in 17.8). `DT_SYMBOLIC`/
`DF_SYMBOLIC` are written iff the line has no `--dynamic-list` and its
last form is `-Bsymbolic` (D-102, D-103; request P-R7): glibc applies
them to every symbol of the library, so with a list they would bind the
listed keys too (W31); with every key bound at link time they change
nothing and tell a reader of the library (D-126 item 2) that it is
symbolic. A key bound by the table is `Local` (or `Ifunc`, `Abs`) for
the relocation rules (D-113), a preemptible one `Preemptible` (request
R-A10); a bound `STT_TLS` key's GD pair is one `DTPMOD` with `r_sym` 0
and a static offset (D-119), never lld's x86-64 mixture (W43).

Everything else exported is preemptible: default visibility, weak
definitions, IFUNCs (D-121), and a definition that a needed shared
input also defines (the library's own wins at link time, even weak over
the shared strong, and stays preemptible: with the other library first
in the loader's order its definition is used by the library too; all
three, [X-G] G2). A data definition that some input binds without a
record (D-117's address-level rule, non-PIC code) is not made bound: the
link is refused. The input property
`GNU_PROPERTY_1_NEEDED_INDIRECT_EXTERN_ACCESS` (`-mno-direct-extern-access`,
`-z indirect-extern-access`) is kept in the output when any input has
it, as GNU ld (lld and mold drop it, and a copy-relocating executable
then runs with two instances of protected data where glibc refuses GNU
ld's library: W41; glibc 2.41 refuses a protected `COPY` at run time only
when the library has the property, and otherwise warns ``copy
relocation against non-copyable protected symbol`` and runs with two
instances, `exp2/escape/run2.sh`) [x86-64 and i386 measured; the
AArch64 property unmeasured]. *Tests:* `shared-preemption` (the table:
every row and column, run under `LD_PRELOAD`, both targets),
`shared-protected` (executables against a protected definition:
`-fPIE` GOT ones run, COPY and canonical PLT ones refused, D-18),
`dynamic-list-symbolic` (W31), `dynamic-list-tls-x64` (W43).

**D-113 [M4] (where preemption lives).** Preemption is a property of a
key computed by resolution after the export set, before the relocation
scan (resolve §9.6), stored per key in `Resolution` (request R-M10:
model.md gains a `preemptible` bit per key, `false` for every key of an
executable). `target_kind` (arch.md §2.1) reads it: `Preemptible`
replaces arch §2.1's kind for an exported preemptible definition; a
definition that is not preemptible keeps §2.1's kind (`Local`, `Abs` for
an `SHN_ABS` key: never a `RELATIVE`, D-100; `Ifunc` for a
non-preemptible IFUNC: its IPLT entry, D-121); an import is `Shlib`; a
strong key that nothing defines is `Shlib{func: false}` (D-111 item 2:
it is reached exactly as an import, through a record naming it); every
weak undefined key is `WeakDyn`. Nothing else in the pipeline asks
whether the output is `Shared`.

**D-114 [M4] (version scripts applied).** resolve.md §9.7 is the rule,
restated by WP-110's second pass from an executed model
(`exp2/vmatch2`: its GNU column equals GNU ld on 159 of 159 rows of 53
scripts × 3 names; lld and mold differ where 17.6 says). mortar is GNU
ld wherever the references differ, with these rulings for `Shared`:

- matching: exact names first, then non-star globs (the last node wins,
  `global:` over `local:` within a node), then `*` (resolve §9.7's
  decision). The two scripts on which GNU ld and mold give V1 and lld
  local (`V1 { global: fo*; }; V2 { local: foo*; };` and the same with
  `local: *` in V1) follow GNU ld: a recorded difference;
- quoted entries are exact, `"foo*"` included (GNU ld; lld and mold
  glob them);
- an exact name under `global:` in two nodes: the first, warning
  `version-reassigned` (GNU ld silent, lld warns, mold the last);
- the same entry under `global:` in one node and `local:` in another
  (exact, glob or `*`): refused, `duplicate-version-pattern`, as GNU ld
  (lld and mold accept with differing answers);
- `local:` before `global:` in one node: accepted (lld, mold; GNU ld
  refuses with a syntax error); the `global:` entries still count first;
- a name in a script that no file defines: **accepted with the warning
  `version-script-undefined-name`** (GNU ld accepts silently; zlib's
  `configure` probe links `zlib.map` against one function, so under
  lld's refusal Clang+lld builds no shared zlib, record §A);
  `--no-undefined-version` refuses with `undefined-version` (GNU ld:
  `nosuch: undefined version: V1`); `--undefined-version` is a no-op;
- several parents (`V3 { … } V1 V2;`): accepted, every parent written
  (GNU ld; lld and mold refuse the syntax); a forward or missing parent,
  a node name used twice, an anonymous node with named ones: refused
  (GNU ld; script.md §9.1);
- several `--version-script` files: one script, in order (all three);
- comments: `#` to the end of the line and `/* */` (all three);
  `//` refused (all three; script.md §9.1);
- `extern "C++"` patterns need the demangler (diag.md §9): until it
  exists a script with `extern "C++"` is refused by name; `extern "C"`
  is plain names. `extern "C++" { *; }` with `local: *` exports every
  symbol (GNU ld, lld; mold exports none, W40);
- an anonymous script (`{ global: a; local: *; };`) exports the listed
  keys unversioned and writes no `.gnu.version_d` (all three);
- a `.symver` definition `foo@V1` (non-default) is local exactly when
  node V1's own `local:` entries match `foo` (resolve §9.7 rule 12): GNU
  ld and lld drop it (`V1 { global: bar; local: *; };`), and all three
  keep it when the `local:` is in another node (`exp2/vscript2`).
  mortar drops it as GNU ld and lld, with the warning
  `versioned-definition-hidden` (an executable linked against the
  earlier library then fails to load, ``undefined symbol: foo, version
  V1``: the warning says so). mold keeps it (17.8). Q12 is closed this
  way (the review's ruling);
- `foo@V`/`foo@@V` with no node `V` in the script, or with no script:
  refused, `undefined-version` (all three, [X-F] F2; resolve §9.4
  confirmed; D-27's note on GNU ld writing a verdef holds for
  executables only).

The script grammar is script.md §9. *Tests:* `version-script-rules`
(every script of `exp2/vmatch2/scripts.txt`, each with its
`readelf --dyn-syms -V`; the model's mortar column is the expectation),
`version-script-zlib` (zlib.map; the 48 cross-runs),
`version-script-undefined-name`, `version-script-comments`,
`symver-old-and-default` (an old and a new executable run against one
library), `symver-hidden-by-own-node`.

'''
s = s[:a] + new + s[b:]
open(p, 'w').write(s)
