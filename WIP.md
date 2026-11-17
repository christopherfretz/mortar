# WIP: WP-196 (load-time ODR runtime to shipped bytes)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-196.md. Scratch: .build/target-wp-077/wp196/.
- [x] move the runtime to runtime/odr/
- [x] runtime: dynsym-to-row map, bounded reads, bad-mode (one per process), AT_SECURE, CP, IF, TL, MT
- [x] matrix cases CP IF TL HV SE BM MT VC + RB attacks (smoked on aarch64)
- [x] objects + PINS + xtask odr-runtime (commit 4664168e has the final objects)
- [ ] matrix on 3 targets with final objects (wp196/matrix.sh -> matrix.log, m-*.log)
- [ ] cost rerun before/after at low load (cost-before.log, cost-after.log provisional, load 6-22)
- [ ] spec/odr.md §10.3 (draft wp196/s103.md, COSTTABLE placeholder), O-52 sentence, O-50 sentence
- [ ] record WP-196, gates, delete WIP.md
