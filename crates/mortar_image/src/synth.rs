//! The synthetic-chunk interface of `spec/image.md` §13 is
//! `mortar_synth`'s ([`SynthChunk`], [`Division`], [`SizeCx`], and its
//! own [`SynthCx`], the answers the image gives a chunk's `write`). This
//! module re-exports it and gives the empty answers, for an image whose
//! chunks read no address (tests, and links without the chunk that asks).

pub use mortar_synth::eh::CodeRef;
pub use mortar_synth::got::{GotAddrs, SlotValues};
pub use mortar_synth::hdr::HdrAddrs;
pub use mortar_synth::image::{
    Division, EhChunk, FillCx as SynthCx, GotChunk, GotPart, HdrChunk, SizeCx, SymTable,
    SymtabChunk, SynthChunk,
};
pub use mortar_synth::symtab::{InternalValue, SymAddrs};
pub use mortar_synth::{SynthError, Window};

use mortar_arch::TlsBlock;
use mortar_core::{SectionId, SymKey, SymbolId};

/// No address is known: every question is `None`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAddrs;

impl SlotValues for NoAddrs {
    fn address(&self, _: SymKey) -> Option<u64> {
        None
    }
    fn tls(&self) -> Option<TlsBlock> {
        None
    }
}

impl SymAddrs for NoAddrs {
    fn section(&self, _: SectionId, _: u64) -> Option<(u32, u64)> {
        None
    }
    fn eh_frame(&self) -> Option<(u32, u64)> {
        None
    }
    fn common(&self, _: SymbolId) -> Option<(u32, u64)> {
        None
    }
    fn internal(&self, _: SymbolId) -> Option<InternalValue> {
        None
    }
    fn patch(&self, _: usize) -> Option<(u32, u64)> {
        None
    }
    fn tls_start(&self) -> Option<u64> {
        None
    }
    fn section_range(&self, _: u32) -> Option<(u64, u64)> {
        None
    }
}

/// No code address is known.
#[must_use]
pub fn no_code(_: CodeRef) -> Option<u64> {
    None
}

/// The answers of [`NoAddrs`] as a chunk's `write` takes them.
#[must_use]
pub fn no_answers<'a>(
    arch: &'a dyn mortar_arch::Arch,
    pool: &'a mortar_core::par::Pool,
    output: &'a mortar_diag::FileRef,
) -> SynthCx<'a> {
    SynthCx {
        arch,
        pool,
        output,
        slots: &NoAddrs,
        syms: &NoAddrs,
        got: GotAddrs { got: 0, got_plt: 0, plt: 0 },
        hdr: HdrAddrs { hdr: 0, eh: 0 },
        code_addr: &no_code,
    }
}
