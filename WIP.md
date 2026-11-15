# WIP — WP-041 (M2 step 0)

Done: Q7 (ed635c2). 0g `cargo xtask static-identical` (xtask/src/identical.rs, digest mode in
xtask/src/linktest/engine.rs, baseline tests/identical/baseline.tsv: 150 links / 40 tests, ~6-21 s;
ci step `static-identical`). Main's mortar (d86a0cd) vs the baseline: 42 AArch64 outputs differ (Q7), nothing else.
0a/0j: FileLive::Shared{needed,used}, IdSet, Resolution dynamic columns + bind(), check rule 7, model.md §3.3.
Learned, not yet written: GNU ld 2.44 AArch64 static writes `.plt`'s start into IPLT words (resolver in r_addend);
Q7 ruled S+A. x86-64 kept 0 (X-39, plumb F5(3)).
0b: Slots needed/verneed/dynstr/dynsym_unhashed/gnu_hash_buckets; CopyAlloc{place,align,alias}; DynStr, VerIndex, CopyPlace; Demands.weak_dyn; check rules.
0i: mortar_synth::dynsym (DynsymPlan, dynsym_plan stub); S-3 chunk list M2 rows + DynBss=24, BssRelRo=25, D-34 headers.
0h: mortar_synth::dynstr (DynNames, dynstr_plan stub); DynStr lives in Slots (0b).
Merged main 3f4cfe2 (4e76279; merge 2 of 3): S-69 consistent (my RELA bullet + 030c's REL paragraphs). Baseline re-cut:
167 links; i386: 33 rows exit 1->0 (end/output/map/stderr), 2 stderr-only (diag/duplicate, diag/undefined), 17 new rows;
x86-64 none; AArch64 one: reloc/data-pointers line 10 (output, map) — from main's 31175df (got_base_defined: the
_GLOBAL_OFFSET_TABLE_ definition now demands GOT_BASE on every arch, so .got gains its 8-byte header word; main's
own binary shows the same change). No other previously recorded digest moved.
Supervisor: the data-pointers header word is WANTED (layout L-101); record row+cause in docs/status/m2.md baseline history.
0c: OutputCx.bind_now (not PltVariant::Now: shapes stay under -z now, Bti combines; supervisor agreed);
Arch::lazy_target(PltVariant)->Option<LazyTarget>, dyn_tags(&OutputCx)->&[DynTag]{tag,value,when:TagWhen},
weak_use(r_type,place,offset,writable)->Option<WeakClass> (default Unfillable), DynRole::place(RelForm)->Option<PlaceContent>;
defaults only (WP-047/048 fill). Six M2 kinds registered (mortar_diag + spec/diag.md §3.1): versioned-export,
canonical-plt-in-pie, copy-relocation-against-protected, copy-relocation-of-zero-size, text-relocation,
weak-undefined-pc-in-pie; mortar_arch DynRefusal + ClassOutcome::Refused + ScanError::Dynamic carry them.
0k: mortar_scan::weak (SectionMap: Sync, WeakUses{got,abs_word,branch,unfillable:Option<Key>}.add/verdict(output,dynamic),
WeakKeys{weak_dyn,refused}, weak_classes(arch,t,opts,sections) stub) called in scan() after View::new, before the walk;
ScanOptions.sections: Option<&dyn SectionMap> (None everywhere).
0e done: crates/mortar/src/dynamic.rs (is_gate, dynamic_guard, option_refusals, arch_status, DynTables); mortar_cli Use::refusal; Front.dyn_tables; LinkArch::of(..,status); tests/refusals.rs the_dynamic_gates_keep_their_refusals (pinned before the change).
Was planned as 0e (dynamic_guard: CLI gate rows -pie, -dynamic-linker,
-Bdynamic filtered from parsed.refusals and refused by the guard with identical text; walk's §8.4 refusal stays, D-31),
docs/status/m2.md, spec/landed/dyn/WP-041.md.

Next: commit docs/status/m2.md + spec/landed/dyn/WP-041.md (fill 0e hash), gate `MORTAR_REQUIRE_TOOLS=1 cargo xtask ci --quick`, delete WIP.md, remove /tmp/mortar-wp041-3943017, hand back.
