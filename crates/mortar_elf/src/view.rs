//! The bytes a reader reads: a whole file, or a **parts view** of it
//! (`spec/elf.md` §3.0, `spec/input.md` §11.3).
//!
//! A [`View`] is a file of a known length of which some ranges, the
//! *parts*, are in memory. Parts are sorted, disjoint and not adjacent,
//! so a structure the readers read lies inside one part or in none:
//! [`View::get`] gives the bytes of a range exactly when one part holds
//! it. A whole file is one part. `Copy`, and borrows only the parts.

use core::fmt;

/// One resident range of a file: its offset and its bytes. Owned by the
/// store that read it; views borrow a slice of them.
#[derive(Clone, PartialEq, Eq)]
pub struct Part {
    /// The offset of the first byte in the file.
    pub offset: u64,
    /// The bytes.
    pub bytes: Box<[u8]>,
}

impl Part {
    /// The range of the file the part holds.
    pub fn end(&self) -> u64 {
        self.offset + self.bytes.len() as u64
    }
}

impl fmt::Debug for Part {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Part({:#x}..{:#x})", self.offset, self.end())
    }
}

#[derive(Clone, Copy)]
enum Src<'a> {
    /// The whole underlying file, from offset 0.
    Whole(&'a [u8]),
    /// Parts of the underlying file, sorted by offset, disjoint.
    Parts(&'a [Part]),
}

/// The bytes of a file, or of a range of one (an archive member), as far
/// as they are resident. Offsets given to a view are relative to its
/// start.
#[derive(Clone, Copy)]
pub struct View<'a> {
    src: Src<'a>,
    /// Where the view starts in the underlying file.
    base: u64,
    len: u64,
}

impl fmt::Debug for View<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts = match self.src {
            Src::Whole(_) => 1,
            Src::Parts(p) => p.len(),
        };
        write!(f, "View(base {:#x}, len {:#x}, {parts} parts)", self.base, self.len)
    }
}

impl<'a> View<'a> {
    /// A whole file: everything is resident.
    pub fn whole(bytes: &'a [u8]) -> View<'a> {
        View { src: Src::Whole(bytes), base: 0, len: bytes.len() as u64 }
    }

    /// A file of `len` bytes of which `parts` are resident. `parts` must
    /// be sorted by offset and disjoint, and lie within `len`; parts that
    /// do not are ignored by [`View::get`] (it never returns bytes outside
    /// a part, and never panics).
    pub fn parts(parts: &'a [Part], len: u64) -> View<'a> {
        if let [one] = parts
            && one.offset == 0
            && one.bytes.len() as u64 == len
        {
            return View::whole(&one.bytes);
        }
        View { src: Src::Parts(parts), base: 0, len }
    }

    /// The length of the viewed file (or range), resident or not.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The bytes `start..start + size`, when they lie within the view and
    /// inside one resident part. An empty range within the view is always
    /// resident.
    #[inline]
    pub fn get(&self, start: u64, size: u64) -> Option<&'a [u8]> {
        let end = start.checked_add(size).filter(|end| *end <= self.len)?;
        let abs = self.base + start;
        let abs_end = self.base + end;
        match self.src {
            Src::Whole(bytes) => {
                bytes.get(usize::try_from(abs).ok()?..usize::try_from(abs_end).ok()?)
            }
            Src::Parts(parts) => {
                if size == 0 {
                    return Some(&[]);
                }
                let i = parts.partition_point(|p| p.offset <= abs).checked_sub(1)?;
                let part = &parts[i];
                let lo = usize::try_from(abs - part.offset).ok()?;
                let hi = usize::try_from(abs_end - part.offset).ok()?;
                part.bytes.get(lo..hi)
            }
        }
    }

    /// Whether `start..start + size` lies within the view (resident or
    /// not).
    pub fn contains(&self, start: u64, size: u64) -> bool {
        start.checked_add(size).is_some_and(|end| end <= self.len)
    }

    /// The first `min(size, len)` bytes, when resident.
    pub fn prefix(&self, size: u64) -> Option<&'a [u8]> {
        self.get(0, size.min(self.len))
    }

    /// The whole view as one slice, when one part holds all of it.
    pub fn as_slice(&self) -> Option<&'a [u8]> {
        self.get(0, self.len)
    }

    /// The range `start..start + size` as a view of its own (an archive
    /// member), or `None` when it does not lie within this one.
    pub fn sub(&self, start: u64, size: u64) -> Option<View<'a>> {
        if !self.contains(start, size) {
            return None;
        }
        Some(View { src: self.src, base: self.base + start, len: size })
    }

    /// Where the view starts in the underlying file.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// Whether every byte of the view is resident.
    pub fn is_whole(&self) -> bool {
        matches!(self.src, Src::Whole(_)) || self.as_slice().is_some()
    }
}

impl<'a> From<&'a [u8]> for View<'a> {
    fn from(bytes: &'a [u8]) -> View<'a> {
        View::whole(bytes)
    }
}
