//! The input section headers layout reads, one row per `SectionId`.
//!
//! Stage A to C read only these fields of an input section. They are
//! gathered once, in parallel over files, from the readers of `Inputs`;
//! tests build the column directly.

use mortar_core::check::{SectionShapes, Shape};
use mortar_core::column::Column;
use mortar_core::id::{FileId, SectionId};
use mortar_core::inputs::{Inputs, Reader};
use mortar_core::par::Pool;
use mortar_core::resolution::Resolution;
use mortar_elf::SectionIndex;
use mortar_elf::consts::SHF_COMPRESSED;

use crate::diag::{LayoutDiag, internal};

/// One input section, as layout sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header<'n> {
    pub name: &'n [u8],
    pub sh_type: u32,
    pub flags: u64,
    /// The size placed: the decompressed size of a `SHF_COMPRESSED` input
    /// (L-52).
    pub size: u64,
    /// log2 of the alignment (of the decompressed contents for a
    /// compressed input).
    pub align: u8,
    /// `sh_link` as a `SectionId`, for `SHF_LINK_ORDER` (L-57).
    pub link: Option<SectionId>,
    pub entsize: u64,
}

impl Header<'_> {
    /// The row of section 0 and of sections layout never reads.
    pub const EMPTY: Header<'static> =
        Header { name: b"", sh_type: 0, flags: 0, size: 0, align: 0, link: None, entsize: 0 };
}

/// The headers of every section, by `SectionId`.
pub type Headers<'n> = Column<SectionId, Header<'n>>;

/// Gathers the headers from the readers, in parallel over files (model §6
/// rule 3: an ordered collection, one unit per file).
///
/// # Errors
///
/// `internal-error` when a section the resolution numbered cannot be read
/// or has an alignment that is not a power of two: both were read and
/// checked by the reader before.
pub fn gather<'n>(
    inputs: &Inputs<'n>,
    resolution: &Resolution,
    pool: &Pool,
) -> Result<Headers<'n>, LayoutDiag> {
    let files = resolution.section_base.len() as usize;
    let per_file: Vec<Result<Vec<Header<'n>>, String>> = pool.map(files, |f| {
        let file = FileId(f as u32);
        let range = resolution.section_base[file];
        let mut rows = Vec::with_capacity(range.len() as usize);
        if range.is_empty() {
            return Ok(rows);
        }
        let Some(Reader::Object(object)) = inputs.files.get(file).map(|f| f.reader) else {
            return Err(format!("file {f} has sections but no object reader"));
        };
        for index in 0..range.len() {
            if index == 0 {
                rows.push(Header::EMPTY);
                continue;
            }
            let at = || format!("file {f} section {index}");
            let s = object.section(SectionIndex(index)).map_err(|_| at())?;
            let name = s.name().map_err(|_| at())?;
            let mut align = s.alignment().map_err(|_| at())?.max(1);
            let mut size = s.size();
            if s.flags() & SHF_COMPRESSED != 0
                && let Ok(Some(c)) = s.compression()
            {
                size = c.header.ch_size;
                align = c.header.ch_addralign.max(1);
            }
            if !align.is_power_of_two() {
                return Err(format!("{}: alignment {align}", at()));
            }
            let link = s
                .link()
                .ok()
                .filter(|l| l.0 != 0 && l.0 < range.len())
                .map(|l| SectionId(range.start.0 + l.0));
            rows.push(Header {
                name,
                sh_type: s.sh_type(),
                flags: s.flags(),
                size,
                align: align.trailing_zeros() as u8,
                link,
                entsize: s.header().sh_entsize,
            });
        }
        Ok(rows)
    });
    let mut all = Vec::with_capacity(resolution.sections.len() as usize);
    for rows in per_file {
        all.extend(rows.map_err(internal)?);
    }
    Column::from_vec(all).map_err(|_| internal("too many sections"))
}

/// The headers as the shapes `Layout::check` reads.
#[derive(Debug, Clone, Copy)]
pub struct HeaderShapes<'t, 'n>(pub &'t Headers<'n>);

impl SectionShapes for HeaderShapes<'_, '_> {
    fn shape(&self, section: SectionId) -> Option<Shape> {
        self.0.get(section).map(|h| Shape { size: h.size, align: h.align })
    }
}
