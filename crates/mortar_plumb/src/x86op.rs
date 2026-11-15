//! The x86 instruction operands both x86 tables read (`spec/x86.md` R-12):
//! the `ModRM` and `SIB` bytes, the ALU opcodes a GOT load may be rewritten
//! from, and the register-immediate forms the rewritings write. Shared by
//! the x86-64 record path (`x64rec.rs`, `spec/x64.md` §8) and the i386 one
//! (`x86rec.rs`, `spec/x86.md` §8); nothing here depends on the machine.

/// A `ModRM` byte's three fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModRm {
    /// Bits 7-6.
    pub md: u8,
    /// Bits 5-3: a register, or an opcode extension (`/digit`).
    pub reg: u8,
    /// Bits 2-0.
    pub rm: u8,
}

impl ModRm {
    pub const fn of(b: u8) -> Self {
        ModRm { md: b >> 6, reg: (b >> 3) & 7, rm: b & 7 }
    }

    /// `mod` 00, `rm` 101: a 32-bit displacement and no base register
    /// (RIP-relative on x86-64, absolute on i386).
    pub const fn disp32_only(self) -> bool {
        self.md == 0 && self.rm == 5
    }
}

/// The `reg` of a `ModRM` whose operand is a bare 32-bit displacement
/// (`mod` 00, `rm` 101).
pub const fn disp32_reg(modrm: u8) -> Option<u8> {
    let m = ModRm::of(modrm);
    if m.disp32_only() { Some(m.reg) } else { None }
}

/// Whether `op` is one of the eight ALU operations `op r32, r/m32`
/// (`add or adc sbb and sub xor cmp`: `03 0b 13 1b 23 2b 33 3b`) a GOT
/// operand may be rewritten into an immediate from.
pub const fn alu(op: u8) -> bool {
    matches!(op, 0x03 | 0x0b | 0x13 | 0x1b | 0x23 | 0x2b | 0x33 | 0x3b)
}

/// The register-immediate form of `op reg, mem` (`spec/x64.md` §8,
/// `spec/x86.md` §8): an ALU operation `81 /n`, `test` (`85`) `f7 /0`,
/// `mov` (`8b`) `c7 /0`, each with `mod` 11 and `rm` `reg`. The opcode
/// and `ModRM` bytes; the immediate follows.
pub const fn imm_form(op: u8, reg: u8) -> Option<[u8; 2]> {
    if alu(op) {
        Some([0x81, 0xc0 | (op & 0x38) | reg])
    } else if op == 0x85 {
        Some([0xf7, 0xc0 | reg])
    } else if op == 0x8b {
        Some([0xc7, 0xc0 | reg])
    } else {
        None
    }
}

/// `ff /2` (`call`) and `ff /4` (`jmp`) through a bare 32-bit
/// displacement: the `ModRM` bytes `15` and `25`.
pub const CALL_DISP32: u8 = 0x15;
pub const JMP_DISP32: u8 = 0x25;

/// `endbr64`.
pub const ENDBR64: [u8; 4] = [0xf3, 0x0f, 0x1e, 0xfa];

/// A byte string as a little-endian number (its first 8 bytes), for a
/// finding's expected and found values.
pub fn le(v: &[u8]) -> u64 {
    v.iter().take(8).rev().fold(0, |a, &b| a << 8 | u64::from(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imm_forms() {
        assert_eq!(imm_form(0x2b, 3), Some([0x81, 0xeb]));
        assert_eq!(imm_form(0x85, 1), Some([0xf7, 0xc1]));
        assert_eq!(imm_form(0x8b, 7), Some([0xc7, 0xc7]));
        assert_eq!(imm_form(0x8d, 0), None);
        assert!(ModRm::of(0x05).disp32_only());
        assert!(!ModRm::of(0x83).disp32_only());
        assert_eq!(disp32_reg(0x1d), Some(3));
    }
}
