//! Программа: AST (Inst | Rep) и текстовый формат.
//!
//! Текстовый формат — одна инструкция на строку, `#` — комментарий.
//! Регистры `r0..rN`, imm — десятичное, `0x…`, `0b…` или отрицательное
//! (уходит в u64 как two's complement).
//!
//! ```text
//! # граница (ввод)
//! set r0 5
//! set r1 7
//! # обратимое ядро
//! add r3 r0        # t = a        (r3 нулевой: свежая ячейка)
//! rep 4
//!   xor r0 r1
//!   rotl r1 7
//! end
//! ```

use crate::inst::Inst;

#[derive(Debug, Clone)]
pub enum Node {
    Inst(Inst),
    Rep { count: u64, body: Vec<Node> },
}

#[derive(Debug, Clone)]
pub struct Program {
    pub nodes: Vec<Node>,
    pub nregs: usize,
}

fn parse_imm(tok: &str) -> Result<u64, String> {
    let t = tok.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).map_err(|e| format!("imm '{tok}': {e}"))
    } else if let Some(bin) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        u64::from_str_radix(bin, 2).map_err(|e| format!("imm '{tok}': {e}"))
    } else if let Some(neg) = t.strip_prefix('-') {
        let v: u64 = neg.parse().map_err(|e| format!("imm '{tok}': {e}"))?;
        Ok(v.wrapping_neg())
    } else {
        t.parse().map_err(|e| format!("imm '{tok}': {e}"))
    }
}

fn parse_reg(tok: &str, nregs: usize) -> Result<u8, String> {
    let r = tok
        .strip_prefix('r')
        .or_else(|| tok.strip_prefix('R'))
        .ok_or_else(|| format!("ожидался регистр rN, получено '{tok}'"))?;
    let n: usize = r.parse().map_err(|e| format!("регистр '{tok}': {e}"))?;
    if n >= nregs {
        return Err(format!("регистр r{} вне диапазона (--regs {})", n, nregs));
    }
    Ok(n as u8)
}

fn push_node(
    rep_stack: &mut [(u64, Vec<Node>)],
    root: &mut Vec<Node>,
    node: Node,
) -> Result<(), String> {
    if let Some(top) = rep_stack.last_mut() {
        top.1.push(node);
    } else {
        root.push(node);
    }
    Ok(())
}

/// Разобрать программу из текста.
pub fn parse(text: &str, nregs: usize) -> Result<Program, String> {
    let mut root: Vec<Node> = Vec::new();
    // Стек открытых rep: (count, body)
    let mut rep_stack: Vec<(u64, Vec<Node>)> = Vec::new();

    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let err_at = |msg: String| format!("строка {}: {}", lineno + 1, msg);
        let toks: Vec<&str> = line.split_whitespace().collect();

        match toks[0].to_ascii_lowercase().as_str() {
            "rep" => {
                if toks.len() != 2 {
                    return Err(err_at("rep: ожидается 'rep <count>'".into()));
                }
                let count = parse_imm(toks[1]).map_err(err_at)?;
                rep_stack.push((count, Vec::new()));
            }
            "end" => {
                if toks.len() != 1 {
                    return Err(err_at("end: лишние токены".into()));
                }
                let (count, body) = rep_stack
                    .pop()
                    .ok_or_else(|| err_at("end без rep".into()))?;
                let node = Node::Rep { count, body };
                push_node(&mut rep_stack, &mut root, node)?;
            }
            mnem => {
                let inst = parse_inst(mnem, &toks[1..], nregs).map_err(err_at)?;
                push_node(&mut rep_stack, &mut root, Node::Inst(inst))?;
            }
        }
    }
    if !rep_stack.is_empty() {
        return Err("незакрытый rep".into());
    }
    Ok(Program { nodes: root, nregs })
}

fn parse_inst(mnem: &str, args: &[&str], nregs: usize) -> Result<Inst, String> {
    let need = |n: usize| -> Result<(), String> {
        if args.len() != n {
            return Err(format!(
                "{mnem}: ожидалось {n} аргументов, дано {}",
                args.len()
            ));
        }
        Ok(())
    };
    let r = |i: usize| parse_reg(args[i], nregs);
    let imm = |i: usize| parse_imm(args[i]);

    use Inst::*;
    Ok(match mnem {
        "nop" => {
            need(0)?;
            Nop
        }
        "not" => {
            need(1)?;
            Not(r(0)?)
        }
        "inc" => {
            need(1)?;
            Inc(r(0)?)
        }
        "dec" => {
            need(1)?;
            Dec(r(0)?)
        }
        "xor" => {
            need(2)?;
            Xor(r(0)?, r(1)?)
        }
        "add" => {
            need(2)?;
            Add(r(0)?, r(1)?)
        }
        "sub" => {
            need(2)?;
            Sub(r(0)?, r(1)?)
        }
        "swp" => {
            need(2)?;
            Swap(r(0)?, r(1)?)
        }
        "rotl" => {
            need(2)?;
            RotL(r(0)?, imm(1)? as u32)
        }
        "rotr" => {
            need(2)?;
            RotR(r(0)?, imm(1)? as u32)
        }
        "toff" => {
            need(3)?;
            Toff(r(0)?, r(1)?, r(2)?)
        }
        "cswp" => {
            need(3)?;
            CSwap(r(0)?, r(1)?, r(2)?)
        }
        "madd" => {
            need(2)?;
            MAdd(r(0)?, r(1)?)
        }
        "msub" => {
            need(2)?;
            MSub(r(0)?, r(1)?)
        }
        "mxor" => {
            need(2)?;
            MXor(r(0)?, r(1)?)
        }
        "rmadd" => {
            need(2)?;
            RAdd(r(0)?, r(1)?)
        }
        "rmsub" => {
            need(2)?;
            RSub(r(0)?, r(1)?)
        }
        "mset" => {
            need(2)?;
            MSet(imm(0)?, imm(1)?)
        }
        "set" => {
            need(2)?;
            Set(r(0)?, imm(1)?)
        }
        other => return Err(format!("неизвестная инструкция '{other}'")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple() {
        let p = parse("set r0 5\nadd r0 r1\nrep 3\n  xor r0 r1\nend\n", 8).unwrap();
        assert_eq!(p.nodes.len(), 3);
        match &p.nodes[2] {
            Node::Rep { count, body } => {
                assert_eq!(*count, 3);
                assert_eq!(body.len(), 1);
            }
            _ => panic!("ожидался rep"),
        }
    }

    #[test]
    fn parse_hex_and_neg() {
        let p = parse("set r0 0xff\nset r1 -1\nset r2 0b101\n", 8).unwrap();
        if let Node::Inst(Inst::Set(_, v)) = p.nodes[0] {
            assert_eq!(v, 255);
        } else {
            panic!()
        }
        if let Node::Inst(Inst::Set(_, v)) = p.nodes[1] {
            assert_eq!(v, u64::MAX);
        } else {
            panic!()
        }
        if let Node::Inst(Inst::Set(_, v)) = p.nodes[2] {
            assert_eq!(v, 5);
        } else {
            panic!()
        }
    }

    #[test]
    fn parse_errors() {
        assert!(parse("frob r0\n", 8).is_err());
        assert!(parse("rep 3\nset r0 1\n", 8).is_err()); // незакрытый rep
        assert!(parse("set r8 1\n", 8).is_err()); // вне диапазона
    }
}
