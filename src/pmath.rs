//! Математика в парадигме: обратимые целочисленные функции (биекции на Z/2^64).
//!
//! Идея «замены математических функций»: обычная математика (float) живёт на
//! границе фазы; ВНУТРИ парадигмы математика = набор БИЕКЦИЙ с аналитически
//! вычисляемой инверсией, которые можно композировать, применять и откатывать
//! (F⁻¹∘F = Id, свойство доказывается тестом), а результат — валидировать на
//! границе. Примеры биекций:
//!   MulOdd(m) — умножение на НЕЧЁТНУЮ константу mod 2^64 (биекция!),
//!               инверсия = умножение на обратное по модулю (Newton);
//!   Add(c)     — сдвиг (инверсия = Add(-c));
//!   Xor(k)     — самобратная;
//!   RotL(r)    — циклический сдвиг (инверсия RotR);
//!   ARX-раунд  — add/rot/xor (как PC1, но скалярная функция с инверсией).
//! Композиция любых таких функций = биекция → любая «формула» обратима.

/// Обратное к нечётному a по модулю 2^64 (Newton: 6 итераций удваивают биты).
pub fn mod_inv_odd(a: u64) -> u64 {
    debug_assert!(a & 1 == 1, "нужно нечётное a");
    let mut x = a; // x ≡ a^{-1} mod 2^1
    for _ in 0..6 {
        x = x.wrapping_mul(2u64.wrapping_sub(a.wrapping_mul(x)));
    }
    x
}

/// Базовые обратимые операции.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// x = x * m (m нечётное) — биекция; инверсия MulOdd(inv(m)).
    MulOdd(u64),
    /// x = x + c (wrap)
    Add(u64),
    /// x = x ^ k — самобратная
    Xor(u64),
    /// x = rotl(x, r)
    RotL(u32),
}

impl Op {
    pub fn apply(&self, x: u64) -> u64 {
        match *self {
            Op::MulOdd(m) => x.wrapping_mul(m),
            Op::Add(c) => x.wrapping_add(c),
            Op::Xor(k) => x ^ k,
            Op::RotL(r) => x.rotate_left(r & 63),
        }
    }
    /// Аналитическая инверсия.
    pub fn inverse(&self) -> Op {
        match *self {
            Op::MulOdd(m) => Op::MulOdd(mod_inv_odd(m)),
            Op::Add(c) => Op::Add(c.wrapping_neg()),
            Op::Xor(k) => Op::Xor(k),
            Op::RotL(r) => Op::RotL((64 - (r & 63)) & 63),
        }
    }
}

/// Применить последовательность операций (порядок слева направо).
pub fn apply(ops: &[Op], mut x: u64) -> u64 {
    for o in ops {
        x = o.apply(x);
    }
    x
}

/// Аналитическая инверсия последовательности (обратный порядок, инверсии).
pub fn inverse(ops: &[Op]) -> Vec<Op> {
    ops.iter().rev().map(|o| o.inverse()).collect()
}

/// Один ARX-раунд (вперёд/назад), биекция на паре слов:
///   forward:  s = a+b ; t = rotl(b,k) ^ s ; возвращаем (s,t)
///   inverse:  b = rotr(t^s, k) ; a = s-b
pub fn arx_round(a: u64, b: u64, k: u32) -> (u64, u64) {
    let s = a.wrapping_add(b);
    let t = b.rotate_left(k & 63) ^ s;
    (s, t)
}
pub fn arx_round_inv(s: u64, t: u64, k: u32) -> (u64, u64) {
    let b = (t ^ s).rotate_right(k & 63);
    let a = s.wrapping_sub(b);
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modular_inverse() {
        let mut rng = 0xACE1u64;
        let mut next = move || {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        for _ in 0..2000 {
            let a = next() | 1; // нечётное
            let inv = mod_inv_odd(a);
            assert_eq!(a.wrapping_mul(inv), 1, "a*inv != 1 mod 2^64");
        }
        // известные: inv(3) mod 2^64
        assert_eq!(3u64.wrapping_mul(mod_inv_odd(3)), 1);
    }

    #[test]
    fn op_roundtrip_random() {
        let ops = vec![
            Op::MulOdd(0x9E3779B97F4A7C15),
            Op::Add(0xD1B54A32D192ED03),
            Op::Xor(0xABCDEF0123456789),
            Op::RotL(17),
            Op::MulOdd(3),
            Op::RotL(33),
        ];
        let g = inverse(&ops);
        let mut rng = 0xFEEDu64;
        let mut next = move || {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        for _ in 0..5000 {
            let x = next();
            assert_eq!(apply(&g, apply(&ops, x)), x, "F⁻¹(F(x)) != x");
        }
    }

    #[test]
    fn arx_round_invertible() {
        let mut rng = 0xBEEF5u64;
        let mut next = move || {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        for _ in 0..3000 {
            let (a, b) = (next(), next());
            let k = (next() % 63) as u32 + 1;
            let (x, y) = arx_round(a, b, k);
            let (a2, b2) = arx_round_inv(x, y, k);
            assert_eq!((a2, b2), (a, b), "ARX-инверсия не точна");
        }
    }
}
