//! `cargo xtask meta`: the test producer and the dumper of Mason metadata
//! containers (`spec/meta.md` §12, "The test producer"). A front end to
//! `mortar_meta`'s writer and reader: the format has one implementation.
//!
//!   cargo xtask meta write <description> -o <out.s>
//!   cargo xtask meta write <description> --bin <out>        (one section's bytes)
//!   cargo xtask meta write <description> --object <out.o> --target <arch>
//!   cargo xtask meta write <description> --into <in.o> -o <out.o>   (objcopy)
//!   cargo xtask meta write <description> --ll <out.ll>                (LLVM IR, WP-099)
//!   cargo xtask meta write <description> --into-ll <in.ll> -o <out.ll>
//!   cargo xtask meta dump <object>
//!   cargo xtask meta table <rows> --into <in.so> -o <out.so>    (`.note.mason`)
//!
//! `<arch>` is `x86_64`, `aarch64` or `i386`.
//!
//! The IR forms (WP-099) write the same bytes as LLVM IR for a code
//! generator (hod, llc): one `private constant [N x i8] c"...", section
//! ".mason.meta", align 8, !exclude !{}` per section of the description,
//! in `@llvm.compiler.used`; `--into-ll` adds them to a module (kiln's or
//! clang's `-S -emit-llvm`), merging its `@llvm.compiler.used`. A code
//! generator puts them all into one section ([`ir_module`]).
//!
//! # The description
//!
//! One directive per line; `#` starts a comment; fields are `name=value`
//! in any order. A value is a number (decimal or `0x` hex), a string (a
//! bare word, or `"..."` with `\\`, `\"` and `\xNN`), or hex bytes. A
//! field left out is zero (or empty); `details` of `type` defaults to
//! "none" (`0xffffffff`).
//!
//! ```text
//! section                         # a new .mason.meta section (MT-4); the first is implied
//! unit producer=4 flags=0         # a new unit (the first is implied, producer 4)
//! pad 2                           # two zero words before the next unit (MT-10)
//! summary name=S value=S flags=N
//! definition name=S|anchor=S key=N hash=HEX class=N flags=N module=S file=S line=N
//! type name=S key=N hash=HEX layout=N size=N align=N flags=N file=S line=N details=N
//! declaration name=S|anchor=S key=N ty=N size=N type_name=S flags=N
//! details hex=HEX                 # kind 4's body, as given
//! raw kind=N version=N row_size=N row_count=N hex=HEX   # any block, as given
//! level detail                    # this unit writes kind 4 (`default`: it does not)
//! ```
//!
//! # Layout details (kind 4, `spec/odr.md` O-19; WP-145)
//!
//! Item lines after a `type` describe that type's layout, in O-18's order
//! (O-19: the order the items stand in). Offsets are in BITS, as O-19
//! writes them:
//!
//! ```text
//! member name=S type=S offset=N [width=N]   # width= makes a bit-field
//! member offset=N nested=N         # an unnamed class-type member: N items follow (O-19)
//! base type=S offset=N [virtual=1]
//! vptr offset=N
//! slot name=S index=N              # a vtable slot: the final overrider's declaration
//! enumerator name=S value=N      # N may be negative: two's complement (O-19)
//! underlying type=S [scoped=1] [fixed=1]   # O-18's bits in the value (kiln-odr-1, from .d5; WP-204)
//! item flags=N name=S type=S value=N width=N   # any item, as given (kind in bits 0-3)
//! ```
//!
//! A type with items gets the layout hash computed from them (unless it
//! says `layout=`), so that kind 3's hash and kind 4's items cannot
//! disagree: FNV-1a 64 (MT-13's) over `size` (u32 LE), `align` (u16 LE),
//! then per item `flags`, `value`, `width` (LE) and `name` and `type`, each
//! followed by a NUL. The test producer's hash: kiln's is its own (O-18).
//! The unit writes the items as kind 4 entries only at the `detail` level
//! (`level detail`, kiln's `-fmason-metadata=detail`); at the default
//! level the same description writes the same kind 3 rows, without
//! details. The dump prints a canonical kind 4 block as `level detail` and
//! item lines (a type row then shows its `layout=`), any other one raw.
//!
//! `name=S` sets the key to FNV-1a 64 of `S` (MT-13) unless `key=` says
//! otherwise; `anchor=S` writes an anchored row (name 0) whose key is that
//! of `S`; a row with neither is anchored with the `key=` given.
//!
//! The dump is a description: `meta write` of a dump gives the bytes that
//! were dumped, for every unit the writer could have written (a canonical
//! unit, MT-38). The reader's findings are `#` lines.
//!
//! # A shared object's table (`spec/odr.md` O-50 to O-52; WP-075)
//!
//! `meta table` fills the `.note.mason` table of a shared object that was
//! linked with a placeholder note (owner `MasonMeta`, type `0x4d4f4401`,
//! a header and room for the rows: `.long 1, N, 24, 0` and `.zero 24*N`
//! after the note header), so that the table sits in a `PT_NOTE` as a
//! mortar-made library's would. One row per line:
//!
//! ```text
//! row name=S version=V hash=HEX16 flags=N
//! ```
//!
//! The row's `.dynsym` index is that of the defined symbol `S` whose
//! version node is `V` (`version=` left out: the definition of `S`, which
//! must be the only one); the key is FNV-1a 64 of `S` (O-51); `hash` is the
//! 8 bytes written (16 hex digits); `flags` are O-51's (`0x8000` is
//! `NONPREEMPTIBLE`). The header's count becomes the number of rows.
//!
//! `--odr-table=full`'s second note (type `0x4d4f4402`, WP-076) is filled
//! the same way, from a placeholder of that type (`.long 1, 0, 32, 0` and
//! `.zero 32*N`), by these lines:
//!
//! ```text
//! hidden name=S hash=HEX16 flags=N
//! type name=ENC hash=HEX16 layout=N size=N align=N flags=N
//! ```
//!
//! `hidden`: a definition the library does not export (the key is FNV-1a
//! 64 of `S`; no `.dynsym` entry is looked up); `type`: a kind 3 type of
//! encoding `ENC` (its key FNV-1a 64 of `ENC`). A description with either
//! needs the second placeholder; one without leaves it as it is.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mortar_meta::read::{BlockHeader, FileReader, Finding, SectionAttrs, Unit};
use mortar_meta::write::{Declaration, Definition, Detail, RawBlock, Summary, Type};
use mortar_meta::{UnitBuilder, kind, producer};

pub const USAGE: &str = "usage: cargo xtask meta write <description> (-o <out.s> | --bin <out> | \
                         --object <out.o> --target x86_64|aarch64|i386 | --into <in.o> -o <out.o> |\n                                            \
                         --ll <out.ll> | --into-ll <in.ll> -o <out.ll>)\n       \
                         cargo xtask meta dump <object>\n       \
                         cargo xtask meta table <rows> --into <in.so> -o <out.so>\n";

/// One item of a section: a unit, or zero words before the next one.
#[derive(Debug, Clone)]
pub enum Item {
    Unit(UnitBuilder),
    Pad(usize),
}

/// The sections a description gives, each a sequence of items.
pub type Sections = Vec<Vec<Item>>;

/// Parses a description (the module's grammar).
///
/// # Errors
/// The line and what is wrong with it.
#[allow(clippy::too_many_lines)] // one arm per directive, read top to bottom
pub fn parse(text: &str) -> Result<Sections, String> {
    let mut sections: Sections = vec![Vec::new()];
    // Whether the current section's last unit was opened by `unit` or by
    // a row (rows after a `section` or `pad` open a new implicit unit).
    let mut open = false;
    // The open unit's level and which of its types gave `layout=`
    // (WP-145): applied when the unit closes.
    let mut state = Level::default();
    for (n, line) in text.lines().enumerate() {
        let at = |e: String| format!("line {}: {e}", n + 1);
        let words = split(line).map_err(at)?;
        let Some((directive, rest)) = words.split_first() else { continue };
        let items = sections.last_mut().expect("one section at least");
        if directive == b"pad" {
            let [count] = rest else { return Err(at("pad takes one count".into())) };
            let count = number(count).map_err(at)?;
            state.close(items, &mut open);
            items.push(Item::Pad(usize::try_from(count).map_err(|e| at(e.to_string()))?));
            continue;
        }
        if directive == b"level" {
            state.detail = match rest {
                [w] if w == b"detail" => true,
                [w] if w == b"default" => false,
                _ => return Err(at("level is 'level detail' or 'level default'".into())),
            };
            unit(items, &mut open);
            continue;
        }
        let fields = Fields::parse(rest).map_err(at)?;
        if let Some(detail) = detail_item(directive, &fields).map_err(at)? {
            let u = unit(items, &mut open);
            let Some(t) = u.types.last_mut() else {
                return Err(at("a layout item before any type".into()));
            };
            t.items.get_or_insert_with(Vec::new).push(detail);
            continue;
        }
        match directive.as_slice() {
            b"section" => {
                fields.done(&[]).map_err(at)?;
                state.close(items, &mut open);
                sections.push(Vec::new());
            }
            b"unit" => {
                let mut u = UnitBuilder::new(producer::TEST);
                u.producer = fields.num("producer", u64::from(producer::TEST)).map_err(at)?;
                fields.done(&["producer"]).map_err(at)?;
                state.close(items, &mut open);
                items.push(Item::Unit(u));
                open = true;
            }
            b"summary" => {
                let row = Summary {
                    name: fields.string("name"),
                    value: fields.string("value"),
                    flags: fields.num("flags", 0).map_err(at)?,
                };
                fields.done(&["name", "value", "flags"]).map_err(at)?;
                unit(items, &mut open).summary.push(row);
            }
            b"definition" => {
                let (key, name) = fields.keyed().map_err(at)?;
                let row = Definition {
                    key,
                    name,
                    hash: fields.hash().map_err(at)?,
                    class: fields.num("class", 0).map_err(at)?,
                    flags: fields.num("flags", 0).map_err(at)?,
                    module: fields.string("module"),
                    file: fields.string("file"),
                    line: fields.num("line", 0).map_err(at)?,
                };
                fields
                    .done(&[
                        "name", "anchor", "key", "hash", "class", "flags", "module", "file", "line",
                    ])
                    .map_err(at)?;
                unit(items, &mut open).definitions.push(row);
            }
            b"type" => {
                let name = fields.string("name");
                let key = match fields.get("key") {
                    Some(k) => number(k).map_err(at)?,
                    None => mortar_meta::fnv1a64(&name),
                };
                if fields.get("layout").is_some() {
                    state.explicit.push(unit(items, &mut open).types.len());
                }
                let row = Type {
                    items: None,
                    key,
                    name,
                    hash: fields.hash().map_err(at)?,
                    layout: fields.num("layout", 0).map_err(at)?,
                    size: fields.num("size", 0).map_err(at)?,
                    align: fields.num("align", 0).map_err(at)?,
                    flags: fields.num("flags", 0).map_err(at)?,
                    file: fields.string("file"),
                    line: fields.num("line", 0).map_err(at)?,
                    details: fields
                        .num("details", u64::from(mortar_meta::NO_DETAILS))
                        .map_err(at)?,
                };
                fields
                    .done(&[
                        "name", "key", "hash", "layout", "size", "align", "flags", "file", "line",
                        "details",
                    ])
                    .map_err(at)?;
                unit(items, &mut open).types.push(row);
            }
            b"declaration" => {
                let (key, name) = fields.keyed().map_err(at)?;
                let row = Declaration {
                    key,
                    name,
                    ty: fields.num("ty", 0).map_err(at)?,
                    size: fields.num("size", 0).map_err(at)?,
                    type_name: fields.string("type_name"),
                    flags: fields.num("flags", 0).map_err(at)?,
                };
                fields
                    .done(&["name", "anchor", "key", "ty", "size", "type_name", "flags"])
                    .map_err(at)?;
                unit(items, &mut open).declarations.push(row);
            }
            b"details" => {
                let body = fields.bytes("hex").map_err(at)?;
                fields.done(&["hex"]).map_err(at)?;
                let u = unit(items, &mut open);
                if u.details.is_some() {
                    return Err(at("kind 4 given twice in one unit".into()));
                }
                u.details = Some(body);
            }
            b"raw" => {
                let block = RawBlock {
                    kind: fields.num("kind", 0).map_err(at)?,
                    version: fields.num("version", 1).map_err(at)?,
                    row_size: fields.num("row_size", 0).map_err(at)?,
                    row_count: fields.num("row_count", 0).map_err(at)?,
                    body: fields.bytes("hex").map_err(at)?,
                };
                fields.done(&["kind", "version", "row_size", "row_count", "hex"]).map_err(at)?;
                unit(items, &mut open).raw.push(block);
            }
            other => {
                return Err(at(format!("unknown directive '{}'", String::from_utf8_lossy(other))));
            }
        }
    }
    let items = sections.last_mut().expect("one section at least");
    state.close(items, &mut open);
    Ok(sections)
}

/// The open unit's kind 4 state while a description is parsed (WP-145).
#[derive(Debug, Default)]
struct Level {
    /// `level detail`: the unit writes its items as kind 4.
    detail: bool,
    /// The unit's types (by index) that gave `layout=`.
    explicit: Vec<usize>,
}

impl Level {
    /// Closes the open unit, if any: layout hashes from the items where
    /// `layout=` was not given; the items dropped below `detail`.
    fn close(&mut self, items: &mut [Item], open: &mut bool) {
        if *open && let Some(Item::Unit(u)) = items.last_mut() {
            for (i, t) in u.types.iter_mut().enumerate() {
                let explicit = self.explicit.contains(&i);
                if let Some(list) = &t.items
                    && !explicit
                {
                    t.layout = layout_hash(t.size, t.align, list);
                }
                if !self.detail {
                    t.items = None;
                }
            }
        }
        *open = false;
        *self = Self::default();
    }
}

/// The test producer's layout hash (the module's doc): FNV-1a 64 over the
/// size, the alignment and the items.
pub fn layout_hash(size: u32, align: u16, items: &[Detail]) -> u64 {
    let mut b = Vec::with_capacity(6 + items.len() * 32);
    b.extend_from_slice(&size.to_le_bytes());
    b.extend_from_slice(&align.to_le_bytes());
    for it in items {
        b.extend_from_slice(&it.flags.to_le_bytes());
        b.extend_from_slice(&it.value.to_le_bytes());
        b.extend_from_slice(&it.width.to_le_bytes());
        b.extend_from_slice(&it.name);
        b.push(0);
        b.extend_from_slice(&it.ty);
        b.push(0);
    }
    mortar_meta::fnv1a64(&b)
}

/// A layout item line (the module's doc), or `None` for another directive.
fn detail_item(directive: &[u8], fields: &Fields) -> Result<Option<Detail>, String> {
    use mortar_meta::details::item;
    let d = |kind: u32, known: &[&str]| -> Result<Detail, String> {
        fields.done(known)?;
        Ok(Detail { flags: kind, ..Detail::default() })
    };
    let out = match directive {
        b"member" => {
            let mut x = d(item::MEMBER, &["name", "type", "offset", "width", "nested"])?;
            x.name = fields.string("name");
            x.ty = fields.string("type");
            x.value = fields.num("offset", 0)?;
            if fields.get("width").is_some() {
                x.width = fields.num("width", 0)?;
                x.flags |= item::BITFIELD;
            } else if fields.get("nested").is_some() {
                // O-19 (WP-151): the nested-item count of an unnamed
                // class-type member, in `width`, without BITFIELD.
                x.width = fields.num("nested", 0)?;
            }
            x
        }
        b"base" => {
            let mut x = d(item::BASE, &["type", "offset", "virtual"])?;
            x.ty = fields.string("type");
            x.value = fields.num("offset", 0)?;
            if fields.num::<u64>("virtual", 0)? != 0 {
                x.flags |= item::VIRTUAL;
            }
            x
        }
        b"vptr" => {
            let mut x = d(item::VPTR, &["offset"])?;
            x.value = fields.num("offset", 0)?;
            x
        }
        b"slot" => {
            let mut x = d(item::SLOT, &["name", "index"])?;
            x.name = fields.string("name");
            x.value = fields.num("index", 0)?;
            x
        }
        b"enumerator" => {
            let mut x = d(item::ENUMERATOR, &["name", "value"])?;
            x.name = fields.string("name");
            x.value = match fields.get("value").and_then(|v| v.strip_prefix(b"-")) {
                Some(neg) => {
                    let n = number(neg)?;
                    if n > 1 << 63 {
                        return Err("an enumerator value below -2^63".to_owned());
                    }
                    0u64.wrapping_sub(n)
                }
                None => fields.num("value", 0)?,
            };
            x
        }
        b"underlying" => {
            let mut x = d(item::UNDERLYING, &["type", "scoped", "fixed"])?;
            x.ty = fields.string("type");
            let scoped: u64 = fields.num("scoped", 0)?;
            let fixed: u64 = fields.num("fixed", 0)?;
            if scoped > 1 || fixed > 1 {
                return Err("scoped= and fixed= are 0 or 1".to_owned());
            }
            x.value = (scoped * item::SCOPED) | (fixed * item::FIXED);
            x
        }
        b"item" => {
            fields.done(&["flags", "name", "type", "value", "width"])?;
            Detail {
                name: fields.string("name"),
                ty: fields.string("type"),
                value: fields.num("value", 0)?,
                width: fields.num("width", 0)?,
                flags: fields.num("flags", 0)?,
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(out))
}

/// The unit rows go to: the open one, or a new implicit one.
fn unit<'a>(items: &'a mut Vec<Item>, open: &mut bool) -> &'a mut UnitBuilder {
    if !*open {
        items.push(Item::Unit(UnitBuilder::new(producer::TEST)));
        *open = true;
    }
    match items.last_mut() {
        Some(Item::Unit(u)) => u,
        _ => unreachable!("an open unit is the last item"),
    }
}

/// The bytes of one section: its units, written by `mortar_meta`, and its
/// padding.
///
/// # Errors
/// The writer's error, with the unit's place.
pub fn section_bytes(items: &[Item]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        match item {
            Item::Unit(u) => out.extend(u.finish().map_err(|e| format!("unit {i}: {e}"))?),
            Item::Pad(words) => out.resize(out.len() + 8 * words, 0),
        }
    }
    Ok(out)
}

/// GNU as source for the sections: one `.mason.meta` per section (MT-1:
/// `SHT_PROGBITS`, `SHF_EXCLUDE` alone, aligned to 8), distinct by
/// `unique` when there are several (MT-4).
///
/// # Errors
/// [`section_bytes`]'s.
pub fn assembly(sections: &Sections) -> Result<String, String> {
    let mut s = String::from("# Mason metadata: cargo xtask meta write\n");
    for (i, items) in sections.iter().enumerate() {
        let bytes = section_bytes(items)?;
        let unique = if sections.len() > 1 { format!(",unique,{}", i + 1) } else { String::new() };
        let _ = writeln!(s, "\t.section .mason.meta,\"e\",%progbits{unique}\n\t.p2align 3");
        for line in bytes.chunks(16) {
            let row: Vec<String> = line.iter().map(|b| format!("0x{b:02x}")).collect();
            let _ = writeln!(s, "\t.byte {}", row.join(","));
        }
    }
    // An executable stack is not what the test objects mean to ask for.
    s.push_str("\t.section .note.GNU-stack,\"\",%progbits\n");
    Ok(s)
}

/// The IR global of the `i`-th section of a description.
fn ir_name(i: usize) -> String {
    if i == 0 { IR_GLOBAL.to_owned() } else { format!("{IR_GLOBAL}.{i}") }
}

/// The name of the container's IR global (`spec/odr.md` §11, K-1: the
/// spelling is kiln's to choose; this is the test producer's).
pub const IR_GLOBAL: &str = "@__mason_meta";

/// `bytes` as an LLVM IR string constant's body (`c"..."`): a printable
/// ASCII byte other than `"` and `\` as itself, every other byte as `\XX`
/// (LLVM's own printer, `printEscapedString`). Exact for every byte value.
pub fn c_string(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() + 2);
    for &b in bytes {
        if (b' '..=b'~').contains(&b) && b != b'"' && b != b'\\' {
            s.push(char::from(b));
        } else {
            let _ = write!(s, "\\{b:02X}");
        }
    }
    s
}

/// The IR globals of the sections (WP-099), in hod's accepted shape
/// (hod `spec/codegen.md` §9.7.4; `spec/odr.md` §11, H-1): each
/// `private constant [N x i8] c"...", section ".mason.meta", align 8,
/// !exclude !{}`, and their names for `@llvm.compiler.used`. A code
/// generator puts variables of one section name and flags into ONE
/// section, each at its alignment: several sections of a description
/// become one `.mason.meta` whose bytes are theirs concatenated (each is a
/// multiple of 8 bytes, MT-10, so nothing is inserted between them), which
/// a reader reads as the same sequence of units (MT-4).
///
/// # Errors
/// [`section_bytes`]'s.
fn ir_globals(sections: &Sections) -> Result<(String, Vec<String>), String> {
    let mut text = String::new();
    let mut names = Vec::new();
    for (i, items) in sections.iter().enumerate() {
        let bytes = section_bytes(items)?;
        let name = ir_name(i);
        let _ = writeln!(
            text,
            "{name} = private constant [{} x i8] c\"{}\", section \".mason.meta\", align 8, \
             !exclude !{{}}",
            bytes.len(),
            c_string(&bytes),
        );
        names.push(name);
    }
    Ok((text, names))
}

/// An LLVM IR module holding only the sections' globals and
/// `@llvm.compiler.used` (without it `opt -O2` deletes them, `spec/odr.md`
/// K-1). It names no target: hod is given `-mtriple`.
///
/// # Errors
/// [`section_bytes`]'s.
pub fn ir_module(sections: &Sections) -> Result<String, String> {
    let (globals, names) = ir_globals(sections)?;
    let used: Vec<String> = names.iter().map(|n| format!("ptr {n}")).collect();
    Ok(format!(
        "; Mason metadata: cargo xtask meta write --ll\n{globals}@llvm.compiler.used = appending \
         global [{} x ptr] [{}], section \"llvm.metadata\"\n",
        used.len(),
        used.join(", ")
    ))
}

/// The IR module `module` with the sections' globals added (the IR form of
/// `--into`): the globals after the module's text, their names added to
/// the module's `@llvm.compiler.used` (or one written).
///
/// # Errors
/// [`section_bytes`]'s; the module already has a global of the name; its
/// `@llvm.compiler.used` is not one line of LLVM's printed form
/// (`appending global [K x ptr] [ptr @a, ...], section "llvm.metadata"`).
pub fn into_ir(module: &str, sections: &Sections) -> Result<String, String> {
    let (globals, names) = ir_globals(sections)?;
    for n in &names {
        let taken = module
            .lines()
            .any(|l| l.strip_prefix(n.as_str()).is_some_and(|r| r.trim_start().starts_with('=')));
        if taken {
            return Err(format!("the module already defines {n}"));
        }
    }
    let mut out = String::with_capacity(module.len() + globals.len() + 128);
    let mut merged = false;
    for line in module.lines() {
        if let Some(rest) = line.strip_prefix("@llvm.compiler.used = appending global [") {
            out.push_str(&merge_used(rest, &names).ok_or_else(|| {
                format!("@llvm.compiler.used is not in the form this writer knows: {line}")
            })?);
            merged = true;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out.push_str("\n; Mason metadata: cargo xtask meta write --into-ll\n");
    out.push_str(&globals);
    if !merged {
        let used: Vec<String> = names.iter().map(|n| format!("ptr {n}")).collect();
        let _ = writeln!(
            out,
            "@llvm.compiler.used = appending global [{} x ptr] [{}], section \"llvm.metadata\"",
            used.len(),
            used.join(", ")
        );
    }
    Ok(out)
}

/// `rest` (after `@llvm.compiler.used = appending global [`) with `names`
/// added, or `None` if it is not `K x ptr] [<list>], section "llvm.metadata"`.
fn merge_used(rest: &str, names: &[String]) -> Option<String> {
    let (count, tail) = rest.split_once(" x ptr] [")?;
    let count: usize = count.parse().ok()?;
    let (list, tail) = tail.split_once(']')?;
    if tail != ", section \"llvm.metadata\"" {
        return None;
    }
    let mut items: Vec<String> = list.split(", ").map(str::to_owned).collect();
    if items.len() != count || items.iter().any(|i| !i.starts_with("ptr @")) {
        return None;
    }
    items.extend(names.iter().map(|n| format!("ptr {n}")));
    Some(format!(
        "@llvm.compiler.used = appending global [{} x ptr] [{}], section \"llvm.metadata\"",
        items.len(),
        items.join(", ")
    ))
}

/// The text dump of every container of the ELF object `bytes` (MT-4: in
/// section-index order, one reader for the file, so that MT-35's report
/// comes once). A section named `.mason.meta` that is not a container
/// (MT-6) is a `#` line.
///
/// # Errors
/// The object cannot be parsed.
pub fn dump(bytes: &[u8]) -> Result<String, String> {
    let object = mortar_elf::Object::parse(bytes).map_err(|e| format!("not an object: {e}"))?;
    let mut reader = FileReader::new();
    let mut out = String::new();
    let mut first = true;
    let mut index = Vec::new();
    for s in object.sections() {
        let name = s.name().map_err(|e| e.to_string())?;
        if name != mortar_meta::SECTION_NAME {
            continue;
        }
        let i = s.index().0;
        if !mortar_meta::is_container(name, s.sh_type(), s.flags()) {
            let _ = writeln!(
                out,
                "# section {i}: not a container (sh_type {:#x}, sh_flags {:#x}; MT-6)",
                s.sh_type(),
                s.flags()
            );
            continue;
        }
        let h = s.header();
        let attrs = SectionAttrs {
            index: i,
            addralign: h.sh_addralign,
            entsize: h.sh_entsize,
            link: h.sh_link,
            info: h.sh_info,
        };
        reader.section(attrs, s.data().map_err(|e| e.to_string())?);
        index.push(i);
    }
    let mut units = reader.units.iter().peekable();
    let mut findings = reader.findings.iter().peekable();
    for &i in &index {
        if !first {
            out.push_str("section\n");
        }
        first = false;
        let _ = writeln!(out, "# section {i}");
        // Units and findings of this section, by offset.
        loop {
            let u = units.peek().filter(|u| u.section == i).map(|u| u.offset);
            let f = findings.peek().filter(|f| f.section() == i).map(|f| finding_at(f));
            match (u, f) {
                (Some(a), Some(b)) if b < a => print_finding(&mut out, findings.next().unwrap()),
                (Some(_), _) => print_unit(&mut out, units.next().unwrap()),
                (None, Some(_)) => print_finding(&mut out, findings.next().unwrap()),
                (None, None) => break,
            }
        }
    }
    Ok(out)
}

fn finding_at(f: &Finding) -> u64 {
    match f {
        Finding::Malformed { unit, .. } => unit.unwrap_or(0),
        Finding::Version { unit, .. } => *unit,
    }
}

fn print_finding(out: &mut String, f: &Finding) {
    match f {
        Finding::Version { unit, major, .. } => {
            let _ = writeln!(out, "# meta-version: unit at offset {unit}: major {major}");
        }
        Finding::Malformed { .. } => {
            let _ = writeln!(out, "# meta-malformed: {}", f.reason());
        }
    }
}

/// Whether a known kind's block is the one this writer writes (version 1,
/// the v1 row size); a later version is dumped raw, so that writing the
/// dump back gives its bytes.
fn as_written(b: &BlockHeader) -> bool {
    b.version == 1 && kind::row_size(b.kind).is_some_and(|r| r == b.row_size)
}

/// One unit as a description: a kind 4 block the writer would write from
/// typed items is printed as `level detail` and item lines (WP-145), when
/// writing that text back gives the unit's bytes; otherwise raw.
fn print_unit(out: &mut String, u: &Unit<'_>) {
    let mut text = String::new();
    if unit_text(&mut text, u, true) && reproduces(&text, u) {
        out.push_str(&text);
        return;
    }
    text.clear();
    unit_text(&mut text, u, false);
    out.push_str(&text);
}

/// Whether the description `text` of one unit writes `u`'s bytes.
fn reproduces(text: &str, u: &Unit<'_>) -> bool {
    let Ok(sections) = parse(text) else { return false };
    match sections.as_slice() {
        [items] => match items.as_slice() {
            [Item::Unit(b)] => b.finish().is_ok_and(|bytes| bytes == u.bytes()),
            _ => false,
        },
        _ => false,
    }
}

/// `u` as a description; with `typed`, kind 4 as items (false when the
/// unit has no version 1 kind 4 block or an entry does not read).
fn unit_text(out: &mut String, u: &Unit<'_>, typed: bool) -> bool {
    let typed = typed && u.version(kind::DETAILS) == Some(1);
    let _ = writeln!(
        out,
        "# unit at offset {}: {} bytes, minor {}, header_size {}, flags {:#x}",
        u.offset,
        u.bytes().len(),
        u.minor,
        u.header_size,
        u.flags
    );
    let _ = writeln!(out, "unit producer={}", u.producer);
    if typed {
        out.push_str("level detail\n");
    }
    for b in u.blocks() {
        let known = kind::row_size(b.kind).is_some();
        if known && as_written(&b) {
            if !print_rows(out, u, b.kind, typed) {
                return false;
            }
            continue;
        }
        let body = u
            .bytes()
            .get((b.at + mortar_meta::BLOCK_HEADER_SIZE) as usize..(b.at + b.block_size) as usize)
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "raw kind={:#06x} version={} row_size={} row_count={} hex={}",
            b.kind,
            b.version,
            b.row_size,
            b.row_count,
            hex(body)
        );
    }
    typed
}

/// The item lines of the kind 4 entry at `offset` (the module's doc):
/// false when it does not read.
fn print_items(out: &mut String, u: &Unit<'_>, offset: u32) -> bool {
    use mortar_meta::details::item;
    const VIRTUAL_BASE: u32 = item::BASE | item::VIRTUAL;
    let Ok(Some(entry)) = u.detail_entry(offset) else { return false };
    if entry.reserved != 0 {
        return false;
    }
    // O-19 (WP-151): an enumerator in signed decimal when the underlying
    // type is signed.
    let char_signed = u.summary_value(b"abi.char").and_then(|(v, _)| match v {
        b"signed" => Some(true),
        b"unsigned" => Some(false),
        _ => None,
    });
    let signed = entry
        .all_items()
        .find(|it| it.flags == item::UNDERLYING)
        .and_then(|it| mortar_meta::details::signed_underlying(u.string(it.ty), char_signed))
        == Some(true);
    for it in entry.all_items() {
        let (name, ty) = (quote(u.string(it.name)), quote(u.string(it.ty)));
        let plain = it.name == 0 && it.ty == 0 && it.width == 0;
        let line = match it.flags {
            item::MEMBER if it.width == 0 => {
                format!("member name={name} type={ty} offset={}", it.value)
            }
            item::MEMBER if it.name == 0 && it.ty == 0 => {
                format!("member offset={} nested={}", it.value, it.width)
            }
            f if f == item::MEMBER | item::BITFIELD => {
                format!("member name={name} type={ty} offset={} width={}", it.value, it.width)
            }
            f @ (item::BASE | VIRTUAL_BASE) if it.name == 0 && it.width == 0 => {
                let v = if f == item::BASE { "" } else { " virtual=1" };
                format!("base type={ty} offset={}{v}", it.value)
            }
            item::VPTR if plain => format!("vptr offset={}", it.value),
            item::SLOT if it.ty == 0 && it.width == 0 => {
                format!("slot name={name} index={}", it.value)
            }
            item::ENUMERATOR if it.ty == 0 && it.width == 0 => {
                #[allow(clippy::cast_possible_wrap, reason = "O-19's two's complement")]
                let v = if signed { (it.value as i64).to_string() } else { it.value.to_string() };
                format!("enumerator name={name} value={v}")
            }
            item::UNDERLYING
                if it.name == 0
                    && it.width == 0
                    && it.value & !(item::SCOPED | item::FIXED) == 0 =>
            {
                let mut l = format!("underlying type={ty}");
                if it.value & item::SCOPED != 0 {
                    l.push_str(" scoped=1");
                }
                if it.value & item::FIXED != 0 {
                    l.push_str(" fixed=1");
                }
                l
            }
            _ => format!(
                "item flags={:#x} name={name} type={ty} value={} width={}",
                it.flags, it.value, it.width
            ),
        };
        out.push_str(&line);
        out.push('\n');
    }
    true
}

/// After a kind 4 block dumped raw (its typed form would not write the
/// same bytes: another producer's string or entry order, WP-134), each
/// kind 3 row's entry as comment lines: what the reader reads, or why it
/// does not read.
fn comment_items(out: &mut String, u: &Unit<'_>) {
    for r in u.types().iter() {
        if r.details == mortar_meta::NO_DETAILS {
            continue;
        }
        let name = quote(u.string(r.name));
        match u.detail_entry(r.details) {
            Ok(Some(entry)) => {
                let mut items = String::new();
                if print_items(&mut items, u, r.details) {
                    let _ = writeln!(
                        out,
                        "# details {:#x} ({name}): {} items",
                        r.details,
                        entry.all_items().len()
                    );
                    for line in items.lines() {
                        let _ = writeln!(out, "#   {line}");
                    }
                } else {
                    let _ = writeln!(out, "# details {:#x} ({name}): reserved set", r.details);
                }
            }
            Ok(None) => {}
            Err(e) => {
                let _ = writeln!(out, "# meta-malformed: details {:#x} ({name}): {e}", r.details);
            }
        }
    }
}

fn print_rows(out: &mut String, u: &Unit<'_>, k: u16, typed: bool) -> bool {
    let name_or_anchor = |out: &mut String, name: u32, key: u64| {
        if name == 0 {
            let _ = write!(out, " key={key:#018x}");
        } else {
            let n = u.string(name);
            let _ = write!(out, " name={}", quote(n));
            if mortar_meta::fnv1a64(n) != key {
                let _ = write!(out, " key={key:#018x}");
            }
        }
    };
    match k {
        kind::SUMMARY => {
            for r in u.summary().iter() {
                let _ = writeln!(
                    out,
                    "summary name={} value={} flags={:#x}",
                    quote(u.string(r.name)),
                    quote(u.string(r.value)),
                    r.flags
                );
            }
        }
        kind::DEFINITIONS => {
            for r in u.definitions().iter() {
                out.push_str("definition");
                name_or_anchor(out, r.name, r.key);
                let _ = writeln!(
                    out,
                    " hash={} class={} flags={:#x} module={} file={} line={}",
                    hex(&r.hash),
                    r.class,
                    r.flags,
                    quote(u.string(r.module)),
                    quote(u.string(r.file)),
                    r.line
                );
            }
        }
        kind::TYPES => {
            for r in u.types().iter() {
                let _ = write!(out, "type name={}", quote(u.string(r.name)));
                if mortar_meta::fnv1a64(u.string(r.name)) != r.key {
                    let _ = write!(out, " key={:#018x}", r.key);
                }
                let _ = writeln!(
                    out,
                    " hash={} layout={:#x} size={} align={} flags={:#x} file={} line={} details={:#x}",
                    hex(&r.hash),
                    r.layout,
                    r.size,
                    r.align,
                    r.flags,
                    quote(u.string(r.file)),
                    r.line,
                    r.details
                );
                if typed && r.details != mortar_meta::NO_DETAILS && !print_items(out, u, r.details)
                {
                    return false;
                }
            }
        }
        kind::DETAILS if typed => {}
        kind::DETAILS => {
            let _ = writeln!(out, "details hex={}", hex(u.details().unwrap_or_default()));
            if u.version(kind::DETAILS) == Some(1) {
                comment_items(out, u);
            }
        }
        kind::DECLARATIONS => {
            for r in u.declarations().iter() {
                out.push_str("declaration");
                name_or_anchor(out, r.name, r.key);
                let _ = writeln!(
                    out,
                    " ty={:#018x} size={} type_name={} flags={:#x}",
                    r.ty,
                    r.size,
                    quote(u.string(r.type_name)),
                    r.flags
                );
            }
        }
        _ => {}
    }
    true
}

/// A string as the description writes it: bare when it is a plain word.
fn quote(s: &[u8]) -> String {
    let plain = !s.is_empty()
        && s.iter().all(|&b| b.is_ascii_graphic() && !matches!(b, b'"' | b'\\' | b'#' | b'='));
    if plain {
        return String::from_utf8_lossy(s).into_owned();
    }
    let mut out = String::from("\"");
    for &b in s {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b' '..=b'~' => out.push(char::from(b)),
            _ => {
                let _ = write!(out, "\\x{b:02x}");
            }
        }
    }
    out.push('"');
    out
}

fn hex(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "-".into();
    }
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// A line's words, as bytes: `"..."` is one word (with its escapes);
/// `#` outside quotes ends the line. A `name=` prefix and its quoted value
/// are one word.
fn split(line: &str) -> Result<Vec<Vec<u8>>, String> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b[i] == b'#' {
            break;
        }
        let mut w = Vec::new();
        while i < b.len() && !b[i].is_ascii_whitespace() {
            if b[i] != b'"' {
                w.push(b[i]);
                i += 1;
                continue;
            }
            // Mark the word as quoted (a quoted empty string is a value).
            w.push(0);
            i += 1;
            loop {
                match b.get(i) {
                    None => return Err("unterminated string".into()),
                    Some(b'"') => {
                        i += 1;
                        break;
                    }
                    Some(b'\\') => match b.get(i + 1) {
                        Some(b'x') => {
                            let h = line.get(i + 2..i + 4).ok_or("short \\x escape")?;
                            w.push(u8::from_str_radix(h, 16).map_err(|e| e.to_string())?);
                            i += 4;
                        }
                        Some(&c @ (b'"' | b'\\')) => {
                            w.push(c);
                            i += 2;
                        }
                        _ => return Err("unknown escape".into()),
                    },
                    Some(&c) => {
                        w.push(c);
                        i += 1;
                    }
                }
            }
        }
        out.push(w);
    }
    Ok(out)
}

fn number(w: &[u8]) -> Result<u64, String> {
    let s = std::str::from_utf8(w).map_err(|e| e.to_string())?;
    let r = match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16),
        None => s.parse(),
    };
    r.map_err(|_| format!("'{s}' is not a number"))
}

/// The `name=value` fields of a directive.
struct Fields(Vec<(Vec<u8>, Vec<u8>)>);

impl Fields {
    fn parse(words: &[Vec<u8>]) -> Result<Self, String> {
        let mut out: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for w in words {
            let eq = w
                .iter()
                .position(|&b| b == b'=')
                .ok_or_else(|| format!("'{}' is not name=value", String::from_utf8_lossy(w)))?;
            let (k, v) = (w[..eq].to_vec(), w[eq + 1..].to_vec());
            if out.iter().any(|(x, _)| *x == k) {
                return Err(format!("field '{}' given twice", String::from_utf8_lossy(&k)));
            }
            out.push((k, v));
        }
        Ok(Self(out))
    }

    /// The raw value: a quoted one starts with the 0 [`split`] put there.
    fn get(&self, name: &str) -> Option<&[u8]> {
        self.0.iter().find(|(k, _)| k == name.as_bytes()).map(|(_, v)| v.as_slice())
    }

    fn string(&self, name: &str) -> Vec<u8> {
        match self.get(name) {
            Some([0, rest @ ..]) => rest.to_vec(),
            Some(v) => v.to_vec(),
            None => Vec::new(),
        }
    }

    fn num<T: TryFrom<u64>>(&self, name: &str, default: u64) -> Result<T, String> {
        let v = self.get(name).map_or(Ok(default), number)?;
        T::try_from(v).map_err(|_| format!("{name}={v} does not fit its field"))
    }

    fn bytes(&self, name: &str) -> Result<Vec<u8>, String> {
        let Some(v) = self.get(name) else { return Ok(Vec::new()) };
        if v == b"-" {
            return Ok(Vec::new());
        }
        if v.len() % 2 != 0 {
            return Err(format!("{name}: an odd number of hex digits"));
        }
        v.chunks(2)
            .map(|p| {
                std::str::from_utf8(p)
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or_else(|| format!("{name}: not hex"))
            })
            .collect()
    }

    fn hash(&self) -> Result<[u8; 16], String> {
        let b = self.bytes("hash")?;
        if b.is_empty() {
            return Ok([0; 16]);
        }
        b.try_into().map_err(|_| "hash= takes 16 bytes (32 hex digits)".into())
    }

    /// `name=` / `anchor=` / `key=` of kinds 2 and 5.
    fn keyed(&self) -> Result<(u64, Option<Vec<u8>>), String> {
        let key = self.get("key").map(number).transpose()?;
        match (self.get("name"), self.get("anchor")) {
            (Some(_), Some(_)) => Err("name= and anchor= exclude each other".into()),
            (Some(_), None) => {
                let n = self.string("name");
                Ok((key.unwrap_or_else(|| mortar_meta::fnv1a64(&n)), Some(n)))
            }
            (None, Some(_)) => {
                let a = self.string("anchor");
                Ok((key.unwrap_or_else(|| mortar_meta::fnv1a64(&a)), None))
            }
            (None, None) => Ok((key.unwrap_or(0), None)),
        }
    }

    fn done(&self, known: &[&str]) -> Result<(), String> {
        match self.0.iter().find(|(k, _)| !known.iter().any(|n| n.as_bytes() == k.as_slice())) {
            Some((k, _)) => Err(format!("unknown field '{}'", String::from_utf8_lossy(k))),
            None => Ok(()),
        }
    }
}

/// The assembler of a target (`spec/harness.md`'s three targets).
fn assembler(target: &str) -> Result<(&'static str, &'static [&'static str]), String> {
    match target {
        "x86_64" => Ok(("x86_64-linux-gnu-as", &["--64"])),
        "i386" => Ok(("i686-linux-gnu-as", &["--32"])),
        "aarch64" => Ok(("aarch64-linux-gnu-as", &[])),
        _ => Err(format!("unknown target '{target}' (x86_64, aarch64, i386)")),
    }
}

fn run_tool(name: &str, args: &[String]) -> Result<(), String> {
    let program = crate::mason::tool(name).ok_or_else(|| format!("{name} is not installed"))?;
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let o = crate::mason::run_in(&program, args, &dir, &[], crate::mason::TOOL_LIMITS)?;
    if o.end.success() {
        Ok(())
    } else {
        Err(format!("{name} failed: {}", String::from_utf8_lossy(&o.stderr)))
    }
}

/// Assembles `sections` into the relocatable object `out` for `target`.
///
/// # Errors
/// The writer's, or the assembler's.
pub fn assemble(sections: &Sections, target: &str, out: &Path) -> Result<(), String> {
    let (name, flags) = assembler(target)?;
    let s = out.with_extension("mason.s");
    std::fs::write(&s, assembly(sections)?).map_err(|e| e.to_string())?;
    let mut args: Vec<String> = flags.iter().map(|f| (*f).to_string()).collect();
    args.extend(["-o".into(), out.display().to_string(), s.display().to_string()]);
    let r = run_tool(name, &args);
    let _ = std::fs::remove_file(&s);
    r
}

/// Adds one container holding `items` to the object `input`, as `out`.
/// objcopy (binutils 2.44) gives an added section `SHF_WRITE` unless told
/// `readonly`, and ignores `--set-section-alignment` for a section it adds
/// in the same run: so two runs, the second setting MT-1's alignment.
///
/// The objcopy is the input's own target's (`<triple>-objcopy`, from its
/// `e_machine`): the host's objcopy does not know another target's
/// objects (WP-075: an x86-64 object on an AArch64 host was "file format
/// not recognized").
///
/// # Errors
/// The writer's, or objcopy's.
pub fn add_to(items: &[Item], input: &Path, out: &Path) -> Result<(), String> {
    let bytes = std::fs::read(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let name = objcopy_for(&bytes).map_err(|e| format!("{}: {e}", input.display()))?;
    let program = crate::mason::tool(name).ok_or_else(|| format!("{name} is not installed"))?;
    add_with(&program, items, input, out)
}

/// The objcopy of an ELF file's target, by its `e_machine`.
///
/// # Errors
/// Not ELF, or a machine without a target here.
pub fn objcopy_for(elf: &[u8]) -> Result<&'static str, String> {
    if elf.len() < 20 || elf[..4] != *b"\x7fELF" {
        return Err("not an ELF file".into());
    }
    let machine = match elf[5] {
        1 => u16::from_le_bytes([elf[18], elf[19]]),
        2 => u16::from_be_bytes([elf[18], elf[19]]),
        d => return Err(format!("ELF data encoding {d}")),
    };
    match machine {
        62 => Ok("x86_64-linux-gnu-objcopy"),
        183 => Ok("aarch64-linux-gnu-objcopy"),
        3 => Ok("i686-linux-gnu-objcopy"),
        m => Err(format!("e_machine {m}: no target (x86_64, aarch64, i386)")),
    }
}

/// [`add_to`] with the `objcopy` given (a cross target's own).
///
/// # Errors
/// The writer's, or objcopy's.
pub fn add_with(objcopy: &Path, items: &[Item], input: &Path, out: &Path) -> Result<(), String> {
    let run = |args: &[String]| -> Result<(), String> {
        let dir = std::env::current_dir().map_err(|e| e.to_string())?;
        let o = crate::mason::run_in(objcopy, args, &dir, &[], crate::mason::TOOL_LIMITS)?;
        if o.end.success() {
            Ok(())
        } else {
            Err(format!("objcopy failed: {}", String::from_utf8_lossy(&o.stderr)))
        }
    };
    let bin = out.with_extension("mason.bin");
    let step = out.with_extension("mason.o");
    // An object with a container already (hod's own unit, H-3): the
    // description's unit goes before the bytes there, as hod appends its
    // unit after an incoming container (spec/meta.md MT-20), in the one
    // section objcopy can name.
    let elf = std::fs::read(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let present = mortar_elf::Object::parse(&elf)
        .ok()
        .and_then(|o| {
            o.sections()
                .find(|s| s.name().is_ok_and(|n| n == mortar_meta::SECTION_NAME))
                .map(|s| s.data().map(<[u8]>::to_vec))
        })
        .transpose()
        .map_err(|e| format!("{}: .mason.meta: {e}", input.display()))?;
    let mut bytes = section_bytes(items)?;
    let how = match present {
        Some(old) => {
            bytes.extend_from_slice(&old);
            "--update-section"
        }
        None => "--add-section",
    };
    std::fs::write(&bin, bytes).map_err(|e| e.to_string())?;
    let r = run(&[
        how.into(),
        format!(".mason.meta={}", bin.display()),
        "--set-section-flags".into(),
        ".mason.meta=exclude,readonly".into(),
        input.display().to_string(),
        step.display().to_string(),
    ])
    .and_then(|()| {
        run(&[
            "--set-section-alignment".into(),
            ".mason.meta=8".into(),
            step.display().to_string(),
            out.display().to_string(),
        ])
    });
    let _ = std::fs::remove_file(&bin);
    let _ = std::fs::remove_file(&step);
    r
}

/// One row of a `meta table` description.
struct TableRow {
    name: Vec<u8>,
    version: Option<Vec<u8>>,
    hash: [u8; 8],
    flags: u16,
}

/// A `meta table` description: the first note's rows and `=full`'s.
#[derive(Default)]
struct TableText {
    rows: Vec<TableRow>,
    hidden: Vec<mortar_meta::table::FullRow>,
    types: Vec<mortar_meta::table::FullRow>,
}

fn table_rows(text: &str) -> Result<TableText, String> {
    let mut out = TableText::default();
    for (n, line) in text.lines().enumerate() {
        let at = |e: String| format!("line {}: {e}", n + 1);
        let words = split(line).map_err(at)?;
        let Some((directive, rest)) = words.split_first() else { continue };
        let fields = Fields::parse(rest).map_err(at)?;
        let hash = || -> Result<[u8; 8], String> {
            let hash = fields.bytes("hash").map_err(at)?;
            hash.try_into().map_err(|_| at("hash= takes 8 bytes (16 hex digits)".into()))
        };
        match directive.as_slice() {
            b"row" => {
                fields.done(&["name", "version", "hash", "flags"]).map_err(at)?;
                out.rows.push(TableRow {
                    name: fields.string("name"),
                    version: fields.get("version").map(|_| fields.string("version")),
                    hash: hash()?,
                    flags: fields.num("flags", 0).map_err(at)?,
                });
            }
            b"hidden" => {
                fields.done(&["name", "hash", "flags"]).map_err(at)?;
                out.hidden.push(mortar_meta::table::FullRow {
                    key: mortar_meta::fnv1a64(&fields.string("name")),
                    hash: hash()?,
                    flags: fields.num("flags", 0).map_err(at)?,
                    ..Default::default()
                });
            }
            b"type" => {
                fields.done(&["name", "hash", "layout", "size", "align", "flags"]).map_err(at)?;
                out.types.push(mortar_meta::table::FullRow {
                    key: mortar_meta::fnv1a64(&fields.string("name")),
                    hash: hash()?,
                    layout: fields.num("layout", 0).map_err(at)?,
                    size: fields.num("size", 0).map_err(at)?,
                    align: fields.num("align", 0).map_err(at)?,
                    flags: fields.num("flags", 0).map_err(at)?,
                });
            }
            other => {
                return Err(at(format!(
                    "'{}' is not 'row', 'hidden' or 'type'",
                    String::from_utf8_lossy(other)
                )));
            }
        }
    }
    Ok(out)
}

/// `meta table`: fills the placeholder table of `input` with the rows of
/// `text`, as `out`.
///
/// # Errors
/// A malformed description, no placeholder or too little room in it, a row
/// whose symbol is not defined once.
pub fn fill_table(text: &str, input: &Path, out: &Path) -> Result<(), String> {
    use mortar_meta::table;
    let text = table_rows(text)?;
    let full = !text.hidden.is_empty() || !text.types.is_empty();
    let mut bytes = std::fs::read(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let mut fills: Vec<(usize, usize, Vec<u8>)> = Vec::new();
    {
        let so = mortar_elf::SharedObject::parse(&bytes).map_err(|e| e.to_string())?;
        let (mut place, mut full_place) = (None, None);
        for seg in so.program_headers() {
            if seg.p_type() != mortar_elf::consts::PT_NOTE {
                continue;
            }
            for note in seg.notes().map_err(|e| e.to_string())? {
                let note = note.map_err(|e| e.to_string())?;
                let at = note.desc().as_ptr() as usize - so.data().as_ptr() as usize;
                if note.name_bytes() != table::OWNER {
                    continue;
                }
                match note.n_type {
                    table::NT_MASON_ODR => place = Some((at, note.desc().len())),
                    table::NT_MASON_ODR_FULL => full_place = Some((at, note.desc().len())),
                    _ => {}
                }
            }
        }
        let (at, len) = place.ok_or("no placeholder .note.mason table in a PT_NOTE")?;
        let symbols = so.symbols().map_err(|e| e.to_string())?.ok_or("no .dynsym")?;
        let versions = so.versions().map_err(|e| e.to_string())?;
        // The defined symbols, (name, version node, index), sorted once.
        let mut defined: Vec<(Vec<u8>, Option<Vec<u8>>, u32)> = Vec::new();
        for (index, sym) in symbols.iter() {
            let Ok(name) = symbols.name(&sym) else { continue };
            if sym.st_shndx == 0 {
                continue;
            }
            let node = versions
                .of(index)
                .ok()
                .flatten()
                .and_then(|v| v.name().ok().flatten())
                .map(<[u8]>::to_vec);
            defined.push((name.to_vec(), node, index.0));
        }
        defined.sort();
        let mut encoded = Vec::new();
        for r in &text.rows {
            let from = defined.partition_point(|d| d.0 < r.name);
            let found: Vec<u32> = defined[from..]
                .iter()
                .take_while(|d| d.0 == r.name)
                .filter(|d| r.version.is_none() || d.1 == r.version)
                .map(|d| d.2)
                .collect();
            let [dynsym] = found.as_slice() else {
                return Err(format!(
                    "row {}: {} definitions in .dynsym",
                    String::from_utf8_lossy(&r.name),
                    found.len()
                ));
            };
            encoded.push(table::Row {
                key: mortar_meta::fnv1a64(&r.name),
                hash: r.hash,
                dynsym: *dynsym,
                flags: r.flags,
            });
        }
        fills.push((at, len, table::note(&encoded)));
        if full {
            let (at, len) = full_place.ok_or("no placeholder =full note (type 0x4d4f4402)")?;
            fills.push((at, len, table::full_note(&text.hidden, &text.types)));
        }
    }
    for (at, room, encoded) in fills {
        let desc = &encoded[table::DESC_OFFSET..];
        if desc.len() > room {
            return Err(format!(
                "the placeholder holds {room} bytes, the table takes {}",
                desc.len()
            ));
        }
        bytes[at..at + desc.len()].copy_from_slice(desc);
    }
    std::fs::write(out, &bytes).map_err(|e| format!("{}: {e}", out.display()))
}

/// `cargo xtask meta ...`.
pub fn main(args: &[String]) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprint!("xtask meta: {e}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, object] if cmd == "dump" => {
            let bytes = std::fs::read(object).map_err(|e| format!("{object}: {e}"))?;
            print!("{}", dump(&bytes)?);
            Ok(())
        }
        [cmd, rows, into, input, o, out] if cmd == "table" && into == "--into" && o == "-o" => {
            let text = std::fs::read_to_string(rows).map_err(|e| format!("{rows}: {e}"))?;
            fill_table(&text, Path::new(input), Path::new(out))
        }
        [cmd, description, rest @ ..] if cmd == "write" => {
            let text =
                std::fs::read_to_string(description).map_err(|e| format!("{description}: {e}"))?;
            let sections = parse(&text)?;
            let mut out: Option<PathBuf> = None;
            let mut mode: Option<(&str, &str)> = None;
            let mut target: Option<&str> = None;
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                let mut value =
                    || it.next().map(String::as_str).ok_or(format!("{a} takes a value"));
                match a.as_str() {
                    "-o" => out = Some(PathBuf::from(value()?)),
                    "--bin" | "--object" | "--into" | "--ll" | "--into-ll" => {
                        mode = Some((a.as_str(), value()?));
                    }
                    "--target" => target = Some(value()?),
                    _ => return Err(format!("unknown argument '{a}'")),
                }
            }
            match (mode, out, target) {
                (None, Some(o), None) => {
                    std::fs::write(&o, assembly(&sections)?).map_err(|e| e.to_string())
                }
                (Some(("--bin", path)), None, None) => {
                    let [items] = sections.as_slice() else {
                        return Err("--bin writes one section".into());
                    };
                    std::fs::write(path, section_bytes(items)?).map_err(|e| e.to_string())
                }
                (Some(("--object", path)), None, Some(t)) => {
                    assemble(&sections, t, Path::new(path))
                }
                (Some(("--into", input)), Some(o), None) => {
                    let [items] = sections.as_slice() else {
                        return Err("--into adds one section".into());
                    };
                    add_to(items, Path::new(input), &o)
                }
                (Some(("--ll", path)), None, None) => {
                    std::fs::write(path, ir_module(&sections)?).map_err(|e| e.to_string())
                }
                (Some(("--into-ll", input)), Some(o), None) => {
                    let module =
                        std::fs::read_to_string(input).map_err(|e| format!("{input}: {e}"))?;
                    std::fs::write(&o, into_ir(&module, &sections)?).map_err(|e| e.to_string())
                }
                _ => {
                    Err("one output: -o, --bin, --object with --target, --into with -o, --ll, or \
                          --into-ll with -o"
                        .into())
                }
            }
        }
        _ => Err("unknown command".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &str = r#"
# every directive
summary name=lang value=c++ flags=1
summary name=std value="c++ 20"
definition name=_ZN1A1fEv hash=000102030405060708090a0b0c0d0e0f class=1 flags=0x3 module=- file=a.h line=7
definition anchor=_ZN1A1gEv class=2
definition key=0x1234 class=1
type name=N1A size=8 align=4 file="a b.h" line=3
type name="\x01odd\"" key=5 details=0
declaration name=_Z1hv ty=0x99 size=4 type_name=int flags=2
declaration anchor=x
details hex=0102030405060708
raw kind=0x7f00 version=3 row_size=8 row_count=1 hex=aabbccddeeff0011
pad 2
unit producer=1
summary name=a value=b
section
definition name=f
"#;

    // spec: meta.md §12 ("the test producer"), MT-38: the dump of a
    // written unit writes back to the same bytes.
    #[test]
    fn dump_writes_back() {
        let sections = parse(ALL).unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].len(), 3, "a unit, a pad, a unit");
        for items in &sections {
            let bytes = section_bytes(items).unwrap();
            let mut r = FileReader::new();
            r.section(SectionAttrs::conforming(1), &bytes);
            assert!(r.findings.is_empty(), "{:?}", r.findings);
            let mut text = String::new();
            for u in &r.units {
                print_unit(&mut text, u);
            }
            let again = parse(&text).unwrap();
            let units: Vec<&Item> = items.iter().filter(|i| matches!(i, Item::Unit(_))).collect();
            let back: Vec<Vec<u8>> = again[0]
                .iter()
                .map(|i| match i {
                    Item::Unit(u) => u.finish().unwrap(),
                    Item::Pad(_) => unreachable!(),
                })
                .collect();
            let want: Vec<Vec<u8>> = units
                .iter()
                .map(|i| match i {
                    Item::Unit(u) => u.finish().unwrap(),
                    Item::Pad(_) => unreachable!(),
                })
                .collect();
            assert_eq!(back, want, "{text}");
        }
    }

    const DETAILED: &str = r"
unit producer=4
level detail
summary name=odr.scheme value=test-1
type name=1S size=16 align=8 flags=4
vptr offset=0
member name=a type=i offset=64
member name=b type=i offset=96 width=3
slot name=_ZN1S1fEv index=0
type name=1E size=4 align=4 flags=1
underlying type=i
enumerator name=A value=0
enumerator name=B value=0xffffffffffffffff
enumerator name=C value=-2
type name=1F size=4 align=4 flags=1
underlying type=j scoped=1 fixed=1
enumerator name=U value=0xffffffffffffffff
type name=1U size=8 align=4
member offset=0 nested=2
member name=i type=i offset=0
member name=f type=f offset=0
member name=k type=i offset=32
type name=1D size=24 align=8 flags=8
base type=1S offset=0 virtual=1
base type=1B offset=128
item flags=0x7 name=future type=x value=9 width=1
type name=1N size=1 align=1
";

    /// The description's single unit.
    fn only_unit(text: &str) -> UnitBuilder {
        match parse(text).unwrap().as_slice() {
            [items] => match items.as_slice() {
                [Item::Unit(u)] => u.clone(),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }

    // spec: odr.md O-19 (WP-145): the test producer writes kind 4 from the
    // same items its kind 3 hash is computed from; at the default level
    // the same rows without details; the dump prints entries and items
    // and writes back to the same bytes.
    #[test]
    fn layout_details_round_trip() {
        let detail = only_unit(DETAILED);
        let default = only_unit(&DETAILED.replace("level detail\n", ""));
        for (d, f) in detail.types.iter().zip(&default.types) {
            assert_eq!(d.layout, f.layout, "the level does not change the hash");
            assert!(f.items.is_none());
        }
        let s = detail.types.iter().find(|t| t.name == b"1S").unwrap();
        assert_eq!(s.layout, layout_hash(16, 8, s.items.as_ref().unwrap()));
        assert_ne!(s.layout, 0);
        assert!(detail.types.iter().find(|t| t.name == b"1N").unwrap().items.is_none());
        let bytes = detail.finish().unwrap();
        let (units, findings) = mortar_meta::read_section(SectionAttrs::conforming(1), &bytes);
        assert!(findings.is_empty(), "{findings:?}");
        let mut text = String::new();
        print_unit(&mut text, &units[0]);
        assert!(text.contains("level detail\n"), "{text}");
        for line in [
            "vptr offset=0\n",
            "member name=a type=i offset=64\n",
            "member name=b type=i offset=96 width=3\n",
            "slot name=_ZN1S1fEv index=0\n",
            "underlying type=i\n",
            "underlying type=j scoped=1 fixed=1\n",
            "enumerator name=B value=-1\n",
            "enumerator name=C value=-2\n",
            "enumerator name=U value=18446744073709551615\n",
            "member offset=0 nested=2\n",
            "base type=1S offset=0 virtual=1\n",
            "base type=1B offset=128\n",
            "item flags=0x7 name=future type=x value=9 width=1\n",
        ] {
            assert!(text.contains(line), "{line}: {text}");
        }
        assert_eq!(only_unit(&text).finish().unwrap(), bytes, "{text}");
        // An entry the writer would not write (a set `reserved`): raw.
        let mut odd = bytes.clone();
        let body = units[0].details().unwrap();
        let at = odd.len() - units[0].bytes().len()
            + (body.as_ptr() as usize - units[0].bytes().as_ptr() as usize)
            + 4;
        odd[at] = 1;
        let (units, _) = mortar_meta::read_section(SectionAttrs::conforming(1), &odd);
        let mut text = String::new();
        print_unit(&mut text, &units[0]);
        // (Not a unit the writer could have written: its strings that only
        // kind 4 names are not in the raw form, so it does not write back.)
        assert!(!text.contains("level detail") && text.contains("details hex="), "{text}");
        for bad in [
            "member name=x\n",
            "type name=1S\nmember nam=x\n",
            "level\n",
            "level x\n",
            "type name=1E\nenumerator name=A value=-9223372036854775809\n",
            "type name=1E\nenumerator name=A value=-x\n",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(parse("type name=1S\nmember name=x\ndetails hex=00\n").is_ok());
        let both = only_unit("level detail\ntype name=1S\nmember name=x\ndetails hex=00\n");
        assert!(both.finish().is_err(), "typed items and a raw kind 4 block");
    }

    #[test]
    fn fields() {
        let s = parse("definition name=f\n").unwrap();
        let Item::Unit(u) = &s[0][0] else { panic!() };
        assert_eq!(u.definitions[0].key, mortar_meta::fnv1a64(b"f"));
        assert_eq!(u.producer, producer::TEST);
        let s = parse("declaration anchor=g\n").unwrap();
        let Item::Unit(u) = &s[0][0] else { panic!() };
        assert_eq!(
            (u.declarations[0].key, u.declarations[0].name.clone()),
            (mortar_meta::fnv1a64(b"g"), None)
        );
        for bad in [
            "summary nam=x",
            "summary name=x name=y",
            "definition name=a anchor=b",
            "frob",
            "summary name=\"x",
            "details hex=abc",
            "definition hash=00",
            "summary flags=0x100000000",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert_eq!(quote(b""), "\"\"");
        assert_eq!(quote(b"a=b"), "\"a=b\"");
    }

    // WP-075 (ruling item 4): `--into` uses the input's own target's
    // objcopy, so an x86-64 or i386 object takes a container on an AArch64
    // host (and the reverse); the container is MT-1's (alignment 8,
    // excluded) and reads back.
    #[test]
    fn into_uses_the_targets_objcopy() {
        let scratch = mortar_testutil::Scratch::new("meta-into");
        let items = &parse("definition name=f class=1\n").unwrap()[0];
        for target in ["x86_64", "aarch64", "i386"] {
            let (asm, flags) = assembler(target).unwrap();
            let objcopy = objcopy_for(&elf_header(target)).unwrap();
            if crate::mason::tool(asm).is_none() || crate::mason::tool(objcopy).is_none() {
                assert!(!mortar_testutil::tools_required(), "{asm} and {objcopy} are required");
                continue;
            }
            let src = scratch.write(&format!("{target}.s"), "\t.text\n\t.globl f\nf:\n");
            let input = scratch.join(format!("{target}.o"));
            let out = scratch.join(format!("{target}-meta.o"));
            let mut args: Vec<String> = flags.iter().map(|f| (*f).to_string()).collect();
            args.extend(["-o".into(), input.display().to_string(), src.display().to_string()]);
            run_tool(asm, &args).unwrap();
            add_to(items, &input, &out).unwrap();
            let bytes = std::fs::read(&out).unwrap();
            let object = mortar_elf::Object::parse(&bytes).unwrap();
            let meta = object
                .sections()
                .find(|s| s.name().is_ok_and(|n| n == mortar_meta::SECTION_NAME))
                .expect("a .mason.meta section");
            assert_eq!(meta.header().sh_addralign, 8, "{target}");
            assert!(mortar_meta::is_container(b".mason.meta", meta.sh_type(), meta.flags()));
            let text = dump(&bytes).unwrap();
            assert!(text.contains("definition name=f "), "{target}: {text}");
        }
        assert!(objcopy_for(b"not an elf file at all").is_err());
    }

    /// The first 20 bytes of an ELF header of `target`.
    fn elf_header(target: &str) -> Vec<u8> {
        let machine: u16 = match target {
            "x86_64" => 62,
            "aarch64" => 183,
            _ => 3,
        };
        let mut h = vec![0x7f, b'E', b'L', b'F', 2, 1, 1];
        h.resize(18, 0);
        h.extend_from_slice(&machine.to_le_bytes());
        h
    }

    #[test]
    fn assembly_has_one_section_per_section() {
        let s = parse(ALL).unwrap();
        let a = assembly(&s).unwrap();
        assert_eq!(a.matches(".section .mason.meta,\"e\",%progbits,unique,").count(), 2);
        let one = assembly(&parse("summary name=a value=b\n").unwrap()).unwrap();
        assert!(one.contains(".section .mason.meta,\"e\",%progbits\n\t.p2align 3"));
    }

    /// LLVM's lexer for a `c"..."` body (`\\` or `\XX`), to read back.
    fn unescape(body: &str) -> Vec<u8> {
        let b = body.as_bytes();
        let (mut out, mut i) = (Vec::new(), 0);
        while i < b.len() {
            if b[i] == b'\\' {
                if b.get(i + 1) == Some(&b'\\') {
                    out.push(b'\\');
                    i += 2;
                } else {
                    out.push(u8::from_str_radix(&body[i + 1..i + 3], 16).unwrap());
                    i += 3;
                }
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        out
    }

    // spec: meta.md §12 "The test producer" (WP-099: the IR form, exact
    // for every byte value)
    #[test]
    fn c_string_is_exact_for_every_byte() {
        let all: Vec<u8> = (0..=255u8).chain([b'"', b'\\', b'\\', b'x', b'0']).collect();
        let s = c_string(&all);
        assert!(!s.contains('"') && s.is_ascii());
        assert_eq!(unescape(&s), all);
        assert_eq!(c_string(b"MASN\x01\x00\"\\"), "MASN\\01\\00\\22\\5C");
    }

    // spec: meta.md §12 (WP-099): one global per section, the bytes of
    // the ELF form, `@llvm.compiler.used`, and the merge into a module
    #[test]
    fn ir_forms() {
        let s = parse(ALL).unwrap();
        let m = ir_module(&s).unwrap();
        let globals: Vec<&str> = m.lines().filter(|l| l.starts_with("@__mason_meta")).collect();
        assert_eq!(globals.len(), 2);
        for (g, items) in globals.iter().zip(&s) {
            let bytes = section_bytes(items).unwrap();
            assert!(g.contains(&format!("= private constant [{} x i8] c\"", bytes.len())));
            assert!(g.ends_with(", section \".mason.meta\", align 8, !exclude !{}"));
            let body = g.split_once("c\"").unwrap().1.rsplit_once("\", section").unwrap().0;
            assert_eq!(unescape(body), bytes);
        }
        assert!(m.contains(
            "@llvm.compiler.used = appending global [2 x ptr] [ptr @__mason_meta, ptr \
             @__mason_meta.1], section \"llvm.metadata\"\n"
        ));
        let one = parse("summary name=a value=b\n").unwrap();
        let module = "define i32 @main() {\n  ret i32 0\n}\n@llvm.compiler.used = appending \
                      global [1 x ptr] [ptr @x], section \"llvm.metadata\"\n";
        let into = into_ir(module, &one).unwrap();
        assert!(into.starts_with("define i32 @main() {\n"));
        assert!(into.contains(
            "@llvm.compiler.used = appending global [2 x ptr] [ptr @x, ptr @__mason_meta], \
             section \"llvm.metadata\"\n"
        ));
        assert_eq!(into.matches("@llvm.compiler.used").count(), 1);
        let fresh = into_ir("define void @f() {\n  ret void\n}\n", &one).unwrap();
        assert!(fresh.contains("[1 x ptr] [ptr @__mason_meta]"));
        assert!(into_ir(&fresh, &one).unwrap_err().contains("already defines @__mason_meta"));
        let odd = "@llvm.compiler.used = appending global [1 x ptr] [ptr @x]\n";
        assert!(into_ir(odd, &one).unwrap_err().contains("not in the form"));
    }
}
