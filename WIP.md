# WIP: WP-164 (verifier: shared object's metadata; stripped outputs)

Part 1 code done (uncommitted spec text): P-D22/P-D6 DT_INIT/DT_FINI
stripped (dyncheck::stripped_def), reloc.rs undefined_out on stripped,
GOT base fallback (.got.plt x86-64), P-D9 slot guard; coverage
`stripped_not_judged`. Tests shared/shared-stripped,
dyn/dynamic/exec-stripped, exec-stripped-x64-large pass 4 linkers.
Next: part 2 (P-M2 Shared: presence by line kind; NONPREEMPTIBLE from
Sets after compute, in dynverify), part 3 (P-M1 in no segment),
mutations, sweep, spec text, record.
Found on the way (not mine): IFUNC export in -shared: bfd F4 under
-Bsymbolic, lld A20, mold view B (unstripped too); mold x86-64
-no-pie tbss-only PT_TLS outside PT_LOAD -> S9.
