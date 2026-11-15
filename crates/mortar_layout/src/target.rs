//! The facts and options layout reads (`spec/layout.md` §11.3, L-68,
//! L-74, L-75, L-81, L-110, L-112).
//!
//! `mortar_arch` on `main` has no `Facts` yet (WP-023 writes it); the few
//! facts layout needs are here, with the values `spec/arch.md` §3 gives.
//! When `mortar_arch::Facts` lands, [`Target::of`] becomes a projection of
//! it and nothing else changes.

use mortar_core::layout::SegmentRules;
use mortar_core::plan::{OutputKind, Plan};
use mortar_elf::consts::{PF_R, PF_W, PF_X};
use mortar_elf::{Class, Machine};

use crate::diag::{LayoutDiag, feature, invalid_option};

/// The machine facts of layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub machine: Machine,
    pub class: Class,
    /// `Facts.max_page`: the default maxpage (L-74).
    pub max_page: u64,
    /// `Facts.image_base` for a non-PIE executable (L-68).
    pub image_base: u64,
    /// The island policy, on machines with range-extension thunks or
    /// erratum patches (AArch64); `None` on x86 (L-110).
    pub islands: Option<IslandRules>,
}

/// The part limit and island cap (L-110, L-112).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IslandRules {
    /// `S`: no body part is larger.
    pub part_limit: u64,
    /// `C`: no island is larger.
    pub cap: u64,
    /// log2 of an island's alignment (patch alignment 4).
    pub align: u8,
}

/// AArch64's island rules: `S = 2^27 − 2^22 − 32`, `C = 2^22` (L-110,
/// L-112).
pub const AARCH64_ISLANDS: IslandRules =
    IslandRules { part_limit: (1 << 27) - (1 << 22) - 32, cap: 1 << 22, align: 2 };

impl Target {
    /// The facts of a machine.
    #[must_use]
    pub const fn of(machine: Machine) -> Self {
        match machine {
            Machine::AArch64 => Self {
                machine,
                class: Class::Elf64,
                max_page: 0x1_0000,
                image_base: 0x40_0000,
                islands: Some(AARCH64_ISLANDS),
            },
            Machine::X86_64 => Self {
                machine,
                class: Class::Elf64,
                max_page: 0x1000,
                image_base: 0x40_0000,
                islands: None,
            },
            Machine::I386 => Self {
                machine,
                class: Class::Elf32,
                max_page: 0x1000,
                image_base: 0x0804_8000,
                islands: None,
            },
        }
    }

    /// log2 of the word size.
    #[must_use]
    pub const fn word_log(&self) -> u8 {
        match self.class {
            Class::Elf64 => 3,
            Class::Elf32 => 2,
        }
    }

    /// The end of the address space of the class (L-73).
    #[must_use]
    pub const fn address_limit(&self) -> u128 {
        match self.class {
            Class::Elf64 => 1 << 64,
            Class::Elf32 => 1 << 32,
        }
    }
}

/// How the entry is named (L-102).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryName {
    /// `-e` / `--entry` named a symbol, or the default `_start`.
    Symbol(Vec<u8>),
    /// `-e` gave a number that is no symbol's name: an absolute address.
    Address(u64),
    /// `-shared` or `-r` with no `-e`: no entry (`e_entry = 0`, silently).
    None,
}

/// The options layout reads, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // one per -z keyword
pub struct Options {
    /// maxpage (L-74).
    pub max_page: u64,
    /// `-z common-page-size`, validated, unused (L-75).
    pub common_page: Option<u64>,
    /// `-z separate-code` (L-71).
    pub separate_code: bool,
    /// `--fix-cortex-a53-843419` (L-117, L-118).
    pub fix_843419: bool,
    /// `-z execstack` / `-z noexecstack` (L-84).
    pub execstack: Option<bool>,
    /// `--eh-frame-hdr`: whether `.eh_frame_hdr` exists (§13.1).
    pub eh_frame_hdr: bool,
    /// `-z relro` (default) / `-z norelro` (§20.2).
    pub relro: bool,
    pub entry: EntryName,
}

impl Options {
    /// The defaults for a static non-PIE executable on `target`.
    #[must_use]
    pub fn defaults(target: &Target) -> Self {
        Self {
            max_page: target.max_page,
            common_page: None,
            separate_code: false,
            fix_843419: false,
            execstack: None,
            eh_frame_hdr: false,
            relro: true,
            entry: EntryName::Symbol(b"_start".to_vec()),
        }
    }

    /// The options of a plan. M1 links static non-PIE executables only;
    /// another output kind is refused by name.
    ///
    /// # Errors
    ///
    /// `invalid-option-value` for a page size L-74 or L-75 does not accept;
    /// `refused` for an output kind M1 does not link.
    pub fn from_plan(plan: &Plan, target: &Target) -> Result<Self, LayoutDiag> {
        match plan.kind {
            OutputKind::StaticExecutable | OutputKind::Executable => {}
            OutputKind::PieExecutable | OutputKind::StaticPie => {
                return Err(feature("position-independent executable output"));
            }
            OutputKind::Shared => return Err(feature("shared object output")),
            OutputKind::Relocatable => return Err(feature("relocatable output")),
        }
        let mut o = Self::defaults(target);
        if let Some(v) = &plan.z.max_page_size {
            let text = v.to_string_lossy();
            o.max_page = match parse_number(&text) {
                Some(0) => target.max_page,
                Some(n) if n.is_power_of_two() && n >= 4096 => n,
                _ => return Err(invalid_option("-z max-page-size", &text)),
            };
        }
        if let Some(v) = &plan.z.common_page_size {
            let text = v.to_string_lossy();
            o.common_page = match parse_number(&text) {
                Some(0) => None,
                Some(n) if n.is_power_of_two() && n <= o.max_page => Some(n),
                _ => return Err(invalid_option("-z common-page-size", &text)),
            };
        }
        o.separate_code = plan.z.separate_code.unwrap_or(false);
        o.execstack = plan.z.execstack;
        o.relro = plan.z.relro.unwrap_or(true);
        o.eh_frame_hdr = plan.shaping.eh_frame_hdr.unwrap_or(false);
        o.fix_843419 = target.machine == Machine::AArch64
            && plan.target_options.iter().any(|t| t.option.starts_with("--fix-cortex-a53-843419"));
        // `full` (the default) is spec/a64.md §11.2's choice per site; GNU
        // ld's `adr` and `adrp` restrict it, which mortar does not do
        // (WP-027c: refused by name rather than linked as `full`).
        if o.fix_843419
            && let Some((t, v)) = plan.target_options.iter().find_map(|t| {
                let v = t.value.as_ref()?;
                (t.option.starts_with("--fix-cortex-a53-843419") && v != "full").then_some((t, v))
            })
        {
            return Err(crate::diag::refused(
                mortar_diag::RefusalClass::OptionValue,
                format!("{}={}", t.option, v.to_string_lossy()),
            ));
        }
        if let Some(e) = &plan.entry {
            o.entry = EntryName::Symbol(e.as_encoded_bytes().to_vec());
        }
        Ok(o)
    }

    /// What `derive_segments` reads (`PT_GNU_STACK`'s flags are decided by
    /// [`crate::stack`]).
    #[must_use]
    pub fn segment_rules(&self, target: &Target, exec_stack: bool) -> SegmentRules {
        let stack = if exec_stack { PF_R | PF_W | PF_X } else { PF_R | PF_W };
        SegmentRules { class: target.class, max_page: self.max_page, stack }
    }
}

/// A number as GNU ld's option parser reads it: decimal, `0x` hexadecimal,
/// or a leading `0` octal.
#[must_use]
pub fn parse_number(text: &str) -> Option<u64> {
    let t = text.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else if t.len() > 1 && t.starts_with('0') {
        u64::from_str_radix(&t[1..], 8).ok()
    } else {
        t.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(f: impl FnOnce(&mut Plan)) -> Result<Options, LayoutDiag> {
        let mut plan = Plan { kind: OutputKind::StaticExecutable, ..Plan::default() };
        f(&mut plan);
        Options::from_plan(&plan, &Target::of(Machine::AArch64))
    }

    // spec: layout.md §11.4 L-74, L-75 (page sizes), L-71, L-84, §20.2
    #[test]
    fn l74_l75_options() {
        let d = with(|_| {}).unwrap();
        assert_eq!(d.max_page, 0x10000);
        assert!(!d.separate_code && d.relro && d.execstack.is_none());
        let o = with(|p| p.z.max_page_size = Some("0x4000".into())).unwrap();
        assert_eq!(o.max_page, 0x4000);
        assert_eq!(with(|p| p.z.max_page_size = Some("0".into())).unwrap().max_page, 0x10000);
        for bad in ["4095", "0x3000", "2048", "x"] {
            let e = with(|p| p.z.max_page_size = Some(bad.into())).unwrap_err();
            assert_eq!(e.id(), "invalid-option-value", "{bad}");
        }
        let o = with(|p| p.z.common_page_size = Some("4096".into())).unwrap();
        assert_eq!((o.common_page, o.max_page), (Some(4096), 0x10000));
        let e = with(|p| p.z.common_page_size = Some("0x20000".into())).unwrap_err();
        assert_eq!(e.id(), "invalid-option-value");
        let o = with(|p| {
            p.z.separate_code = Some(true);
            p.z.relro = Some(false);
            p.z.execstack = Some(true);
        })
        .unwrap();
        assert!(o.separate_code && !o.relro && o.execstack == Some(true));
        let e = with(|p| p.kind = OutputKind::Shared).unwrap_err();
        assert_eq!(e.refusal().map(|r| r.1), Some("shared object output"));
    }
}
