//! `--keep-mason-meta` (`spec/meta.md` MT-32, MT-30; WP-076): the units of
//! the live inputs' containers, concatenated for tools (tuck) into the
//! output's non-allocated `.mason.meta`.
//!
//! The order is MT-30's: by `FileId`, then by section index, then by the
//! unit's place in its section; each unit is copied as it is (mortar does
//! not re-encode a unit it did not write), followed by zeros to the next
//! multiple of 8 (MT-10's padding). A unit that does not parse is dropped
//! (MT-30, MT-14): the check that reports it (`meta-malformed`, strict
//! only, O-41) is the record checks', not this copy's. The bytes depend
//! on the inputs only, never on the threads.

use mortar_core::par::Pool;
use mortar_core::{FileId, Inputs, Resolution};
use mortar_elf::SectionIndex;
use mortar_meta::read::FileReader;

use crate::records::container;

/// The kept section's bytes: empty when no live input has a unit.
#[must_use]
pub fn kept_units(inputs: &Inputs<'_>, resolution: &Resolution, pool: &Pool) -> Vec<u8> {
    let meta: &[(FileId, SectionIndex)] = &resolution.meta;
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    for i in 1..=meta.len() {
        if i == meta.len() || meta[i].0 != meta[start].0 {
            if start < i {
                runs.push((start, i));
            }
            start = i;
        }
    }
    let per_file: Vec<Vec<u8>> = pool
        .unit_cost(20_000)
        .map_collect(runs.len(), |r, out: &mut Vec<Vec<u8>>| {
            let (a, b) = runs[r];
            let sections: Vec<_> = meta[a..b]
                .iter()
                .filter_map(|&(file, index)| container(inputs, file, index))
                .collect();
            let mut reader = FileReader::new();
            for (attrs, bytes) in &sections {
                reader.section(*attrs, bytes);
            }
            let mut units: Vec<_> = reader.units.iter().collect();
            units.sort_by_key(|u| (u.section, u.offset));
            let mut bytes = Vec::new();
            for u in units {
                bytes.extend_from_slice(u.bytes());
                bytes.resize(bytes.len().next_multiple_of(8), 0);
            }
            out.push(bytes);
        })
        .items;
    per_file.concat()
}
