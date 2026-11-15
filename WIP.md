# WIP: WP-030 review (briefs/reviews/review-030-x86.md), all 16 findings accepted

Branch `wp/030-x86-spec`. Docs only; one commit per finding. Each claim
is re-measured with `.build/review-x86/` scripts before the edit.
Experiment dir: `/tmp/mortar-wp030r-EYWFOo` (remove by explicit path at the end).

| # | finding | state |
|---|---|---|
| 1 | T1 any destination register; unmatched GOTDESC refused; form matcher over both corpora (0 unmatched) | done |
| 2 | X86-13: delete the moffs reading; GOT32 at non-exec place refused; prefix note (SIB check kept: flag to supervisor) | done |
| 3 | max_reach (3, 6); Facts.seq_leads = [18, 19]; R-3 | done |
| 4 | REL addends for non-resident bodies: R-13 (image.md/arch.md), X86-5 rewritten, read windows in X86-40 | done |
| 5 | checks on the value reduced mod 2^32 (§3.1); X86-38 exception; W15 (mold) found; X86-10 wrap re-run | done |
| 6 | lld writes IBT notes and IBT IPLT on i386; only -z ibt unknown (warning) | done |
| 7 | GNU relaxes GOT32X in static-pie; M2 candidates (mold: G1 lea only when no other reference needs the slot) | done |
| 8 | EI_OSABI ruling (X86-64); §2.4 per-library OSABI-3 contents; E19 osabi32.py + review.sh osabi | done |
| 9, 13 | IFUNC names (S-68), __rel_iplt_* not defined in static-pie, R-14 (S-71 exception), R-15 + §1 tag, Q9 stub; W16 (mold) found | done |
| 10 | .got.plt word 0 in static-pie = _DYNAMIC (X86-47, R-16); patched-to-0 runs | done |
| 11 | tombstone replaces the field, A ignored (X86-59, I-45); review.sh tomb, GCC and Clang | done |
| 12 | existing diag kinds only: section-type, malformed-input, relocation-incompatible reasons, refused feature; R-5, R-9, R-10 withdrawn | done |
| 14 | (with 2) | done |
| 15 | §2.4: no GOTOFF names a merge section symbol; X86-8 examples are S-19 (review.sh merge) | done |
| 16 | Q1 closed (i686 gfortran: run under 4 linkers; same records as -m32 over 90 flag sets), Q6 closed (lldb-19 gdb-remote, 8 outputs) | done |
| extra | W8 (lld added), W9, W10, W12, W14 re-derived (review.sh wre); X86-10 wrap re-run (done in 5); mortar IPLT bytes run whole (review.sh iplt, ipltpatch.py) | done |
| end | merge main (b18e431) done, README conflict resolved, R-8/R-12 rewritten on main's plumb.md §4.5 x86-64 table and mortar_x64; run-all.sh full run; delete WIP.md | part |
