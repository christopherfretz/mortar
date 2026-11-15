//! AArch64's instruction proof (`spec/a64.md` §3.4): its mapping symbols,
//! as the scan and the erratum scan read them (`mortar_scan::CodeProof`).
//!
//! `CodeProof` should be an `Arch` method (`mortar_scan/src/lib.rs`, the
//! module note); until it is, the facade, which knows the architecture,
//! supplies it.

use mortar_a64::mapping::{MapSym, Mapping, mapping_kind, proof_runs};
use mortar_arch::ProofRun;
use mortar_scan::{CodeProof, Mark};

/// AArch64's mapping symbols (`$x`, `$d`).
#[derive(Debug, Clone, Copy, Default)]
pub struct A64Proof;

impl CodeProof for A64Proof {
    fn mark(&self, name: &[u8], st_info: u8) -> Option<bool> {
        mapping_kind(name, st_info).map(|m| m == Mapping::Code)
    }

    fn runs(&self, marks: &mut [Mark], len: u64, exec: bool, out: &mut Vec<ProofRun>) {
        let mut syms: Vec<MapSym> = marks
            .iter()
            .map(|m| MapSym {
                index: m.index,
                value: m.value,
                kind: if m.code { Mapping::Code } else { Mapping::Data },
            })
            .collect();
        proof_runs(&mut syms, len, exec, 0, 0, out);
    }
}
