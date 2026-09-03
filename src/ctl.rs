//! L3 — decision-журнал: структурный data-dependent control-flow с обратимостью.
//!
//! Аксиома PHASE: внутри фазы операторы биективны; data-dependent ветвление —
//! честная цена, и она платится **журналом решений, а не состоянием**:
//! `ifnz` пишет один флаг, `wz` пишет одно число итераций — O(решений),
//! никогда O(байт состояния) и не O(шагов).
//!
//! Семантика:
//!   - `ifnz rN … end` — тело выполняется, если rN != 0 (решение: флаг);
//!   - `wz  rN … end` — тело повторяется, пока rN != 0 (решение: счётчик);
//!   - `rep N … end` — статический цикл (журнала не требует).
//!
//! Вложенность произвольная. Forward пишет решение ПОСЛЕ тела (post-order) —
//! поэтому reverse читает журнал строго LIFO, без неоднозначностей слияния.
//!
//! Reverse не пересчитывает условия — он доверяет журналу и применяет
//! обратные операторы. Для контроля целостности можно включить сверку
//! условий (см. `reverse_checked`).

use crate::inst::Inst;
use crate::program::{parse_imm, parse_inst, parse_reg};
use crate::state::State;

#[derive(Debug, Clone)]
pub enum Node {
    Inst(Inst),
    /// Статический sweep-цикл: биекция, повторённая count раз.
    Rep {
        count: u64,
        body: Vec<Node>,
    },
    /// ifnz rN: тело iff rN != 0. Журнал: 1 флаг.
    IfN {
        reg: u8,
        body: Vec<Node>,
    },
    /// wz rN: тело, пока rN != 0. Журнал: 1 счётчик.
    WhileN {
        reg: u8,
        body: Vec<Node>,
    },
}

/// Предел итераций одного while (защита от незавершающихся программ).
const MAX_WHILE_ITERS: u64 = 1 << 24;

/// Разобрать программу L3. Ключевые слова: rep/end, ifnz rN/end, wz rN/end.
pub fn parse(text: &str, nregs: usize) -> Result<Vec<Node>, String> {
    #[derive(Clone, Copy)]
    enum Kind {
        Rep,
        If,
        While,
    }
    let mut root: Vec<Node> = Vec::new();
    let mut stack: Vec<(Kind, u64, u8, Vec<Node>)> = Vec::new(); // kind, count/_, reg/_, body
    let push = |stack: &mut Vec<(Kind, u64, u8, Vec<Node>)>, root: &mut Vec<Node>, node: Node| {
        if let Some(top) = stack.last_mut() {
            top.3.push(node);
        } else {
            root.push(node);
        }
    };

    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let err_at = |m: String| format!("строка {}: {}", lineno + 1, m);
        let toks: Vec<&str> = line.split_whitespace().collect();
        match toks[0].to_ascii_lowercase().as_str() {
            "rep" => {
                if toks.len() != 2 {
                    return Err(err_at("rep <count>".into()));
                }
                let count = parse_imm(toks[1]).map_err(err_at)?;
                stack.push((Kind::Rep, count, 0, Vec::new()));
            }
            "ifnz" => {
                if toks.len() != 2 {
                    return Err(err_at("ifnz rN".into()));
                }
                let reg = parse_reg(toks[1], nregs).map_err(err_at)?;
                stack.push((Kind::If, 0, reg, Vec::new()));
            }
            "wz" => {
                if toks.len() != 2 {
                    return Err(err_at("wz rN".into()));
                }
                let reg = parse_reg(toks[1], nregs).map_err(err_at)?;
                stack.push((Kind::While, 0, reg, Vec::new()));
            }
            "end" => {
                let (kind, count, reg, body) = stack
                    .pop()
                    .ok_or_else(|| err_at("end без rep/ifnz/wz".into()))?;
                let node = match kind {
                    Kind::Rep => Node::Rep { count, body },
                    Kind::If => Node::IfN { reg, body },
                    Kind::While => Node::WhileN { reg, body },
                };
                push(&mut stack, &mut root, node);
            }
            mnem => {
                let inst = parse_inst(mnem, &toks[1..], nregs).map_err(err_at)?;
                push(&mut stack, &mut root, Node::Inst(inst));
            }
        }
    }
    if !stack.is_empty() {
        return Err("незакрытый блок".into());
    }
    Ok(root)
}

fn seq_forward(
    s: &mut State,
    nodes: &[Node],
    j: &mut Vec<u64>,
    steps: &mut u64,
) -> Result<(), String> {
    for node in nodes {
        match node {
            Node::Inst(i) => {
                i.exec(s).map_err(|e| format!("{i}: {e}"))?;
                *steps += 1;
            }
            Node::Rep { count, body } => {
                for _ in 0..*count {
                    seq_forward(s, body, j, steps)?;
                }
            }
            Node::IfN { reg, body } => {
                let cond = s.regs[*reg as usize] != 0;
                if cond {
                    seq_forward(s, body, j, steps)?;
                }
                j.push(cond as u64); // решение ПОСЛЕ тела (post-order)
            }
            Node::WhileN { reg, body } => {
                let mut k = 0u64;
                while s.regs[*reg as usize] != 0 {
                    if k >= MAX_WHILE_ITERS {
                        return Err("wz: превышен предел итераций".into());
                    }
                    seq_forward(s, body, j, steps)?;
                    k += 1;
                }
                j.push(k);
            }
        }
    }
    Ok(())
}

fn seq_reverse(
    s: &mut State,
    nodes: &[Node],
    j: &mut Vec<u64>,
    steps: &mut u64,
) -> Result<(), String> {
    for node in nodes.iter().rev() {
        match node {
            Node::Inst(i) => {
                let inv = match i.inverse() {
                    Some(x) if i.operand_alias().is_none() => x,
                    _ => return Err(format!("{i}: граница (set/mset) — откат требует чекпоинта")),
                };
                inv.exec(s).map_err(|e| format!("{inv}: {e}"))?;
                *steps += 1;
            }
            Node::Rep { count, body } => {
                for _ in 0..*count {
                    seq_reverse(s, body, j, steps)?;
                }
            }
            Node::IfN { body, .. } => {
                let flag = j.pop().ok_or("ifnz: журнал пуст при reverse")?;
                if flag != 0 {
                    seq_reverse(s, body, j, steps)?;
                }
            }
            Node::WhileN { body, .. } => {
                let k = j.pop().ok_or("wz: журнал пуст при reverse")?;
                for _ in 0..k {
                    seq_reverse(s, body, j, steps)?;
                }
            }
        }
    }
    Ok(())
}

/// Forward с журналом решений. `journal` дополняется; `steps` — число листьев.
pub fn forward(
    s: &mut State,
    nodes: &[Node],
    journal: &mut Vec<u64>,
    steps: &mut u64,
) -> Result<(), String> {
    seq_forward(s, nodes, journal, steps)
}

/// Reverse с доверием журналу (LIFO). Журнал укорачивается по мере чтения.
/// После полного reverse журнал пуст — откат оставил ноль следов и в нём.
pub fn reverse(
    s: &mut State,
    nodes: &[Node],
    journal: &mut Vec<u64>,
    steps: &mut u64,
) -> Result<(), String> {
    seq_reverse(s, nodes, journal, steps)
}

/// Reverse со сверкой условий: после отката ifnz/wz заново проверяет условие
/// на восстановленном состоянии и сверяет с журналом (детект повреждений).
pub fn reverse_checked(
    s: &mut State,
    nodes: &[Node],
    journal: &mut Vec<u64>,
    steps: &mut u64,
) -> Result<(), String> {
    fn rev(s: &mut State, nodes: &[Node], j: &mut Vec<u64>, steps: &mut u64) -> Result<(), String> {
        for node in nodes.iter().rev() {
            match node {
                Node::Inst(i) => {
                    let inv = i.inverse().ok_or("граница")?;
                    inv.exec(s).map_err(|e| format!("{inv}: {e}"))?;
                    *steps += 1;
                }
                Node::Rep { count, body } => {
                    for _ in 0..*count {
                        rev(s, body, j, steps)?;
                    }
                }
                Node::IfN { reg, body } => {
                    let flag = j.pop().ok_or("ifnz: журнал пуст")?;
                    if flag != 0 {
                        rev(s, body, j, steps)?;
                    }
                    let cond_now = s.regs[*reg as usize] != 0;
                    assert_eq!(
                        cond_now as u64, flag,
                        "ifnz: условие разошлось с журналом — повреждён журнал"
                    );
                }
                Node::WhileN { body, .. } => {
                    let k = j.pop().ok_or("wz: журнал пуст")?;
                    for _ in 0..k {
                        rev(s, body, j, steps)?;
                    }
                    // условие-регистр после отката возвращается к исходному
                    // (доцикловому) значению — сверять с 0 нельзя
                }
            }
        }
        Ok(())
    }
    rev(s, nodes, journal, steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(t: &str, n: usize) -> Vec<Node> {
        parse(t, n).unwrap()
    }

    #[test]
    fn multiply_data_dependent_roundtrip() {
        // Умножение через while: число итераций = данные (множитель),
        // rep статически не смог бы. Журнал: 1 запись на весь цикл.
        let p = st("add r3 r1\nwz r3\n  add r2 r0\n  dec r3\nend\n", 8);
        let mut s = State::new(8, 0);
        s.regs[0] = 7;
        s.regs[1] = 12345;
        let orig = s.clone();
        let mut j = Vec::new();
        let mut steps = 0;
        forward(&mut s, &p, &mut j, &mut steps).unwrap();
        assert_eq!(s.regs[2], 7 * 12345);
        assert_eq!(s.regs[3], 0);
        assert_eq!(j.len(), 1, "журнал = O(решений): один счётчик цикла");
        assert_eq!(j[0], 12345);
        // reverse с доверием журналу
        let mut steps2 = 0;
        reverse(&mut s, &p, &mut j, &mut steps2).unwrap();
        assert_eq!(s, orig, "F⁻¹(F(S)) != S для data-dependent while");
        assert!(j.is_empty(), "журнал исчерпан — ноль следов");
    }

    #[test]
    fn ifnz_taken_and_skipped() {
        let p = st("add r2 r0\nifnz r1\n  add r2 r0\nend\nadd r2 r0\n", 8);
        for seed in 1..20u64 {
            let mut s = State::random(8, 0, seed);
            s.regs[1] &= 1; // 0 или 1
            let orig = s.clone();
            let mut j = Vec::new();
            let mut st_ = 0;
            forward(&mut s, &p, &mut j, &mut st_).unwrap();
            assert_eq!(j.len(), 1, "ifnz пишет ровно один флаг");
            let mut st2 = 0;
            reverse(&mut s, &p, &mut j, &mut st2).unwrap();
            assert_eq!(s, orig, "seed {seed}");
        }
    }

    #[test]
    fn while_skipped_when_zero() {
        let p = st("wz r1\n  add r2 r0\n  dec r1\nend\n", 8);
        let mut s = State::new(8, 0);
        s.regs[1] = 0;
        let orig = s.clone();
        let mut j = Vec::new();
        let mut st_ = 0;
        forward(&mut s, &p, &mut j, &mut st_).unwrap();
        assert_eq!(j, vec![0]);
        let mut st2 = 0;
        reverse(&mut s, &p, &mut j, &mut st2).unwrap();
        assert_eq!(s, orig);
    }

    #[test]
    fn nested_rep_ifnz_while_roundtrip() {
        // rep содержит while, while содержит ifnz — журнал строго LIFO.
        let p = st(
            "rep 3\n\
               add r0 r4\n\
               ifnz r1\n\
                 wz r2\n\
                   add r3 r5\n\
                   dec r2\n\
                 end\n\
               end\n\
               dec r1\n\
             end\n",
            8,
        );
        for seed in 1..100u64 {
            let mut s = State::random(8, 0, seed);
            s.regs[1] = seed % 4; // небольшие, чтобы циклы завершались
            s.regs[2] = seed % 5;
            let orig = s.clone();
            let mut j = Vec::new();
            let mut st_ = 0;
            forward(&mut s, &p, &mut j, &mut st_).unwrap();
            let mut st2 = 0;
            reverse_checked(&mut s, &p, &mut j, &mut st2).unwrap();
            assert_eq!(s, orig, "seed {seed}");
            assert!(j.is_empty());
        }
    }

    #[test]
    fn journal_corruption_detected() {
        let p = st("ifnz r1\n  add r2 r0\nend\n", 8);
        let mut s = State::new(8, 0);
        s.regs[1] = 1;
        let mut j = Vec::new();
        let mut st_ = 0;
        forward(&mut s, &p, &mut j, &mut st_).unwrap();
        let orig = State::new(8, 0); // r2 не должен был измениться
                                     // (1) обычный reverse с испорченным журналом: состояние НЕ вернётся
        let mut s1 = s.clone();
        j[0] = 0; // врём: «ветка не выполнялась»
        let mut st2 = 0;
        reverse(&mut s1, &p, &mut j, &mut st2).unwrap();
        assert_ne!(
            s1, orig,
            "с испорченным журналом откат не должен восстановить состояние"
        );
        // (2) reverse_checked ловит расхождение условий (debug_assert)
        let mut s2 = s.clone();
        let mut j2 = vec![0u64];
        let mut st3 = 0;
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            reverse_checked(&mut s2, &p, &mut j2, &mut st3).unwrap();
        }));
        assert!(
            res.is_err(),
            "reverse_checked обязан детектировать порчу журнала"
        );
    }
}
