//! `.shstrtab` (`spec/synth.md` S-90): a zero byte, then each output
//! section's name in section index order, each once, `.shstrtab`
//! included. Planned after layout stage C, when the sections and their
//! order are known; written whole (one unit, S-4).

use std::collections::HashMap;

/// The names and the offset of each section's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShstrtabPlan {
    bytes: Box<[u8]>,
    /// Per section, in index order from 1: `sh_name`.
    names: Box<[u32]>,
}

impl ShstrtabPlan {
    /// The table for the output sections `names`, in section index order
    /// starting at index 1 (index 0, the null section, has name 0). A name
    /// given twice is written once (S-90).
    #[must_use]
    pub fn new(names: &[&[u8]]) -> Self {
        let mut bytes = vec![0u8];
        let mut seen: HashMap<&[u8], u32> = HashMap::new();
        let mut offs = Vec::with_capacity(names.len());
        for &name in names {
            let off = *seen.entry(name).or_insert_with(|| {
                let at = bytes.len() as u32;
                bytes.extend_from_slice(name);
                bytes.push(0);
                at
            });
            offs.push(off);
        }
        Self { bytes: bytes.into(), names: offs.into() }
    }

    /// The size (S-108): `1 + Σ (name + 1)`.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    /// `sh_name` of section `index` (from 1).
    #[must_use]
    pub fn sh_name(&self, index: u32) -> Option<u32> {
        index.checked_sub(1).and_then(|i| self.names.get(i as usize)).copied()
    }

    /// The bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
