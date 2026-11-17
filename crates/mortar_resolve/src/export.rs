//! The export set and preemption of a shared object (`spec/dyn.md` D-110,
//! D-112, D-113; `spec/resolve.md` §9.6; `spec/layout.md` L-100).
//!
//! Two per-key functions the shared stage (`crate::shared`) applies to
//! every key of a `Shared` output, after `def` is final: [`exported`]
//! (D-110) and then [`preemptible`] (D-112). `Resolution.preemptible` is
//! the second's column, a subset of `exported` (`spec/model.md` §3.3
//! check (8)); the scan reads it as the target kind `Preemptible`
//! (D-113). Neither is called for an executable.
//!
//! A version script's `local:` (D-110 item 1, D-114) is applied after
//! [`exported`] by `crate::version::apply`; `--exclude-libs` (D-110 item
//! 1, WP-128) before it, by the shared stage (`Options::exclude_libs`). A
//! `--dynamic-list` (D-112, WP-128) changes preemption only: the shared
//! stage tells [`preemptible`] whether the list names the key.

use mortar_core::plan::{Symbolic, SymbolicLine};
use mortar_elf::consts::{STT_FUNC, STV_DEFAULT, STV_PROTECTED};

use crate::linker::Provided;
use crate::resolve::Def;

/// D-110: whether a key whose definition is `def` is exported from a
/// shared object. `visibility` is the key's most constraining (resolve
/// §3.6), `ref_dynamic` whether a live shared input references it (D-21).
///
/// - a definition of a relocatable object (strong, weak, common;
///   `STB_GNU_UNIQUE` is strong): exported when `STV_DEFAULT` or
///   `STV_PROTECTED` (hidden and internal ones never; `__dso_handle` and
///   libgcc's helpers are hidden);
/// - a linker-defined key (item 2, L-100): `__start_NAME`/`__stop_NAME`
///   always (they exist only when a live object references them,
///   resolve §8.2), every other one only when `ref_dynamic`, and never one
///   the table makes local (`_DYNAMIC`, `_GLOBAL_OFFSET_TABLE_`,
///   `__ehdr_start`, …);
/// - an undefined key or a shared definition: never (an import, D-111).
#[must_use]
pub fn exported(def: Def, visibility: u8, ref_dynamic: bool) -> bool {
    let visible = matches!(visibility, STV_DEFAULT | STV_PROTECTED);
    match def {
        Def::Strong(_) | Def::Weak(_) | Def::Common(_) => visible,
        Def::Provide { sym: Provided::Start | Provided::Stop, .. } => true,
        // D-110 item 2 (WP-150): `_end`, `__bss_start`, ... that an input
        // object references are exported as the library's own definitions
        // (GNU ld and lld); every other linker-defined key only when a
        // shared object references it.
        Def::Provide { sym, .. } => {
            let (local, row) = sym.binding();
            visible
                && (ref_dynamic || sym.exported_as_definition())
                && !local
                && matches!(row, STV_DEFAULT | STV_PROTECTED)
        }
        Def::Undefined { .. } | Def::Shared(_) => false,
    }
}

/// D-112: whether an exported key stays preemptible (its uses go through
/// a record the loader binds) or is bound inside the library at link
/// time. `sym_type` is the defining entry's `STT_*` (0 for a
/// linker-defined key, `STT_NOTYPE`).
///
/// `STV_PROTECTED` keys are bound on every line, and so are `__start_`
/// and `__stop_` (exported protected, D-110 item 2). Otherwise the line's
/// last form decides (without a `--dynamic-list`): `-Bsymbolic` binds
/// every type, `-Bsymbolic-functions` binds `STT_FUNC` only (lld's rule;
/// `STT_GNU_IFUNC` stays preemptible), `-Bno-symbolic` or no form binds
/// nothing. A weak definition is its type's column. A line with a
/// `--dynamic-list` decides by the list alone, whatever its `-Bsymbolic`
/// forms: `listed` (whether the list names the key) keeps it
/// preemptible, of every type; an unlisted key is bound (lld's rule; GNU
/// ld keeps `STT_TLS` and `STT_GNU_IFUNC` preemptible). `listed` is read
/// only on such a line.
#[must_use]
pub fn preemptible(
    def: Def,
    visibility: u8,
    sym_type: u8,
    exported: bool,
    symbolic: SymbolicLine,
    listed: bool,
) -> bool {
    if !exported || visibility == STV_PROTECTED {
        return false;
    }
    if let Def::Provide { sym: Provided::Start | Provided::Stop, .. } = def {
        return false;
    }
    if symbolic.has_dynamic_list {
        return listed;
    }
    match symbolic.form {
        Symbolic::None => true,
        Symbolic::All => false,
        Symbolic::Functions => sym_type != STT_FUNC,
    }
}

#[cfg(test)]
mod tests {
    use super::{exported, preemptible};
    use crate::linker::{Provided, TABLE};
    use crate::resolve::Def;
    use mortar_core::plan::{Symbolic, SymbolicLine};
    use mortar_core::{FileId, SymEntry};
    use mortar_elf::SymbolIndex;
    use mortar_elf::consts::{
        STT_COMMON, STT_FUNC, STT_GNU_IFUNC, STT_NOTYPE, STT_OBJECT, STT_TLS, STV_DEFAULT,
        STV_HIDDEN, STV_INTERNAL, STV_PROTECTED,
    };

    const E: SymEntry = SymEntry { file: FileId(1), index: SymbolIndex(5) };

    fn table(name: &[u8]) -> Provided {
        Provided::Table(TABLE.iter().position(|r| r.name == name).expect("a row") as u8)
    }

    // spec: dyn.md D-110 (the export set), layout.md L-100
    #[test]
    fn d110_export_set() {
        for d in [Def::Strong(E), Def::Weak(E), Def::Common(E)] {
            assert!(exported(d, STV_DEFAULT, false), "{d:?}");
            assert!(exported(d, STV_PROTECTED, false), "{d:?}");
            assert!(!exported(d, STV_HIDDEN, true), "{d:?}");
            assert!(!exported(d, STV_INTERNAL, true), "{d:?}");
        }
        let start = Def::Provide { sym: Provided::Start, conditional: true };
        let stop = Def::Provide { sym: Provided::Stop, conditional: true };
        assert!(exported(start, STV_DEFAULT, false));
        assert!(exported(stop, STV_DEFAULT, false));
        // Item 2 (WP-150): the seven names GNU ld and lld both define are
        // exported once an input object references them (the key is then
        // `Provide`); `__etext` and the AArch64-only names (lld defines
        // none of them) only when a shared input references them; the
        // local rows never.
        for name in [&b"_etext"[..], b"etext", b"_edata", b"edata", b"__bss_start", b"_end", b"end"]
        {
            let d = Def::Provide { sym: table(name), conditional: false };
            assert!(exported(d, STV_DEFAULT, false), "{}", String::from_utf8_lossy(name));
            assert!(exported(d, STV_DEFAULT, true));
            assert!(!exported(d, STV_HIDDEN, true), "a hidden reference");
        }
        for name in [&b"__etext"[..], b"__data_start", b"__end__", b"__bss_start__", b"_bss_end__"]
        {
            let d = Def::Provide { sym: table(name), conditional: false };
            assert!(!exported(d, STV_DEFAULT, false), "{}", String::from_utf8_lossy(name));
            assert!(exported(d, STV_DEFAULT, true));
        }
        for name in [&b"_DYNAMIC"[..], b"_GLOBAL_OFFSET_TABLE_", b"__ehdr_start"] {
            let d = Def::Provide { sym: table(name), conditional: false };
            assert!(!exported(d, STV_DEFAULT, true), "{}", String::from_utf8_lossy(name));
        }
        assert!(!exported(Def::Undefined { weak: false }, STV_DEFAULT, true));
        assert!(!exported(Def::Undefined { weak: true }, STV_DEFAULT, true));
        assert!(!exported(Def::Shared(E), STV_DEFAULT, true));
    }

    // spec: dyn.md D-112 (the table: none, -Bsymbolic, -Bsymbolic-functions,
    // STV_PROTECTED), D-110 item 2 (`__start_`/`__stop_` protected)
    #[test]
    fn d112_preemption_table() {
        let line = |form| SymbolicLine { form, has_dynamic_list: false };
        let types = [STT_FUNC, STT_OBJECT, STT_COMMON, STT_NOTYPE, STT_TLS, STT_GNU_IFUNC];
        for d in [Def::Strong(E), Def::Weak(E)] {
            for &t in &types {
                assert!(preemptible(d, STV_DEFAULT, t, true, line(Symbolic::None), false), "{t}");
                assert!(!preemptible(d, STV_DEFAULT, t, true, line(Symbolic::All), false), "{t}");
                let functions =
                    preemptible(d, STV_DEFAULT, t, true, line(Symbolic::Functions), false);
                assert_eq!(functions, t != STT_FUNC, "-Bsymbolic-functions, type {t}");
                for form in [Symbolic::None, Symbolic::All, Symbolic::Functions] {
                    assert!(
                        !preemptible(d, STV_PROTECTED, t, true, line(form), false),
                        "protected"
                    );
                }
            }
        }
        // Not exported: never preemptible (model.md §3.3 check (8)).
        assert!(!preemptible(
            Def::Strong(E),
            STV_HIDDEN,
            STT_OBJECT,
            false,
            line(Symbolic::None),
            false
        ));
        for sym in [Provided::Start, Provided::Stop] {
            let d = Def::Provide { sym, conditional: true };
            assert!(!preemptible(d, STV_DEFAULT, STT_NOTYPE, true, line(Symbolic::None), false));
        }
        // A linker-defined key a shared input references is exported
        // `NOTYPE` and follows the table.
        let end = Def::Provide { sym: table(b"_end"), conditional: false };
        assert!(preemptible(end, STV_DEFAULT, STT_NOTYPE, true, line(Symbolic::None), false));
        assert!(preemptible(end, STV_DEFAULT, STT_NOTYPE, true, line(Symbolic::Functions), false));
        assert!(!preemptible(end, STV_DEFAULT, STT_NOTYPE, true, line(Symbolic::All), false));
    }

    // spec: dyn.md D-112 (the `--dynamic-list` rows: key in F, key not in
    // F, with each `-Bsymbolic` form; protected bound)
    #[test]
    fn d112_dynamic_list_decides_alone() {
        let line = |form| SymbolicLine { form, has_dynamic_list: true };
        let types = [STT_FUNC, STT_OBJECT, STT_COMMON, STT_NOTYPE, STT_TLS, STT_GNU_IFUNC];
        for d in [Def::Strong(E), Def::Weak(E), Def::Common(E)] {
            for &t in &types {
                for form in [Symbolic::None, Symbolic::All, Symbolic::Functions] {
                    assert!(preemptible(d, STV_DEFAULT, t, true, line(form), true), "{t} {form:?}");
                    assert!(!preemptible(d, STV_DEFAULT, t, true, line(form), false), "{t}");
                    assert!(!preemptible(d, STV_PROTECTED, t, true, line(form), true), "{t}");
                    assert!(!preemptible(d, STV_HIDDEN, t, false, line(form), true), "{t}");
                }
            }
        }
        for sym in [Provided::Start, Provided::Stop] {
            let d = Def::Provide { sym, conditional: true };
            assert!(!preemptible(d, STV_DEFAULT, STT_NOTYPE, true, line(Symbolic::None), true));
        }
    }
}
