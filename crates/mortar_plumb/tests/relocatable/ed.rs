//! A small editor of little-endian `ET_REL` files for the mutations of
//! `spec/plumb.md` §9.2 (t): it reads and rewrites header fields, section
//! headers, symbols and relocation records in place. A section whose
//! bytes grow is moved to the end of the file; a new section copies the
//! section header table to the end. Independent of the verifier's reader.

#![allow(dead_code)]

pub const SHT_SYMTAB: u32 = 2;
pub const SHT_RELA: u32 = 4;
pub const SHT_REL: u32 = 9;
pub const SHT_GROUP: u32 = 17;
pub const STT_SECTION: u8 = 3;
pub const SHF_LINK_ORDER: u64 = 0x80;
pub const SHF_GNU_RETAIN: u64 = 0x20_0000;
pub const SHF_EXECINSTR: u64 = 4;

/// A section header.
#[derive(Clone, Debug)]
pub struct Sh {
    pub name: u32,
    pub ty: u32,
    pub flags: u64,
    pub addr: u64,
    pub offset: u64,
    pub size: u64,
    pub link: u32,
    pub info: u32,
    pub align: u64,
    pub entsize: u64,
}

/// A symbol.
#[derive(Clone, Debug)]
pub struct Sym {
    pub name: u32,
    pub value: u64,
    pub size: u64,
    pub info: u8,
    pub other: u8,
    pub shndx: u16,
}

impl Sym {
    pub fn bind(&self) -> u8 {
        self.info >> 4
    }
    pub fn ty(&self) -> u8 {
        self.info & 0xf
    }
}

/// A relocation record; `addend` is `None` for REL.
#[derive(Clone, Debug)]
pub struct R {
    pub offset: u64,
    pub sym: u32,
    pub ty: u32,
    pub addend: Option<i64>,
}

#[derive(Clone)]
pub struct Ed {
    pub b: Vec<u8>,
    pub c64: bool,
}

impl Ed {
    pub fn new(b: &[u8]) -> Ed {
        assert_eq!(&b[..4], b"\x7fELF");
        Ed { b: b.to_vec(), c64: b[4] == 2 }
    }

    pub fn rd(&self, at: usize, n: usize) -> u64 {
        let mut v = 0u64;
        for i in (0..n).rev() {
            v = (v << 8) | u64::from(self.b[at + i]);
        }
        v
    }

    pub fn wr(&mut self, at: usize, n: usize, v: u64) {
        for i in 0..n {
            self.b[at + i] = (v >> (8 * i)) as u8;
        }
    }

    fn w(&self) -> usize {
        if self.c64 { 8 } else { 4 }
    }

    // ---- the file header ----
    pub fn shoff(&self) -> usize {
        if self.c64 { self.rd(0x28, 8) as usize } else { self.rd(0x20, 4) as usize }
    }
    fn set_shoff(&mut self, v: usize) {
        if self.c64 { self.wr(0x28, 8, v as u64) } else { self.wr(0x20, 4, v as u64) }
    }
    pub fn shnum(&self) -> usize {
        self.rd(if self.c64 { 0x3c } else { 0x30 }, 2) as usize
    }
    fn set_shnum(&mut self, v: usize) {
        let at = if self.c64 { 0x3c } else { 0x30 };
        self.wr(at, 2, v as u64);
    }
    fn shentsize(&self) -> usize {
        if self.c64 { 64 } else { 40 }
    }
    pub fn shstrndx(&self) -> usize {
        self.rd(if self.c64 { 0x3e } else { 0x32 }, 2) as usize
    }
    pub fn set_type(&mut self, v: u16) {
        self.wr(16, 2, u64::from(v));
    }
    pub fn set_phnum(&mut self, v: u16) {
        let at = if self.c64 { 0x38 } else { 0x2c };
        self.wr(at, 2, u64::from(v));
    }

    // ---- section headers ----
    fn sh_at(&self, i: usize) -> usize {
        self.shoff() + i * self.shentsize()
    }

    pub fn sh(&self, i: usize) -> Sh {
        let a = self.sh_at(i);
        if self.c64 {
            Sh {
                name: self.rd(a, 4) as u32,
                ty: self.rd(a + 4, 4) as u32,
                flags: self.rd(a + 8, 8),
                addr: self.rd(a + 16, 8),
                offset: self.rd(a + 24, 8),
                size: self.rd(a + 32, 8),
                link: self.rd(a + 40, 4) as u32,
                info: self.rd(a + 44, 4) as u32,
                align: self.rd(a + 48, 8),
                entsize: self.rd(a + 56, 8),
            }
        } else {
            Sh {
                name: self.rd(a, 4) as u32,
                ty: self.rd(a + 4, 4) as u32,
                flags: self.rd(a + 8, 4),
                addr: self.rd(a + 12, 4),
                offset: self.rd(a + 16, 4),
                size: self.rd(a + 20, 4),
                link: self.rd(a + 24, 4) as u32,
                info: self.rd(a + 28, 4) as u32,
                align: self.rd(a + 32, 4),
                entsize: self.rd(a + 36, 4),
            }
        }
    }

    pub fn set_sh(&mut self, i: usize, s: &Sh) {
        let a = self.sh_at(i);
        if self.c64 {
            self.wr(a, 4, u64::from(s.name));
            self.wr(a + 4, 4, u64::from(s.ty));
            self.wr(a + 8, 8, s.flags);
            self.wr(a + 16, 8, s.addr);
            self.wr(a + 24, 8, s.offset);
            self.wr(a + 32, 8, s.size);
            self.wr(a + 40, 4, u64::from(s.link));
            self.wr(a + 44, 4, u64::from(s.info));
            self.wr(a + 48, 8, s.align);
            self.wr(a + 56, 8, s.entsize);
        } else {
            self.wr(a, 4, u64::from(s.name));
            self.wr(a + 4, 4, u64::from(s.ty));
            self.wr(a + 8, 4, s.flags);
            self.wr(a + 12, 4, s.addr);
            self.wr(a + 16, 4, s.offset);
            self.wr(a + 20, 4, s.size);
            self.wr(a + 24, 4, u64::from(s.link));
            self.wr(a + 28, 4, u64::from(s.info));
            self.wr(a + 32, 4, s.align);
            self.wr(a + 36, 4, s.entsize);
        }
    }

    fn cstr(&self, at: usize) -> String {
        let end = self.b[at..].iter().position(|&c| c == 0).map_or(self.b.len(), |p| at + p);
        String::from_utf8_lossy(&self.b[at..end]).into_owned()
    }

    pub fn name(&self, i: usize) -> String {
        let st = self.sh(self.shstrndx());
        self.cstr(st.offset as usize + self.sh(i).name as usize)
    }

    /// The first section of a name.
    pub fn find(&self, name: &str) -> Option<usize> {
        (1..self.shnum()).find(|&i| self.name(i) == name)
    }

    pub fn all(&self, name: &str) -> Vec<usize> {
        (1..self.shnum()).filter(|&i| self.name(i) == name).collect()
    }

    pub fn data(&self, i: usize) -> Vec<u8> {
        let s = self.sh(i);
        if s.ty == 8 {
            return Vec::new();
        }
        self.b[s.offset as usize..(s.offset + s.size) as usize].to_vec()
    }

    /// Replaces a section's bytes: in place when the size is the same,
    /// else at the end of the file.
    pub fn set_data(&mut self, i: usize, d: &[u8]) {
        let mut s = self.sh(i);
        if d.len() as u64 == s.size {
            let at = s.offset as usize;
            self.b[at..at + d.len()].copy_from_slice(d);
            return;
        }
        let al = s.align.max(1) as usize;
        while self.b.len() % al != 0 {
            self.b.push(0);
        }
        s.offset = self.b.len() as u64;
        s.size = d.len() as u64;
        self.b.extend_from_slice(d);
        self.set_sh(i, &s);
    }

    /// Appends `name` to `.shstrtab`; its `sh_name`.
    pub fn add_name(&mut self, name: &str) -> u32 {
        let i = self.shstrndx();
        let mut d = self.data(i);
        let at = d.len() as u32;
        d.extend_from_slice(name.as_bytes());
        d.push(0);
        self.set_data(i, &d);
        at
    }

    /// Appends a section; its index.
    pub fn add(&mut self, s: &Sh, d: &[u8]) -> usize {
        let n = self.shnum();
        let table = self.b[self.shoff()..self.shoff() + n * self.shentsize()].to_vec();
        while self.b.len() % 8 != 0 {
            self.b.push(0);
        }
        let at = self.b.len();
        self.b.extend_from_slice(&table);
        self.b.extend(std::iter::repeat_n(0u8, self.shentsize()));
        self.set_shoff(at);
        self.set_shnum(n + 1);
        let mut s = s.clone();
        s.size = 0;
        s.offset = 0;
        self.set_sh(n, &s);
        if s.ty != 8 {
            self.set_data(n, d);
        } else {
            let mut x = self.sh(n);
            x.size = d.len() as u64;
            self.set_sh(n, &x);
        }
        n
    }

    // ---- symbols ----
    pub fn symtab(&self) -> usize {
        (1..self.shnum()).find(|&i| self.sh(i).ty == SHT_SYMTAB).expect(".symtab")
    }

    fn sym_size(&self) -> usize {
        if self.c64 { 24 } else { 16 }
    }

    pub fn nsyms(&self) -> usize {
        self.sh(self.symtab()).size as usize / self.sym_size()
    }

    fn sym_at(&self, k: usize) -> usize {
        self.sh(self.symtab()).offset as usize + k * self.sym_size()
    }

    pub fn sym(&self, k: usize) -> Sym {
        let a = self.sym_at(k);
        if self.c64 {
            Sym {
                name: self.rd(a, 4) as u32,
                info: self.b[a + 4],
                other: self.b[a + 5],
                shndx: self.rd(a + 6, 2) as u16,
                value: self.rd(a + 8, 8),
                size: self.rd(a + 16, 8),
            }
        } else {
            Sym {
                name: self.rd(a, 4) as u32,
                value: self.rd(a + 4, 4),
                size: self.rd(a + 8, 4),
                info: self.b[a + 12],
                other: self.b[a + 13],
                shndx: self.rd(a + 14, 2) as u16,
            }
        }
    }

    pub fn set_sym(&mut self, k: usize, y: &Sym) {
        let a = self.sym_at(k);
        if self.c64 {
            self.wr(a, 4, u64::from(y.name));
            self.b[a + 4] = y.info;
            self.b[a + 5] = y.other;
            self.wr(a + 6, 2, u64::from(y.shndx));
            self.wr(a + 8, 8, y.value);
            self.wr(a + 16, 8, y.size);
        } else {
            self.wr(a, 4, u64::from(y.name));
            self.wr(a + 4, 4, y.value);
            self.wr(a + 8, 4, y.size);
            self.b[a + 12] = y.info;
            self.b[a + 13] = y.other;
            self.wr(a + 14, 2, u64::from(y.shndx));
        }
    }

    /// The file offset of symbol `k`'s name in `.strtab`.
    pub fn sym_name_at(&self, k: usize) -> usize {
        let st = self.sh(self.sh(self.symtab()).link as usize);
        st.offset as usize + self.sym(k).name as usize
    }

    pub fn sym_name(&self, k: usize) -> String {
        self.cstr(self.sym_name_at(k))
    }

    pub fn sym_by_name(&self, name: &str) -> Option<usize> {
        (1..self.nsyms()).find(|&k| self.sym_name(k) == name)
    }

    /// The `STT_SECTION` symbol of section `i`.
    pub fn section_sym(&self, i: usize) -> Option<usize> {
        (1..self.nsyms()).find(|&k| {
            let y = self.sym(k);
            y.ty() == STT_SECTION && y.shndx as usize == i
        })
    }

    pub fn first_global(&self) -> usize {
        self.sh(self.symtab()).info as usize
    }

    /// Swaps symbols `a` and `b`, and every reference to them: the
    /// relocation records and the groups' `sh_info`.
    pub fn swap_syms(&mut self, a: usize, b: usize) {
        let (ya, yb) = (self.sym(a), self.sym(b));
        self.set_sym(a, &yb);
        self.set_sym(b, &ya);
        for i in 1..self.shnum() {
            let s = self.sh(i);
            if s.ty == SHT_GROUP && (s.info as usize == a || s.info as usize == b) {
                let mut s2 = s.clone();
                s2.info = if s.info as usize == a { b as u32 } else { a as u32 };
                self.set_sh(i, &s2);
            }
            if s.ty == SHT_RELA || s.ty == SHT_REL {
                let mut rs = self.rels(i);
                let mut changed = false;
                for r in &mut rs {
                    if r.sym as usize == a {
                        r.sym = b as u32;
                        changed = true;
                    } else if r.sym as usize == b {
                        r.sym = a as u32;
                        changed = true;
                    }
                }
                if changed {
                    self.set_rels(i, &rs);
                }
            }
        }
    }

    // ---- relocation records ----
    pub fn rels(&self, i: usize) -> Vec<R> {
        let s = self.sh(i);
        let rela = s.ty == SHT_RELA;
        let w = self.w();
        let es = if rela { 3 * w } else { 2 * w };
        let d = self.data(i);
        let e = Ed { b: d, c64: self.c64 };
        (0..e.b.len() / es)
            .map(|k| {
                let a = k * es;
                let offset = e.rd(a, w);
                let info = e.rd(a + w, w);
                let (sym, ty) = if self.c64 { ((info >> 32) as u32, info as u32) } else { ((info >> 8) as u32, (info & 0xff) as u32) };
                let addend = rela.then(|| e.rd(a + 2 * w, w) as i64);
                let addend = addend.map(|v| if self.c64 { v } else { i64::from(v as i32) });
                R { offset, sym, ty, addend }
            })
            .collect()
    }

    pub fn set_rels(&mut self, i: usize, rs: &[R]) {
        let w = self.w();
        let mut e = Ed { b: Vec::new(), c64: self.c64 };
        for r in rs {
            let at = e.b.len();
            let es = if r.addend.is_some() { 3 * w } else { 2 * w };
            e.b.extend(std::iter::repeat_n(0u8, es));
            e.wr(at, w, r.offset);
            let info = if self.c64 { (u64::from(r.sym) << 32) | u64::from(r.ty) } else { (u64::from(r.sym) << 8) | u64::from(r.ty & 0xff) };
            e.wr(at + w, w, info);
            if let Some(a) = r.addend {
                e.wr(at + 2 * w, w, a as u64);
            }
        }
        self.set_data(i, &e.b);
    }

    /// The relocation sections whose `sh_info` is `i`.
    pub fn relocs_of(&self, i: usize) -> Vec<usize> {
        (1..self.shnum())
            .filter(|&r| matches!(self.sh(r).ty, SHT_RELA | SHT_REL) && self.sh(r).info as usize == i)
            .collect()
    }

    /// Every relocation section.
    pub fn reloc_secs(&self) -> Vec<usize> {
        (1..self.shnum()).filter(|&r| matches!(self.sh(r).ty, SHT_RELA | SHT_REL)).collect()
    }
}
