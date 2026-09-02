//! Машина: исполнение вперёд/назад, потоковая развёртка `Rep`, откат суффикса.
//!
//! Поток листьев не материализуется: `Rep` раскрывается на лету стеком джобов.
//! Это позволяет гонять огромные `rep` (бенчмарки) без аллокаций.

use crate::inst::Inst;
use crate::program::Node;
use crate::state::State;

/// Подсчёт листьев (инструкций) в развёрнутом потоке, saturating.
pub fn count_leaves(nodes: &[Node]) -> u64 {
    let mut total = 0u64;
    for n in nodes {
        match n {
            Node::Inst(..) => total = total.saturating_add(1),
            Node::Rep { count, body } => {
                total = total.saturating_add(count.saturating_mul(count_leaves(body)));
            }
        }
    }
    total
}

enum Job<'a> {
    /// Курсор по последовательности узлов (направление задано флагом).
    Seq {
        nodes: &'a [Node],
        idx: isize,
        forward: bool,
    },
    /// Оставшиеся повторы тела (само тело кладётся сверху как Seq).
    Rep { body: &'a [Node], count: u64 },
}

/// Потоковый итератор листьев в прямом порядке.
pub fn iter_forward<'a>(nodes: &'a [Node]) -> impl Iterator<Item = Inst> + 'a {
    StreamIter {
        jobs: vec![Job::Seq {
            nodes,
            idx: 0,
            forward: true,
        }],
        forward: true,
    }
}

/// Потоковый итератор листьев в обратном порядке (сами инструкции,
/// инверсию применяет вызывающий).
pub fn iter_reverse<'a>(nodes: &'a [Node]) -> impl Iterator<Item = Inst> + 'a {
    let n = nodes.len() as isize;
    StreamIter {
        jobs: vec![Job::Seq {
            nodes,
            idx: n - 1,
            forward: false,
        }],
        forward: false,
    }
}

struct StreamIter<'a> {
    jobs: Vec<Job<'a>>,
    forward: bool,
}

impl<'a> Iterator for StreamIter<'a> {
    type Item = Inst;

    fn next(&mut self) -> Option<Inst> {
        // Решение по верхнему джобу вычисляется без удержания заимствования
        // (срезы &'a [Node] копируются), затем применяется мутация.
        loop {
            enum Act<'a> {
                Pop,
                /// Заменить верхний Seq на idx=new_idx и выдать инструкцию.
                YieldInst {
                    new_idx: isize,
                    inst: Inst,
                },
                /// Заменить верхний Seq на idx=new_idx и положить Rep{body,count}.
                PushRep {
                    new_idx: isize,
                    body: &'a [Node],
                    count: u64,
                },
            }

            let act: Act = match self.jobs.last() {
                None => return None,
                Some(Job::Seq {
                    nodes,
                    idx,
                    forward,
                }) => {
                    let nodes = *nodes;
                    let idx = *idx;
                    let fwd = *forward;
                    let exhausted = if fwd {
                        idx >= nodes.len() as isize
                    } else {
                        idx < 0
                    };
                    if exhausted {
                        Act::Pop
                    } else {
                        let u = idx as usize;
                        match &nodes[u] {
                            Node::Inst(inst) => {
                                let nidx = if fwd { idx + 1 } else { idx - 1 };
                                Act::YieldInst {
                                    new_idx: nidx,
                                    inst: *inst,
                                }
                            }
                            Node::Rep { count, body } => {
                                let nidx = if fwd { idx + 1 } else { idx - 1 };
                                if *count == 0 {
                                    // пустой rep: просто перешагнуть
                                    match self.jobs.last_mut().unwrap() {
                                        Job::Seq { idx, .. } => *idx = nidx,
                                        _ => unreachable!(),
                                    }
                                    continue;
                                }
                                Act::PushRep {
                                    new_idx: nidx,
                                    body,
                                    count: *count,
                                }
                            }
                        }
                    }
                }
                Some(Job::Rep { body, count }) => {
                    let body = *body;
                    let count = *count;
                    if count == 0 {
                        Act::Pop
                    } else {
                        match self.jobs.last_mut().unwrap() {
                            Job::Rep { count, .. } => *count -= 1,
                            _ => unreachable!(),
                        }
                        self.jobs.push(Job::Seq {
                            nodes: body,
                            idx: if self.forward {
                                0
                            } else {
                                body.len() as isize - 1
                            },
                            forward: self.forward,
                        });
                        continue;
                    }
                }
            };

            match act {
                Act::Pop => {
                    self.jobs.pop();
                }
                Act::YieldInst { new_idx, inst } => {
                    match self.jobs.last_mut().unwrap() {
                        Job::Seq { idx, .. } => *idx = new_idx,
                        _ => unreachable!(),
                    }
                    return Some(inst);
                }
                Act::PushRep {
                    new_idx,
                    body,
                    count,
                } => {
                    match self.jobs.last_mut().unwrap() {
                        Job::Seq { idx, .. } => *idx = new_idx,
                        _ => unreachable!(),
                    }
                    self.jobs.push(Job::Rep { body, count });
                }
            }
        }
    }
}

/// Исполнить программу вперёд.
pub fn run_forward(state: &mut State, nodes: &[Node]) -> Result<u64, String> {
    let mut k = 0u64;
    for inst in iter_forward(nodes) {
        inst.exec(state)
            .map_err(|e| format!("лист #{k} ({inst}): {e}"))?;
        k += 1;
    }
    Ok(k)
}

/// Обратима ли инструкция для отката (есть инверсия и нет алиасинга).
fn invertible_for_rollback(inst: &Inst) -> bool {
    inst.inverse().is_some() && inst.operand_alias().is_none()
}

/// Длина обратимого суффикса: сколько листьев от конца можно откатить,
/// пока не встретится граница (`Set`) или небиективная инструкция.
pub fn reversible_suffix_len(nodes: &[Node]) -> u64 {
    let mut len = 0u64;
    for inst in iter_reverse(nodes) {
        if !invertible_for_rollback(&inst) {
            break;
        }
        len += 1;
    }
    len
}

/// Откатить до `n` листьев от конца (или весь обратимый суффикс).
/// Возвращает число реально откаченных листьев. Останавливается на границе.
pub fn reverse_n(state: &mut State, nodes: &[Node], n: u64) -> Result<u64, String> {
    let mut done = 0u64;
    for inst in iter_reverse(nodes) {
        if done >= n {
            break;
        }
        let inv = match inst.inverse() {
            Some(i) if inst.operand_alias().is_none() => i,
            _ => break, // граница: дальше откатывать нельзя
        };
        inv.exec(state)
            .map_err(|e| format!("обратный лист #{done} ({inv}): {e}"))?;
        done += 1;
    }
    Ok(done)
}

/// Полный откат: откатить весь обратимый суффикс.
pub fn reverse_all(state: &mut State, nodes: &[Node]) -> Result<u64, String> {
    reverse_n(state, nodes, u64::MAX)
}

/// Инструкция на позиции `idx` в развёрнутом потоке (для отладчика).
/// O(idx) — поиск без индекса.
pub fn leaf_at(nodes: &[Node], idx: u64) -> Option<Inst> {
    iter_forward(nodes).nth(idx as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::parse;

    fn prog(src: &str) -> Vec<Node> {
        parse(src, 16).unwrap().nodes
    }

    #[test]
    fn count_matches_expansion() {
        let nodes = prog("set r0 1\nrep 5\nadd r0 r1\nxor r0 r2\nend\nset r1 2\n");
        // 1 + 5*2 + 1 = 12
        assert_eq!(count_leaves(&nodes), 12);
    }

    #[test]
    fn iter_reverse_matches_reversed_forward() {
        let nodes = prog("set r0 1\nrep 3\nadd r0 r1\nxor r0 r2\nend\n");
        let fwd: Vec<Inst> = iter_forward(&nodes).collect();
        let rev: Vec<Inst> = iter_reverse(&nodes).collect();
        let mut rev_expect = fwd.clone();
        rev_expect.reverse();
        assert_eq!(rev, rev_expect);
    }

    #[test]
    fn forward_then_reverse_restores_state() {
        let nodes = prog(
            "set r0 0x1234\nset r1 7\nset r2 0\nrep 4\nadd r0 r1\nrotl r0 3\nxor r0 r2\nend\n",
        );
        // после последнего Set граница; ядро обратимо
        let mut s = State::new(16, 0);
        run_forward(&mut s, &nodes).unwrap();
        let after = s.clone();
        let suffix = reversible_suffix_len(&nodes);
        assert_eq!(suffix, 4 * 3);
        let done = reverse_all(&mut s, &nodes).unwrap();
        assert_eq!(done, suffix);
        // возврат к состоянию после границ: r0=0x1234, r1=7, r2=0
        assert_eq!(s.regs[0], 0x1234);
        assert_eq!(s.regs[1], 7);
        assert_eq!(s.regs[2], 0);
        // и это НЕ исходное нулевое состояние (граница не откатывалась)
        assert!(after.regs != s.regs);
    }

    #[test]
    fn pure_reversible_roundtrip_from_random() {
        // Без Set вообще: откат обязан вернуть в точности случайное состояние.
        let nodes = prog("rep 5\nadd r0 r1\nrotl r1 3\nxor r1 r0\nswp r0 r1\nend\n");
        for seed in 1..50u64 {
            let mut s = State::random(16, 0, seed);
            let original = s.clone();
            run_forward(&mut s, &nodes).unwrap();
            assert_ne!(s.regs, original.regs, "seed {seed}: нет изменений");
            let done = reverse_all(&mut s, &nodes).unwrap();
            assert_eq!(done, 5 * 4);
            assert_eq!(s, original, "seed {seed}: F⁻¹(F(S)) != S");
        }
    }

    #[test]
    fn partial_reverse_steps() {
        let nodes = prog("rep 2\ninc r0\nend\n"); // +2
        let mut s = State::new(4, 0);
        run_forward(&mut s, &nodes).unwrap();
        assert_eq!(s.regs[0], 2);
        let done = reverse_n(&mut s, &nodes, 1).unwrap();
        assert_eq!(done, 1);
        assert_eq!(s.regs[0], 1);
    }

    #[test]
    fn boundary_stops_rollback() {
        // set посреди ядра: суффикс после него откатывается, до него — нет.
        let nodes = prog("inc r0\nset r1 5\ninc r0\n");
        let mut s = State::new(4, 0);
        run_forward(&mut s, &nodes).unwrap();
        assert_eq!((s.regs[0], s.regs[1]), (2, 5));
        assert_eq!(reversible_suffix_len(&nodes), 1);
        let done = reverse_all(&mut s, &nodes).unwrap();
        assert_eq!(done, 1);
        assert_eq!((s.regs[0], s.regs[1]), (1, 5)); // только последний inc откачен
    }
}
