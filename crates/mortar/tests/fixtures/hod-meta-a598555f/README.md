# Objects hod made with a `.mason.meta` (WP-099)

Made by hod at `a598555facdcc95cee7b7b00ca156f379083c82e` (hod 1.0.0's
rule 9.7.4 for `!exclude`; release build, package `hod_cli`), on an
AArch64 host, 2026-11-16. For each `<case>.ll` and each `<arch>` of
`aarch64` and `x86_64`, exactly:

    hod --strict -O0 -relocation-model=static -filetype=obj \
        -mtriple=<arch>-unknown-linux-gnu <case>.ll -o <case>.<arch>.o

`crates/mortar/tests/hod_meta.rs` links them (no hod needed: the hosted
lanes run it) and, where hod is built (`MASON_HOD`, or
`target-hod/release/hod` in the build root), makes every object again
with this command and requires the same bytes
(`the_fixtures_are_hods`). A hod that writes other bytes fails that test:
make the objects again in a new directory named for its sha, and say in
the commit what changed.

The cases (`spec/landed/meta/WP-099.md`, item 3; the unit of the first
nine is one 80-byte unit with the summary `lang=c++`):

| file | what hod is given | what hod writes |
|---|---|---|
| `base.ll` | `private constant`, `section ".mason.meta"`, align 8, `!exclude !{}`, `@llvm.compiler.used` | `.mason.meta`, `E`, align 8 |
| `used.ll` | the global `internal`, in `@llvm.used` | `RE` (`SHF_GNU_RETAIN`) |
| `comdat.ll` | in `comdat($g)` | `GE`, a group `g` holding it |
| `two.ll` | two such globals | one section, both units in order |
| `reloc.ll` | `{ [80 x i8], ptr @main }` | `E` and `.rela.mason.meta` |
| `alloc.ll` | no `!exclude` | `A` |
| `large.ll` | `section ".ldata.mason.meta"` | `.ldata.mason.meta`, `lE` on x86-64, `E` on AArch64 |
| `align4.ll` | align 4 | `E`, align 4 |
| `empty.ll` | `[0 x i8] zeroinitializer` | `E`, size 0 |
| `odr-a.ll`, `odr-b.ll` | `_Z1fv` `linkonce_odr` in a comdat, a definition row for it with two hashes; the container written by `cargo xtask meta write <a\|b>.meta --into-ll` | `E`, align 8 |
| `two-units.ll` | `odr-a.ll`'s code with both units in one container | `E`, align 8 |

The descriptions of `odr-a`, `odr-b` and `two-units` (for
`cargo xtask meta write`):

    summary name=lang value=c++
    summary name=odr.scheme value=model-1
    summary name=producer value="hod fixture (WP-099)"
    definition anchor=_Z1fv hash=11111111111111111111111111111111 class=1 flags=1 file=h.h line=1

`odr-b` the same with `hash=22221111111111111111111111111111`;
`two-units` is `odr-a`'s rows, a line `unit producer=4`, then `odr-b`'s.
