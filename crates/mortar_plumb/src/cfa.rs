//! Call frame instructions run to a table, and the DWARF expressions a
//! CFA rule may hold, evaluated (DWARF 4 §6.4.2 "Call Frame
//! Instructions", §2.5 "DWARF Expressions"). `spec/plumb.md` §6.4, E6:
//! the FDEs a reference linker writes for its own PLT are judged by the
//! CFA they give at each instruction of the PLT (WP-088).

use std::collections::BTreeMap;

/// How the CFA is computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cfa {
    /// Register + offset.
    Reg(u64, i64),
    /// A DWARF expression whose result is the CFA.
    Expr(Vec<u8>),
}

/// How a register is recovered (only what E6 asks about is kept apart).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RegRule {
    Undefined,
    Same,
    /// Saved at CFA + n.
    Offset(i64),
    /// Any other rule (`val_offset`, `register`, `expression`,
    /// `val_expression`), by its opcode.
    Other(u8),
}

/// One row of the table: from its address on, until the next row's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Row {
    pub cfa: Option<Cfa>,
    pub regs: BTreeMap<u64, RegRule>,
}

struct Rd<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Rd<'_> {
    fn u8(&mut self) -> Result<u8, String> {
        let v = *self.b.get(self.pos).ok_or("the instructions end inside an instruction")?;
        self.pos += 1;
        Ok(v)
    }
    fn uint(&mut self, n: usize) -> Result<u64, String> {
        let mut v = 0u64;
        for i in 0..n {
            v |= u64::from(self.u8()?) << (8 * i);
        }
        Ok(v)
    }
    fn uleb(&mut self) -> Result<u64, String> {
        let mut v = 0u64;
        let mut shift = 0u32;
        loop {
            let b = self.u8()?;
            if shift < 64 {
                v |= u64::from(b & 0x7f) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            if shift > 70 {
                return Err("an overlong LEB128".into());
            }
        }
    }
    fn sleb(&mut self) -> Result<i64, String> {
        let mut v = 0i64;
        let mut shift = 0u32;
        loop {
            let b = self.u8()?;
            if shift < 64 {
                v |= i64::from(b & 0x7f) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                if shift < 64 && b & 0x40 != 0 {
                    v |= -1i64 << shift;
                }
                return Ok(v);
            }
            if shift > 70 {
                return Err("an overlong LEB128".into());
            }
        }
    }
    fn block(&mut self) -> Result<Vec<u8>, String> {
        let n = usize::try_from(self.uleb()?).map_err(|_| "a block too long")?;
        let end = self.pos.checked_add(n).filter(|&e| e <= self.b.len());
        let end = end.ok_or("a block runs past the instructions")?;
        let v = self.b[self.pos..end].to_vec();
        self.pos = end;
        Ok(v)
    }
}

/// Runs a CIE's initial instructions, then an FDE's, from `start`: the
/// rows by their first address. The instructions may have lost trailing
/// zero bytes (`eh.rs` compares them without their `DW_CFA_nop` padding),
/// so they are run with zero bytes after them: an operand cut there was 0,
/// and a zero byte is `DW_CFA_nop`.
pub(crate) fn run(
    cie: &[u8],
    fde: &[u8],
    code_align: u64,
    data_align: i64,
    start: u64,
) -> Result<Vec<(u64, Row)>, String> {
    let mut row = Row::default();
    let mut rows = Vec::new();
    let mut loc = start;
    exec(cie, code_align, data_align, &mut row, None, &mut loc, &mut rows, true)?;
    let initial = row.clone();
    exec(fde, code_align, data_align, &mut row, Some(&initial), &mut loc, &mut rows, false)?;
    rows.push((loc, row));
    Ok(rows)
}

/// The row in force at `pc` (`rows` from [`run`]).
pub(crate) fn row_at(rows: &[(u64, Row)], pc: u64) -> Option<&Row> {
    rows.iter().rev().find(|(a, _)| *a <= pc).map(|(_, r)| r)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // one opcode table
fn exec(
    insns: &[u8],
    code_align: u64,
    data_align: i64,
    row: &mut Row,
    initial: Option<&Row>,
    loc: &mut u64,
    rows: &mut Vec<(u64, Row)>,
    in_cie: bool,
) -> Result<(), String> {
    let mut b = insns.to_vec();
    b.extend([0u8; 16]);
    let mut r = Rd { b: &b, pos: 0 };
    let mut stack: Vec<Row> = Vec::new();
    let mut advance = |delta: u64, row: &Row, rows: &mut Vec<(u64, Row)>| {
        if in_cie {
            return Err("an advance in a CIE's initial instructions".to_string());
        }
        rows.push((*loc, row.clone()));
        *loc = loc.wrapping_add(delta.wrapping_mul(code_align));
        Ok(())
    };
    let restore = |reg: u64, row: &mut Row| -> Result<(), String> {
        let init = initial.ok_or("DW_CFA_restore in a CIE")?;
        match init.regs.get(&reg) {
            Some(x) => row.regs.insert(reg, x.clone()),
            None => row.regs.remove(&reg),
        };
        Ok(())
    };
    let off = |n: i64| n.wrapping_mul(data_align);
    while r.pos < insns.len() {
        let op = r.u8()?;
        match (op >> 6, op & 0x3f) {
            (1, d) => advance(u64::from(d), row, rows)?,
            (2, reg) => {
                let n = r.uleb()? as i64;
                row.regs.insert(u64::from(reg), RegRule::Offset(off(n)));
            }
            (3, reg) => restore(u64::from(reg), row)?,
            _ => match op {
                0x00 => {}
                0x01 => return Err("DW_CFA_set_loc".into()),
                0x02 => advance(r.uint(1)?, row, rows)?,
                0x03 => advance(r.uint(2)?, row, rows)?,
                0x04 => advance(r.uint(4)?, row, rows)?,
                0x05 => {
                    let (reg, n) = (r.uleb()?, r.uleb()? as i64);
                    row.regs.insert(reg, RegRule::Offset(off(n)));
                }
                0x06 => {
                    let reg = r.uleb()?;
                    restore(reg, row)?;
                }
                0x07 => {
                    row.regs.insert(r.uleb()?, RegRule::Undefined);
                }
                0x08 => {
                    row.regs.insert(r.uleb()?, RegRule::Same);
                }
                0x09 => {
                    let (reg, _) = (r.uleb()?, r.uleb()?);
                    row.regs.insert(reg, RegRule::Other(op));
                }
                0x0a => stack.push(row.clone()),
                0x0b => {
                    // The CFA rule comes back with the registers', as the
                    // unwinders do (libgcc's `unwind-dw2.c`, gdb, LLVM's
                    // libunwind) and GCC's epilogues expect.
                    *row = stack.pop().ok_or("DW_CFA_restore_state with nothing remembered")?;
                }
                0x0c => {
                    let (reg, n) = (r.uleb()?, r.uleb()?);
                    row.cfa = Some(Cfa::Reg(reg, n as i64));
                }
                0x0d => {
                    let reg = r.uleb()?;
                    match &mut row.cfa {
                        Some(Cfa::Reg(x, _)) => *x = reg,
                        _ => return Err("DW_CFA_def_cfa_register without a register rule".into()),
                    }
                }
                0x0e => {
                    let n = r.uleb()? as i64;
                    match &mut row.cfa {
                        Some(Cfa::Reg(_, x)) => *x = n,
                        _ => return Err("DW_CFA_def_cfa_offset without a register rule".into()),
                    }
                }
                0x0f => row.cfa = Some(Cfa::Expr(r.block()?)),
                0x10 | 0x16 => {
                    let reg = r.uleb()?;
                    r.block()?;
                    row.regs.insert(reg, RegRule::Other(op));
                }
                0x11 => {
                    let (reg, n) = (r.uleb()?, r.sleb()?);
                    row.regs.insert(reg, RegRule::Offset(off(n)));
                }
                0x12 => {
                    let (reg, n) = (r.uleb()?, r.sleb()?);
                    row.cfa = Some(Cfa::Reg(reg, off(n)));
                }
                0x13 => {
                    let n = r.sleb()?;
                    match &mut row.cfa {
                        Some(Cfa::Reg(_, x)) => *x = off(n),
                        _ => return Err("DW_CFA_def_cfa_offset_sf without a register rule".into()),
                    }
                }
                0x14 | 0x15 => {
                    let reg = r.uleb()?;
                    if op == 0x14 {
                        r.uleb()?;
                    } else {
                        r.sleb()?;
                    }
                    row.regs.insert(reg, RegRule::Other(op));
                }
                // DW_CFA_GNU_args_size: no rule.
                0x2e => {
                    r.uleb()?;
                }
                _ => return Err(format!("call frame instruction {op:#x}")),
            },
        }
    }
    Ok(())
}

/// Evaluates a DWARF expression to its value (the top of the stack), the
/// registers read through `reg`. `None`: an operation outside the
/// arithmetic, comparison and stack subset, a register `reg` does not
/// know, a division by zero, or an empty stack.
#[allow(clippy::too_many_lines)] // one opcode table
pub(crate) fn eval(expr: &[u8], reg: &dyn Fn(u64) -> Option<u64>) -> Option<u64> {
    let mut r = Rd { b: expr, pos: 0 };
    let mut s: Vec<u64> = Vec::new();
    let mut steps = 0usize;
    while r.pos < expr.len() {
        steps += 1;
        if steps > 4096 {
            return None;
        }
        let op = r.u8().ok()?;
        let bin = |s: &mut Vec<u64>, f: &dyn Fn(u64, u64) -> Option<u64>| -> Option<()> {
            let b = s.pop()?;
            let a = s.pop()?;
            s.push(f(a, b)?);
            Some(())
        };
        let cmp = |s: &mut Vec<u64>, f: fn(i64, i64) -> bool| -> Option<()> {
            let b = s.pop()? as i64;
            let a = s.pop()? as i64;
            s.push(u64::from(f(a, b)));
            Some(())
        };
        match op {
            0x08 => s.push(r.uint(1).ok()?),
            0x09 => s.push(i64::from(r.uint(1).ok()? as u8 as i8) as u64),
            0x0a => s.push(r.uint(2).ok()?),
            0x0b => s.push(i64::from(r.uint(2).ok()? as u16 as i16) as u64),
            0x0c => s.push(r.uint(4).ok()?),
            0x0d => s.push(i64::from(r.uint(4).ok()? as u32 as i32) as u64),
            0x0e | 0x0f => s.push(r.uint(8).ok()?),
            0x10 => s.push(r.uleb().ok()?),
            0x11 => s.push(r.sleb().ok()? as u64),
            0x12 => s.push(*s.last()?),
            0x13 => {
                s.pop()?;
            }
            0x14 => s.push(*s.get(s.len().checked_sub(2)?)?),
            0x15 => {
                let i = usize::from(r.u8().ok()?);
                s.push(*s.get(s.len().checked_sub(i + 1)?)?);
            }
            0x16 => {
                let n = s.len();
                if n < 2 {
                    return None;
                }
                s.swap(n - 1, n - 2);
            }
            0x17 => {
                let n = s.len();
                if n < 3 {
                    return None;
                }
                // a b c -> c a b (the top moves to third).
                let top = s.remove(n - 1);
                s.insert(n - 3, top);
            }
            0x19 => {
                let a = s.pop()? as i64;
                s.push(a.wrapping_abs() as u64);
            }
            0x1a => bin(&mut s, &|a, b| Some(a & b))?,
            0x1b => bin(&mut s, &|a, b| (a as i64).checked_div(b as i64).map(|v| v as u64))?,
            0x1c => bin(&mut s, &|a, b| Some(a.wrapping_sub(b)))?,
            0x1d => bin(&mut s, &|a, b| a.checked_rem(b))?,
            0x1e => bin(&mut s, &|a, b| Some(a.wrapping_mul(b)))?,
            0x1f => {
                let a = s.pop()?;
                s.push(a.wrapping_neg());
            }
            0x20 => {
                let a = s.pop()?;
                s.push(!a);
            }
            0x21 => bin(&mut s, &|a, b| Some(a | b))?,
            0x22 => bin(&mut s, &|a, b| Some(a.wrapping_add(b)))?,
            0x23 => {
                let n = r.uleb().ok()?;
                let a = s.pop()?;
                s.push(a.wrapping_add(n));
            }
            0x24 => bin(&mut s, &|a, b| Some(if b >= 64 { 0 } else { a << b }))?,
            0x25 => bin(&mut s, &|a, b| Some(if b >= 64 { 0 } else { a >> b }))?,
            0x26 => bin(&mut s, &|a, b| Some(((a as i64) >> b.min(63)) as u64))?,
            0x27 => bin(&mut s, &|a, b| Some(a ^ b))?,
            0x29 => cmp(&mut s, |a, b| a == b)?,
            0x2a => cmp(&mut s, |a, b| a >= b)?,
            0x2b => cmp(&mut s, |a, b| a > b)?,
            0x2c => cmp(&mut s, |a, b| a <= b)?,
            0x2d => cmp(&mut s, |a, b| a < b)?,
            0x2e => cmp(&mut s, |a, b| a != b)?,
            0x30..=0x4f => s.push(u64::from(op - 0x30)),
            0x70..=0x8f => {
                let n = r.sleb().ok()?;
                s.push(reg(u64::from(op - 0x70))?.wrapping_add_signed(n));
            }
            0x92 => {
                let (x, n) = (r.uleb().ok()?, r.sleb().ok()?);
                s.push(reg(x)?.wrapping_add_signed(n));
            }
            0x96 => {}
            _ => return None,
        }
    }
    s.pop()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GNU ld's x86-64 lazy PLT FDE (`elf_x86_64_eh_frame_lazy_plt`) on
    /// the CIE crt1.o gives (`def_cfa rsp+8; offset rip at cfa-8`).
    #[test]
    fn s6_4_gnu_ld_plt_program() {
        let cie = [0x0c, 0x07, 0x08, 0x90, 0x01];
        let fde = [
            0x0e, 0x10, 0x46, 0x0e, 0x18, 0x4a, 0x0f, 0x0b, 0x77, 0x08, 0x80, 0x00, 0x3f, 0x1a,
            0x3b, 0x2a, 0x33, 0x24, 0x22,
        ];
        let rows = run(&cie, &fde, 1, -8, 0x1020).unwrap();
        let cfa = |pc: u64| {
            let row = row_at(&rows, pc).unwrap();
            assert_eq!(row.regs.get(&16), Some(&RegRule::Offset(-8)));
            let rsp = 0x7000_0000u64;
            match row.cfa.as_ref().unwrap() {
                Cfa::Reg(7, n) => rsp.wrapping_add_signed(*n) - rsp,
                Cfa::Expr(e) => {
                    let reg = |x: u64| match x {
                        7 => Some(rsp),
                        16 => Some(pc),
                        _ => None,
                    };
                    eval(e, &reg).unwrap() - rsp
                }
                c @ Cfa::Reg(..) => panic!("{c:?}"),
            }
        };
        assert_eq!(cfa(0x1020), 16);
        assert_eq!(cfa(0x1026), 24);
        assert_eq!(cfa(0x1030), 8);
        assert_eq!(cfa(0x103a), 8);
        assert_eq!(cfa(0x103b), 16);
        assert_eq!(cfa(0x1040), 8);
        assert_eq!(cfa(0x104b), 16);
    }

    #[test]
    fn s6_4_remember_restore_and_unknown() {
        let rows = run(&[0x0c, 0x07, 0x08], &[0x0a, 0x86, 0x02, 0x41, 0x0b], 1, -8, 0).unwrap();
        assert_eq!(row_at(&rows, 0).unwrap().regs.get(&6), Some(&RegRule::Offset(-16)));
        assert_eq!(row_at(&rows, 1).unwrap().regs.get(&6), None);
        assert!(run(&[0x41], &[], 1, -8, 0).is_err());
        assert!(run(&[], &[0x01, 0, 0, 0, 0], 1, -8, 0).is_err());
        assert_eq!(eval(&[0x31, 0x32, 0x22], &|_| None), Some(3));
        assert_eq!(eval(&[0x70, 0x00], &|_| None), None);
        assert_eq!(eval(&[0x06], &|_| Some(0)), None);
    }
}
