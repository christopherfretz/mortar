### Second pass (`exp2/`)

`/Volumes/AgentWork/mortar-helper/.build/target-wp-110/exp2/` (README
there; uncommitted). Each `run.sh` writes into a fresh `work.run/` next
to it; `b1/` writes into `exp2/work/`.

| id | script | what | backs |
|---|---|---|---|
| X-B1 | `b1/corpus.sh WORK`, `b1/libtool-twins.sh WORK`, `b1/analyze.sh WORK` (`relscan.py`, `groundtruth.py`, `onlyrule.py`), `b1/fds/run.sh` | the address-level rule over about 14,400 objects; ground truth from twins; `-fdata-sections` | D-117, D-122, D-126 item 1, Q21, Q22, §F.1 |
| X-gap | `gapdemo/run.sh` (`gap2.c`, `host.c`) | the anchor+immediate mislink, three references, two targets | W44, Q21, P-R8 |
| X-escape | `escape/run.sh`, `escape/run2.sh` | protected data reached by `COPY` and GOT hosts; glibc's run-time answer; W41 | D-112, Q22, §F.6 |
| X-bsym | `bsym2/run.sh`, `bsym2/tvprobe.sh` | preemption per line, type and reference; the lld x86-64 TLS probe | D-112, W43, §F.2 |
| X-vmatch | `vmatch2/run.sh` (`model.py`, `scripts.txt`) | resolve §9.7's model against the three references | resolve §9.7, D-114, §F.3 |
| X-vscript | `vscript2/run.sh` | `.symver` against `local:`, parents, duplicate nodes, anonymous nodes, empty and several scripts | resolve §9.7 rules 10-13, script.md §9.1, §F.3 |
| X-comments | `comments/run.sh` | comment forms in version scripts and dynamic lists | script.md §9.1, D-114 |
| X-startstop | `startstop/run.sh` | `__start_`/`__stop_` export, visibility, interposition, gc | D-110 item 2, §F.4 |
| X-copy | `q10b/run.sh` (`classify.py`) | copies of locally bound data per section kind; the phdr-only decision | D-126 item 2, R-D43, §F.5 |
| X-order | `dynorder/check.py hash|dynamic LIB…` | D-26 and D-102 orders against reference libraries | D-111, D-102, §F.8 |
| X-maxpage | `maxpage/run.sh` (`phdr.py`) | `p_align`, congruence and RELRO end per reference and max page | D-123, §F.9 |
| X-lines | `ldlines/run.sh` | Python's `LDSHARED`, rustc's `cdylib` line | §A.4, §D, §F.10 |
| — | `patch_*.py` | the scripted edits of this pass to `spec/dyn.md` and this record | — |
