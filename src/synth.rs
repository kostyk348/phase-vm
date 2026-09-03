//! Ось B + superopt-наследие: синтез обратимых ядер по образцам.
//!
//! Дана спецификация: несколько пар (вход, выход) над k регистрами.
//! Ищем КРАТЧАЙШУЮ программу из обратимого пула операторов, которая
//! удовлетворяет образцам. BFS по глубине с отсечением по кортежу
//! промежуточных результатов на образцах. По построению любая найденная
//! программа обратима (пул биективен) — синтез автоматически даёт и F⁻¹.
//!
//! Это инструмент, а не демо: можно «компилировать» желаемое преобразование
//! в обратимый kernel phase-vm (задел под #16 reversible logic synthesis).

use crate::inst::Inst;
use crate::state::State;
use std::collections::HashMap;

/// Пул кандидатов для k регистров.
pub fn pool(k: u8) -> Vec<Inst> {
    let mut p = Vec::new();
    for a in 0..k {
        p.push(Inst::Not(a));
        for b in 0..k {
            if a != b {
                p.push(Inst::Xor(a, b));
                p.push(Inst::Add(a, b));
                p.push(Inst::Sub(a, b));
                if a < b {
                    p.push(Inst::Swap(a, b));
                }
            }
        }
        for rot in [1u32, 7, 13, 31] {
            p.push(Inst::RotL(a, rot));
        }
    }
    p
}

pub type Sample = (State, State);

#[cfg(test)]
fn apply_all(s: &mut State, prog: &[Inst]) {
    for i in prog {
        i.exec(s).unwrap();
    }
}

/// Синтез кратчайшей обратимой программы по образцам.
/// Возвращает None, если в пределах max_depth ничего не нашлось.
pub fn synthesize(samples: &[Sample], k: u8, max_depth: usize) -> Option<Vec<Inst>> {
    let pool = pool(k);
    // ключ = кортеж результатов на образцах (для отсечения дублей)
    let key = |st: &[State]| -> Vec<u64> { st.iter().map(|s| s.hash()).collect() };

    // BFS слоями: frontier хранит (программа, текущие состояния образцов)
    let init_states: Vec<State> = samples.iter().map(|(inp, _)| inp.clone()).collect();
    let mut visited: HashMap<Vec<u64>, usize> = HashMap::new();
    visited.insert(key(&init_states), 0);

    let mut frontier: Vec<(Vec<Inst>, Vec<State>)> = vec![(Vec::new(), init_states)];

    for depth in 0..max_depth {
        let mut next: Vec<(Vec<Inst>, Vec<State>)> = Vec::new();
        for (prog, states) in frontier {
            for op in &pool {
                let mut ns: Vec<State> = states.clone();
                for s in ns.iter_mut() {
                    op.exec(s).unwrap();
                }
                let mut np = prog.clone();
                np.push(*op);
                // проверили спецификацию?
                let ok = ns
                    .iter()
                    .zip(samples.iter())
                    .all(|(got, (_, want))| got.regs == want.regs);
                if ok && !np.is_empty() {
                    return Some(np);
                }
                let kk = key(&ns);
                if visited.contains_key(&kk) {
                    continue;
                }
                visited.insert(kk, depth + 1);
                next.push((np, ns));
            }
        }
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state2(a: u64, b: u64) -> State {
        let mut s = State::new(2, 0);
        s.regs[0] = a;
        s.regs[1] = b;
        s
    }

    #[test]
    fn finds_xor_swap() {
        // x^=y; y^=x; x^=y  — длина 3, если пул содержит Xor(0,1) и Xor(1,0)
        let mut samples = Vec::new();
        for &(a, b) in &[
            (1u64, 2u64),
            (7, 3),
            (0, 9),
            (0xDEAD, 0xBEEF),
            (u64::MAX, 1),
        ] {
            let inp = state2(a, b);
            let want = state2(b, a); // swap
            samples.push((inp, want));
        }
        let found = synthesize(&samples, 2, 6).expect("должен найти swap");
        assert!(found.len() <= 3, "нашёл не кратчайший: len={}", found.len());
        // проверим на удержанных (held-out) входах
        for &(a, b) in &[(42u64, 17u64), (12345, 6789)] {
            let mut s = state2(a, b);
            apply_all(&mut s, &found);
            assert_eq!((s.regs[0], s.regs[1]), (b, a));
        }
    }

    #[test]
    fn rediscover_random_short_program() {
        let mut rng_state = 0x5EEDu64;
        let mut next = move || {
            rng_state ^= rng_state >> 12;
            rng_state ^= rng_state << 25;
            rng_state ^= rng_state >> 27;
            rng_state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        let pool = pool(2);
        // случайная целевая программа длины 2
        let target = vec![
            pool[(next() % pool.len() as u64) as usize],
            pool[(next() % pool.len() as u64) as usize],
        ];
        let mut samples = Vec::new();
        for _ in 0..8 {
            let a = next();
            let b = next();
            let mut inp = state2(a, b);
            let mut want = inp.clone();
            apply_all(&mut want, &target);
            samples.push((inp.clone(), want));
            let _ = &mut inp;
        }
        let found = synthesize(&samples, 2, 3).expect("должен найти программу длины ≤2");
        assert!(found.len() <= 2);
        // held-out проверка эквивалентности
        for _ in 0..200 {
            let a = next();
            let b = next();
            let mut s1 = state2(a, b);
            let mut s2 = s1.clone();
            apply_all(&mut s1, &target);
            apply_all(&mut s2, &found);
            assert_eq!(s1.regs, s2.regs, "синтез не эквивалентен цели");
        }
    }

    #[test]
    fn synthesized_program_is_reversible() {
        let mut samples = Vec::new();
        for &(a, b) in &[(3u64, 5u64), (8, 13), (100, 200)] {
            samples.push((state2(a, b), state2(a + b, b))); // add r0 r1
        }
        let found = synthesize(&samples, 2, 2).unwrap();
        // roundtrip: forward из входа, затем обратный прогон даёт вход
        for &(a, b) in &[(3u64, 5u64), (8, 13)] {
            let mut s = state2(a, b);
            apply_all(&mut s, &found);
            for inst in found.iter().rev() {
                inst.inverse().unwrap().exec(&mut s).unwrap();
            }
            assert_eq!((s.regs[0], s.regs[1]), (a, b), "F⁻¹(F(S)) != S");
        }
    }
}
