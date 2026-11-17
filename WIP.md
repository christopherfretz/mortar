# WIP: WP-172 (deleted in the final commit)

- Code: `ScriptEntry` (walk + core) carries `as_needed` and `group`;
  `shared_link` reads them; `MentionGroup::Unknown`, the merged-state
  inference and `What::NeededOrderUnknown` removed.
- Experiment: `.build/target-wp-116/wp172/exp.sh <arch> <mortar>`;
  base results `base-<arch>.txt`.
- Next: measure with the new mortar; lane tests; spec; record.
