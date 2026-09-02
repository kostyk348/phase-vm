//! Набор инструкций phase-vm.
//!
//! Инвариант обратимости: **каждая инструкция модифицирует ровно один
//! destination (регистр или слово памяти), все источники остаются
//! нетронутыми** и достаточны для обращения из текущего состояния. Никаких
//! логов — источник и есть «лог».
//!
//! Необратимости (границы): `Set` и `MSet` — уничтожают старое значение без
//! источника.

use crate::state::State;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inst {
    Nop,
    /// `r = !r` — самобратная.
    Not(u8),
    /// `r += 1` (wrap) — обратная Dec.
    Inc(u8),
    /// `r -= 1` (wrap) — обратная Inc.
    Dec(u8),
    /// `d ^= s` — самобратная. dst != src.
    Xor(u8, u8),
    /// `d += s` (wrap) — обратная Sub. dst != src.
    Add(u8, u8),
    /// `d -= s` (wrap) — обратная Add. dst != src.
    Sub(u8, u8),
    /// swap(a,b) — самобратная.
    Swap(u8, u8),
    /// `r = rotl(r, k)` — обратная RotR(r, k).
    RotL(u8, u32),
    /// `r = rotr(r, k)` — обратная RotL(r, k).
    RotR(u8, u32),
    /// `t ^= c1 & c2` (поразрядно) — самобратная. t не должен алиасить c1/c2.
    Toff(u8, u8, u8),
    /// Если бит0(c): swap(a,b). Самобратная. c не должен алиасить a/b.
    CSwap(u8, u8, u8),
    /// `r = imm` — **необратимо** (граница): уничтожает старое значение.
    Set(u8, u64),

    // --- Память (слово-адресуемая, mem = Vec<u64>) ---
    /// `mem[r_i] += r_v` — dst память, источник регистр цел. Обратная MSub.
    MAdd(u8, u8),
    /// `mem[r_i] -= r_v` — обратная MAdd.
    MSub(u8, u8),
    /// `mem[r_i] ^= r_v` — самобратная.
    MXor(u8, u8),
    /// `r += mem[r_i]` — dst регистр, память цела (источник). Обратная RSub.
    RAdd(u8, u8),
    /// `r -= mem[r_i]` — обратная RAdd.
    RSub(u8, u8),
    /// `mem[addr] = imm` — **необратимо** (граница).
    MSet(u64, u64),
}

impl Inst {
    /// Обратная инструкция. `None` для необратимых (границ).
    pub fn inverse(self) -> Option<Inst> {
        use Inst::*;
        Some(match self {
            Nop => Nop,
            Not(r) => Not(r),
            Inc(r) => Dec(r),
            Dec(r) => Inc(r),
            Xor(d, s) => Xor(d, s),
            Add(d, s) => Sub(d, s),
            Sub(d, s) => Add(d, s),
            Swap(a, b) => Swap(a, b),
            RotL(r, k) => RotR(r, k),
            RotR(r, k) => RotL(r, k),
            Toff(a, b, t) => Toff(a, b, t),
            CSwap(c, a, b) => CSwap(c, a, b),
            MAdd(a, v) => MSub(a, v),
            MSub(a, v) => MAdd(a, v),
            MXor(a, v) => MXor(a, v),
            RAdd(r, a) => RSub(r, a),
            RSub(r, a) => RAdd(r, a),
            Set(..) => return None,
            MSet(..) => return None,
        })
    }

    pub fn is_irreversible(&self) -> bool {
        matches!(self, Inst::Set(..) | Inst::MSet(..))
    }

    /// Проверка алиасинга операндов, делающего операцию небиективной.
    pub fn operand_alias(&self) -> Option<&'static str> {
        use Inst::*;
        match *self {
            Xor(d, s) | Add(d, s) | Sub(d, s) if d == s => Some("dst == src: операция небиективна"),
            Toff(c1, c2, t) if t == c1 || t == c2 => Some("target алиасит control: небиективно"),
            CSwap(c, a, b) if c == a || c == b => {
                Some("control алиасит переставляемый регистр: небиективно")
            }
            MAdd(a, v) | MSub(a, v) | MXor(a, v) if a == v => {
                Some("index == value: mem[..] ^= mem[..]-индекс сам себя портит")
            }
            RAdd(r, a) | RSub(r, a) if r == a => Some("reg == index: небиективно"),
            _ => None,
        }
    }

    /// Применить инструкцию к состоянию (прямое направление).
    pub fn exec(&self, state: &mut State) -> Result<(), String> {
        use Inst::*;
        let nregs = state.regs.len();
        let at = |r: u8| -> Result<usize, String> {
            let i = r as usize;
            if i >= nregs {
                return Err(format!("регистр r{} вне диапазона ({})", r, nregs));
            }
            Ok(i)
        };
        let at_mem = |idx: u64| -> Result<usize, String> {
            let i = idx as usize;
            if i >= state.mem.len() {
                return Err(format!(
                    "адрес памяти {} вне диапазона ({})",
                    idx,
                    state.mem.len()
                ));
            }
            Ok(i)
        };
        match *self {
            Nop => {}
            Not(r) => {
                let i = at(r)?;
                state.regs[i] = !state.regs[i];
            }
            Inc(r) => {
                let i = at(r)?;
                state.regs[i] = state.regs[i].wrapping_add(1);
            }
            Dec(r) => {
                let i = at(r)?;
                state.regs[i] = state.regs[i].wrapping_sub(1);
            }
            Xor(d, s) => {
                let i = at(d)?;
                let j = at(s)?;
                state.regs[i] ^= state.regs[j];
            }
            Add(d, s) => {
                let i = at(d)?;
                let j = at(s)?;
                state.regs[i] = state.regs[i].wrapping_add(state.regs[j]);
            }
            Sub(d, s) => {
                let i = at(d)?;
                let j = at(s)?;
                state.regs[i] = state.regs[i].wrapping_sub(state.regs[j]);
            }
            Swap(a, b) => {
                let i = at(a)?;
                let j = at(b)?;
                state.regs.swap(i, j);
            }
            RotL(r, k) => {
                let i = at(r)?;
                state.regs[i] = state.regs[i].rotate_left(k & 63);
            }
            RotR(r, k) => {
                let i = at(r)?;
                state.regs[i] = state.regs[i].rotate_right(k & 63);
            }
            Toff(c1, c2, t) => {
                let i = at(c1)?;
                let j = at(c2)?;
                let k = at(t)?;
                state.regs[k] ^= state.regs[i] & state.regs[j];
            }
            CSwap(c, a, b) => {
                let i = at(c)?;
                let j = at(a)?;
                let k = at(b)?;
                if state.regs[i] & 1 == 1 {
                    state.regs.swap(j, k);
                }
            }
            Set(r, imm) => {
                let i = at(r)?;
                state.regs[i] = imm;
            }
            MAdd(a, v) => {
                let i = at_mem(state.regs[at(a)?])?;
                let j = at(v)?;
                state.mem[i] = state.mem[i].wrapping_add(state.regs[j]);
            }
            MSub(a, v) => {
                let i = at_mem(state.regs[at(a)?])?;
                let j = at(v)?;
                state.mem[i] = state.mem[i].wrapping_sub(state.regs[j]);
            }
            MXor(a, v) => {
                let i = at_mem(state.regs[at(a)?])?;
                let j = at(v)?;
                state.mem[i] ^= state.regs[j];
            }
            RAdd(r, a) => {
                let j = at(r)?;
                let i = at_mem(state.regs[at(a)?])?;
                state.regs[j] = state.regs[j].wrapping_add(state.mem[i]);
            }
            RSub(r, a) => {
                let j = at(r)?;
                let i = at_mem(state.regs[at(a)?])?;
                state.regs[j] = state.regs[j].wrapping_sub(state.mem[i]);
            }
            MSet(addr, imm) => {
                let i = at_mem(addr)?;
                state.mem[i] = imm;
            }
        }
        Ok(())
    }

    /// Мнемоника для текстового формата.
    pub fn mnemonic(&self) -> &'static str {
        use Inst::*;
        match self {
            Nop => "nop",
            Not(_) => "not",
            Inc(_) => "inc",
            Dec(_) => "dec",
            Xor(..) => "xor",
            Add(..) => "add",
            Sub(..) => "sub",
            Swap(..) => "swp",
            RotL(..) => "rotl",
            RotR(..) => "rotr",
            Toff(..) => "toff",
            CSwap(..) => "cswp",
            Set(..) => "set",
            MAdd(..) => "madd",
            MSub(..) => "msub",
            MXor(..) => "mxor",
            RAdd(..) => "rmadd",
            RSub(..) => "rmsub",
            MSet(..) => "mset",
        }
    }
}

impl fmt::Display for Inst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Inst::*;
        match *self {
            Nop => write!(f, "nop"),
            Not(r) => write!(f, "not r{}", r),
            Inc(r) => write!(f, "inc r{}", r),
            Dec(r) => write!(f, "dec r{}", r),
            Xor(d, s) => write!(f, "xor r{} r{}", d, s),
            Add(d, s) => write!(f, "add r{} r{}", d, s),
            Sub(d, s) => write!(f, "sub r{} r{}", d, s),
            Swap(a, b) => write!(f, "swp r{} r{}", a, b),
            RotL(r, k) => write!(f, "rotl r{} {}", r, k),
            RotR(r, k) => write!(f, "rotr r{} {}", r, k),
            Toff(a, b, t) => write!(f, "toff r{} r{} r{}", a, b, t),
            CSwap(c, a, b) => write!(f, "cswp r{} r{} r{}", c, a, b),
            Set(r, imm) => write!(f, "set r{} {:#x}", r, imm),
            MAdd(a, v) => write!(f, "madd r{} r{}", a, v),
            MSub(a, v) => write!(f, "msub r{} r{}", a, v),
            MXor(a, v) => write!(f, "mxor r{} r{}", a, v),
            RAdd(r, a) => write!(f, "rmadd r{} r{}", r, a),
            RSub(r, a) => write!(f, "rmsub r{} r{}", r, a),
            MSet(addr, imm) => write!(f, "mset {} {:#x}", addr, imm),
        }
    }
}
