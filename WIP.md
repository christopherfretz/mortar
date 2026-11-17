# WP-141 WIP (delete in the final commit)

## State
- Measured (scope 1): script and raw tables in
  `/Volumes/AgentWork/mortar-helper/.build/target-wp-141/wp141/m/`
  (`meas.sh <a64|x64> <linkers>`; `UNVER=1` builds `libv.so` without a
  version script; `a64.txt`, `x64.txt` references; `*-mortar-base.txt` the
  base mortar; `*-u.txt` the unversioned variant). `-z now` changes nothing.
- STOPPED before choosing the rule (brief scope 2: the references disagree
  in a way that changes behaviour); question handed to the supervisor.

## The question (case (b): `libv` --as-needed, reached by `libm2`'s edge, U)
- AArch64: GNU ld and lld agree: `FUNC`/`OBJECT WEAK`, ver 1, no Verneed,
  GOT/PLT records; every program runs 27/25 with libv, 20/19 with alt
  (lld `-no-pie` `hp = vfn` initializer: 0 statically, 20 even with libv).
- x86-64 `-no-pie` (`-fno-pie` objects): GNU ld binds (option A):
  `cmp` canonical PLT (27; SIGSEGV with alt), `dat` `R_X86_64_COPY` from
  the not-needed `libv` (25; `symbol lookup error` with alt). lld leaves
  the key unbound (option B-like): no record, 20/19 even with libv loaded.
- mortar base: versioned (b) refused by D-27 (every row). UNVERSIONED (b)
  (existing behaviour, no refusal): GNU ld's image incl. the x86-64
  `-no-pie` canonical PLT and the COPY from the not-needed `libv.so`.
- Option B in mortar (unbound, D-25) would make `call` (b) `Weak0`: the
  call is skipped (a64 prints 21) or SIGSEGVs (x86-64) although libv IS
  loaded; all three references print 27.

## Next
Wait for the supervisor's ruling; then D-27/D-10 sentence, code
(`crates/mortar_synth/src/version.rs` `verneed_plan`/`bound`, or
`crates/mortar_resolve/src/shared.rs` `binds`), tests, record.
