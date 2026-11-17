# WIP — WP-144 (verifier: TLS in a shared object, four findings on correct libraries)

Branch `wp/144-plumb-tls-shared` (from int/batch-6 b795288), worktree
`mortar/.worktrees/wp-140`, target `.build/target-wp-140`, scratch
`.build/target-wp-140/scratch/`. Brief: `briefs/wp-144.md`.
Order: findings 2-4 first (WP-127 part 2 waits), finding 1 (A46) last.

## State
- [x] 2. x86-64 TLSDESC kept in Shared: R1 "kept" branch (X-61)
- [x] 3. `_TLS_MODULE_BASE_` Y5 value per output kind
- [x] 4. mortar x86-64 local GD static DTPOFF word (P-D26 local pair)
- [x] 1. GNU ld spare R_AARCH64_NONE (A46)
- [ ] record spec/landed/plumb/WP-144.md; lane + gate; delete this file
