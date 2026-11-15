# WIP: WP-060 final fix round (re-review N1–N7, n1–n9, operator sizing)

State at this commit: every ruling is applied in `spec/incr.md` and the record
except the measurements below. A fresh agent finishes from: this branch, the
review `briefs/reviews/review-060-incr-re.md` (context repo), the first review
`review-060-incr.md`, and the rulings restated here.

## Rulings (restated; all applied in text)
- N1: delta carries type/binding/visibility/st_other/definition kind; a change for
  a key an unchanged file references = full link `symbol-kind-changed` (IN-26, §7).
- N2: stored liveness graph = I-7 edge set, one row kind per edge kind, written and
  walked by the liveness pass's own enumerator (IN-29); mutation per kind; L10
  `lsda-edges`; an unstorable kind = named fallback (none needed).
- N3: IN-3a check 1 by what a reference names; `arr+size` and `blk_-8` valid.
- N4: oracle compares moved units by value; N1 pair with global/synth dropped.
- N5: stamps only on allow-listed fs types after a probe; FUSE etc. untrusted.
- N6/N7: omitted phases, promise table, scaling model, go/no-go that can say no,
  thresholds an operator decision.
- Operator sizing: reference config = static, -g (split DWARF expected), 0.5 GB
  typical / 2 GB rare, 1,000–3,000 archives, reflink fs. Clone path in IN-47.
  Split DWARF section measured on protoc. Decisions (a) 1, 9.

## Remaining work (measurements; scripts under .build/review-incr/)
1. Split DWARF on protoc: run a helper with `.build/review-incr/BRIEF-split.md`;
   fill §6 "Split DWARF" (sizes, slack, refill), §1.5 note, record (a) 9's
   measured gain, record §3 item.
2. (done: protoc edge counts in IN-29, `results/fx/protoc-edges-stats.txt`)
3. N2 dedicated gc scenarios for startstop, linkorder (`"ao"` section),
   group (second member only via group), nonalloc, local; add to
   `oracle_scen.sh`, rerun `livemodel.py --drop KIND`; fill IN-29 text.
4. Oracle: diagnose the 6 failing pairs (O2 l merged `.rodata.str*`; lx
   `crtbeginT.o` R_X86_64_32) – row gap or oracle gap; update IN-63b and §3.
5. `run-all.sh`: add stamp_probe.py, livemodel.py, the new oracle pairs.
6. Final commit: delete WIP.md; remove `.build/target-wp-060`.
