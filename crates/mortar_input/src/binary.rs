//! `-b binary` inputs (`spec/layout.md` L-R9, `spec/cli.md` C-R3's `-b`
//! row, `--format=binary`, `-b default`). Owner: **RL-10**, which owns
//! this file and the dispatch line of `load.rs` that sends a `binary`
//! input here (and its `mortar_cli` rows, after RL-2). No `-r` branch of
//! resolve, layout or image is touched: the object built here is an
//! ordinary input (record `spec/landed/layout/WP-154.md` §F).
//!
//! WP-181 (RL-1) wrote this file with [`object`]'s signature and its
//! refusal; RL-10 fills [`object`] here and may add private items to this
//! file.

use mortar_diag::Diagnostic;
use mortar_elf::Machine;

/// The bytes of the ordinary `ET_REL` input object that stands for the
/// `-b binary` file `path` with `contents`, for `machine` (`-m`'s, or the
/// line's): one `.data` section of the contents, alignment 1, and a
/// symbol table of L-R9's three `NOTYPE GLOBAL` symbols in L-R9's order
/// (`_binary_<path>_size` `SHN_ABS`, `_end`, `_start`), `<path>` with
/// every byte that is not alphanumeric written `_`. The loader then reads
/// it as any object.
///
/// # Errors
///
/// Until RL-10 lands: `unsupported option: -b with -r`
/// (`spec/diag.md` §4.4).
pub fn object(path: &[u8], contents: &[u8], machine: Machine) -> Result<Vec<u8>, Diagnostic> {
    let _ = (path, contents, machine);
    Err(mortar_diag::relocatable::refused_with_r("-b"))
}
