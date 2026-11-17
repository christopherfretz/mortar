# WIP — WP-129 (metadata in `-shared`)

Brief: /Volumes/AgentWork/mortar-helper/briefs/wp-129.md. Branch
wp/129-meta-shared from int/batch-8 0067720. Scratch:
/Volumes/AgentWork/mortar-helper/.build/target-wp-145/wp129/.

## Done
- kiln-next + hod-next pair checked: kiln passes `KILN_HOD`, hod writes
  its 88-byte unit (aarch64 and x86-64 `--target`).

## Plan
1. Part 1.1 read-back matrix (scratch script `wp129/readback.sh`).
2. Part 1.2 rule: `mortar_odr::reloc_model` (container read only for live
   files with containers that define an exported preemptible datum);
   called from `crates/mortar/src/front.rs` in a `Shared` link.
3. Harness: `hod` step + `requires: hod` probe (form of WP-134's
   `KilnRecords`, branch wp/134-kiln-producer, read-only).
4. Part 2: odr_output for `Shared` (`executable: false`), NONPREEMPTIBLE =
   exported && !preemptible; table note in the shared path.
5. Part 3: --keep-mason-meta in `Shared`.

## Open questions
