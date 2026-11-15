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
| 7 | GNU relaxes GOT32X in static-pie; M2 candidates | todo |
| 8 | EI_OSABI ruling; §2.4 libstdc++ UNIQUE | todo |
| 9, 13 | IFUNC names (S-68), __rel_iplt_* undefined in static-pie, S-71 exception request, M2 tag, stub Q | todo |
| 10 | .got.plt word 0 in static-pie = _DYNAMIC | todo |
| 11 | tombstone replaces the field | todo |
| 12 | existing diag classes (section-type, malformed-input) | todo |
| 14 | (with 2) | done |
| 15 | §2.4 GOTOFF wording; X86-8 example is S-19's | todo |
| 16 | Q1 (i386 gfortran run), Q6 (lldb) | todo |
| extra | re-derive W8, W9, W10, W12, W14; X86-10 wrap; mortar IPLT bytes run whole | todo |
| end | run-all.sh covers all; merge main 177c65b+; R-8/R-12 citations; delete WIP.md | todo |
