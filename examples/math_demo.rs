//! Математика в парадигме: «замена математических функций».
//!
//! Внутри фазы математика = биекции на Z/2^64 с АНАЛИТИЧЕСКИ вычисляемой
//! инверсией. Примеры: аффинный кодер y = a·x + b (a нечётное) и 12-раундовый
//! ARX-скремблер — обе функции имеют точную обратную, композиция тоже
//! биекция. Float-математика остаётся на границе (она необратима).

use phase_vm::pmath::{self, Op};
use std::time::Instant;

fn main() {
    // 1) аффинная биекция: y = a*x + b, инверсия аналитическая
    let a: u64 = 0x9E3779B97F4A7C15; // нечётное
    let b: u64 = 0xD1B54A32D192ED03;
    let enc = vec![Op::MulOdd(a), Op::Add(b)];
    let dec = pmath::inverse(&enc);
    let x: u64 = 0x1234_5678_9ABC_DEF0;
    let y = pmath::apply(&enc, x);
    assert_eq!(pmath::apply(&dec, y), x, "аффинная инверсия не точна");

    // 2) 12-раундовый ARX-скремблер как биекция пары (тоже с инверсией)
    let mut rng = 0x51DEu64;
    let mut next = move || {
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let ks: Vec<u32> = (0..12).map(|_| (next() % 63) as u32 + 1).collect();
    let mut pair = (next(), next());
    let orig = pair;
    let t0 = Instant::now();
    for &k in &ks {
        pair = pmath::arx_round(pair.0, pair.1, k);
    }
    let fwd = t0.elapsed();
    for &k in ks.iter().rev() {
        pair = pmath::arx_round_inv(pair.0, pair.1, k);
    }
    assert_eq!(pair, orig, "ARX-скремблер не вернул исходную пару");

    // 3) замер скорости инверсии модульного умножения (Newton, 6 итераций)
    let t1 = Instant::now();
    let mut acc = 0u64;
    for i in 0..100_000u64 {
        acc ^= pmath::mod_inv_odd((i * 2 + 1) | 1);
    }
    let inv_time = t1.elapsed();
    let _ = acc;

    println!("phase-math: математика как обратимые биекции (Z/2^64)");
    println!("аффинный кодер y=a·x+b (a нечётное): инверсия аналитическая, F⁻¹(F(x))==x (assert)");
    println!(
        "ARX-скремблер 12 раундов: forward {:?}, полный roundtrip == оригинал (assert)",
        fwd
    );
    println!(
        "mod_inv_odd (Newton): 100k инверсий за {:?} ({:.0} ns/инверсия)",
        inv_time,
        inv_time.as_nanos() as f64 / 100_000.0
    );
    println!("float-математика: остаётся НА ГРАНИЦЕ (необратима) — внутри фазы только биекции");
}
