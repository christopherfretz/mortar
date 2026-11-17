# WP-131 work in progress (deleted in the final commit)

- [x] V1/V6: opts.rs holds the whole table (ACCEPTED + REFUSED); `Table::word`
      is the one word reader; line.rs dispatches on row ids (INTERPRETED);
      test over the table `line::tests::s12_1_every_spelling_of_a_row_reads_alike`.
- [x] V5: kind from the line; no PT_DYNAMIC with a dynamic line/output = finding
- [x] V5 walk: absent headers/tables
- [x] V3: S2 reads the line's max page size
- [x] V4: R6 entry, compressed not judged; mortar SHF_COMPRESSED finding
- [x] V2: A44 lld DF_SYMBOLIC under -Bsymbolic
- [ ] markers removed, rows, record spec/landed/plumb/WP-131.md, spec text
