//! Математическое усиление (1): алгебра ядра как ГРУППА биекций.
//!
//! Последовательность обратимых инструкций = слово в группе, порождённой
//! образующими ISA. Оптимизатор применяет ЗВУКОВЫЕ правила переписывания
//! (каждое правило — равенство биекций, тождество в группе):
//!   - соседние взаимно-обратные пары сокращаются (add↔sub, xor↔xor, …),
//!   - вращения одного регистра складываются (rotl k1 · rotl k2 = rotl k1+k2),
//!   - независимые инструкции можно переставлять (будущее: глобальный поиск).
//! Стек-алгоритм корректен: удаляемая пара тождественна, значит функция
//! программы не меняется. Свойство-тест: opt(P) ≡ P на случайных состояниях
//! (и вперёд, и обратно).

use crate::inst::Inst;

/// Обратная пара для соседнего сокращения.
fn cancel_pair(a: Inst, b: Inst) -> bool {
    use Inst::*;
    match (a, b) {
        (Add(d1, s1), Sub(d2, s2)) | (Sub(d1, s1), Add(d2, s2)) => d1 == d2 && s1 == s2,
        (Xor(d1, s1), Xor(d2, s2)) => d1 == d2 && s1 == s2,
        (Swap(a1, b1), Swap(a2, b2)) => {
            // swap симметричен: swap(0,1) и swap(1,0) — одно и то же
            let (x1, y1) = (a1.min(b1), a1.max(b1));
            let (x2, y2) = (a2.min(b2), a2.max(b2));
            x1 == x2 && y1 == y2
        }
        (Not(r1), Not(r2)) => r1 == r2,
        (Inc(r1), Dec(r2)) | (Dec(r1), Inc(r2)) => r1 == r2,
        (MAdd(a1, v1), MSub(a2, v2)) | (MSub(a1, v1), MAdd(a2, v2)) => a1 == a2 && v1 == v2,
        (MXor(a1, v1), MXor(a2, v2)) => a1 == a2 && v1 == v2,
        _ => false,
    }
}

/// Вращение как знаковый сдвиг: rotl k -> +k, rotr k -> -k (по модулю 64).
fn rot_signed(i: Inst) -> Option<(u8, i32)> {
    use Inst::*;
    match i {
        RotL(r, k) => Some((r, (k & 63) as i32)),
        RotR(r, k) => Some((r, -((k & 63) as i32))),
        _ => None,
    }
}

fn rot_op(r: u8, k: i32) -> Inst {
    let k = k.rem_euclid(64) as u32;
    // короткая форма: k<=32 как rotl, иначе rotr
    if k == 0 {
        return Inst::RotL(r, 0); // тождество-заглушка (не достижимо из pass)
    }
    if k <= 32 {
        Inst::RotL(r, k)
    } else {
        Inst::RotR(r, 64 - k)
    }
}

/// Оптимизация последовательности: фиксированная точка (вращения + сокращения).
pub fn optimize(prog: &[Inst]) -> Vec<Inst> {
    let mut cur: Vec<Inst> = prog.to_vec();
    for _ in 0..8 {
        let next = pass(&cur);
        if next.len() == cur.len() {
            cur = next;
            break;
        }
        cur = next;
    }
    cur
}

fn pass(prog: &[Inst]) -> Vec<Inst> {
    // 1) слить соседние вращения одного регистра
    let mut a: Vec<Inst> = Vec::with_capacity(prog.len());
    for &i in prog {
        match rot_signed(i) {
            Some((r, k)) => {
                match a.last().copied().and_then(rot_signed) {
                    Some((r2, k2)) if r2 == r => {
                        // сливаем: предыдущее вращение r и текущее
                        a.pop();
                        let kk = (k + k2).rem_euclid(64);
                        if kk != 0 {
                            a.push(rot_op(r, kk));
                        }
                        // kk == 0 => тождество: ничего не кладём
                    }
                    _ => {
                        if k != 0 {
                            a.push(i);
                        }
                    }
                }
            }
            None => a.push(i),
        }
    }
    // 2) сокращение взаимно-обратных соседних пар (стек)
    let mut out: Vec<Inst> = Vec::with_capacity(a.len());
    for i in a {
        if let Some(&top) = out.last() {
            if cancel_pair(top, i) {
                out.pop();
                continue;
            }
        }
        out.push(i);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;

    fn eval(s: &mut State, p: &[Inst]) {
        for i in p {
            i.exec(s).unwrap();
        }
    }

    #[test]
    fn removes_canceling_pairs() {
        use Inst::*;
        let p = vec![Add(0, 1), Sub(0, 1), Xor(2, 3), Xor(2, 3), Add(0, 1)];
        let o = optimize(&p);
        assert_eq!(o.len(), 1);
        assert_eq!(o[0], Add(0, 1));
    }

    #[test]
    fn merges_rotations() {
        use Inst::*;
        // rotl r0 10 · rotl r0 20 == rotl r0 30 ; rotl 30 · rotr 30 == ε
        let p = vec![RotL(0, 10), RotL(0, 20), RotL(0, 30), RotR(0, 30)];
        let o = optimize(&p);
        assert_eq!(o, vec![RotL(0, 30)]);
    }

    #[test]
    fn soundness_on_random_states() {
        // opt(P) эквивалентно P на случайных состояниях, вперёд и обратно
        let mut seed = 0xACEu64;
        let mut next = move || {
            seed ^= seed >> 12;
            seed ^= seed << 25;
            seed ^= seed >> 27;
            seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        for _ in 0..100 {
            // случайная «шумная» программа с парами отмены
            let mut p: Vec<Inst> = Vec::new();
            for _ in 0..60 {
                let op = next() % 6;
                let (d, s) = (next() % 4, next() % 4);
                if d == s {
                    continue;
                }
                match op {
                    0 => p.push(Inst::Add(d as u8, s as u8)),
                    1 => p.push(Inst::Sub(d as u8, s as u8)),
                    2 => p.push(Inst::Xor(d as u8, s as u8)),
                    3 => p.push(Inst::Swap(d as u8, s as u8)),
                    4 => p.push(Inst::RotL(d as u8, (next() % 63) as u32)),
                    _ => p.push(Inst::RotR(d as u8, (next() % 63) as u32)),
                }
            }
            let o = optimize(&p);
            assert!(o.len() <= p.len());
            let mut s1 = State::random(4, 0, next());
            let orig = s1.clone();
            let mut s2 = orig.clone();
            eval(&mut s1, &p);
            eval(&mut s2, &o);
            if s1 != s2 {
                eprintln!("P  = {:?}", p);
                eprintln!("opt= {:?}", o);
                panic!("opt(P) != P (forward)");
            }
            // обратимость обеих
            for i in o.iter().rev() {
                i.inverse().unwrap().exec(&mut s2).unwrap();
            }
            assert_eq!(s2, orig, "opt(P) необратима");
        }
    }
}
