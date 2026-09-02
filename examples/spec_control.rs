//! Полезное применение #2: спекулятивный safety-filtered тик-контроллер.
//!
//! DSO-флейвор: детерминированный тиковый цикл, где ТИК = ФАЗА.
//! За каждый тик хост перебирает K кандидатов управления; каждый кандидат
//! ПРИМЕНЯЕТСЯ к состоянию (forward целочисленного симплектического Эйлера:
//! v += u; q += v — это обратимый symplectic-шаг на целых), затем проверяется
//! на инварианты (|q|<=QMAX, |v|<=VMAX). Нарушитель → обратный прогон (0 байт
//! логов/копий). Лучший по стоимости кандидат коммитится.
//!
//! Ключевое отличие от классики (MPC/снапшоты): состояние НЕ копируется между
//! кандидатами — откат дешевле копии, потому что стоит O(шагов), не O(байт).
//! Внешнее возмущение — граница (вход извне, не откатывается).
//!
//! Детерминизм: тот же seed → та же траектория (проверяется в конце).

use std::time::Instant;

use phase_vm::machine::{reverse_all, run_forward};
use phase_vm::program::parse;
use phase_vm::state::State;

const QMAX: i64 = 3000;
const VMAX: i64 = 60;
const WV: i64 = 3; // вес скорости в стоимости
const CAND: [i64; 7] = [-3, -2, -1, 0, 1, 2, 3];
const NTICKS: u64 = 300_000;
const DMAX: i64 = 40; // амплитуда внешних возмущений
const DIST_EVERY: u64 = 40; // каждые N тиков — возмущение

struct Xs(u64);
impl Xs {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// Один прогон симуляции. Возвращает (финальное состояние, статистика).
type Final = [i64; 2];
type Stats = (u64, u64, u64, u64, u64, i64, i64);

fn run(seed: u64) -> (Final, Stats) {
    // Ядро одного тика: v += u (r2); q += v. Целочисленный symplectic Euler.
    // Обратимо: reverse = q -= v; v -= u. Источники целы.
    let kernel = parse("add r1 r2\nadd r0 r1\n", 8).unwrap().nodes;
    let mut rng = Xs(seed);
    let mut s = State::new(8, 0);
    // старт: q=0, v=0, лёгкий толчок, чтобы было кого стабилизировать
    s.regs[0] = 1000u64; // q0
    s.regs[1] = 10u64; // v0

    let (mut commits, mut holds, mut dists) = (0u64, 0u64, 0u64);
    let (mut leaves_fwd, mut leaves_rev) = (0u64, 0u64);
    let (mut max_q, mut max_v) = (0i64, 0i64);

    for tick in 0..NTICKS {
        // --- внешнее возмущение: граница (вход), не откатывается ---
        if tick % DIST_EVERY == 0 {
            let d = (rng.next() % (2 * DMAX as u64 + 1)) as i64 - DMAX;
            s.regs[0] = (s.regs[0] as i64).wrapping_add(d) as u64;
            dists += 1;
        }

        let q0 = s.regs[0];
        let v0 = s.regs[1];

        // --- спекуляция: пробуем каждый кандидат, отвергнутые откатываем ---
        let mut best: Option<(usize, i128)> = None;
        for (k, &u) in CAND.iter().enumerate() {
            s.regs[2] = u as u64;
            run_forward(&mut s, &kernel).unwrap();
            leaves_fwd += 2;

            let q = s.regs[0] as i64;
            let v = s.regs[1] as i64;
            if q.abs() <= QMAX && v.abs() <= VMAX {
                let cost = (q as i128) * (q as i128) + (WV as i128) * (v as i128) * (v as i128);
                if best.is_none_or(|(_, bc)| cost < bc) {
                    best = Some((k, cost));
                }
            }
            // возврат к базе тика: обратный прогон 2 листьев
            reverse_all(&mut s, &kernel).unwrap();
            leaves_rev += 2;
            // доказательство нулевого следа отката
            assert_eq!((s.regs[0], s.regs[1]), (q0, v0), "откат оставил след");
        }

        // --- коммит лучшего валидного кандидата (или hold) ---
        match best {
            Some((k, _)) => {
                s.regs[2] = CAND[k] as u64;
                run_forward(&mut s, &kernel).unwrap();
                leaves_fwd += 2;
                commits += 1;
            }
            None => holds += 1,
        }

        let q = s.regs[0] as i64;
        let v = s.regs[1] as i64;
        max_q = max_q.max(q.abs());
        max_v = max_v.max(v.abs());
        // инвариант безопасности держится на каждом тике
        assert!(q.abs() <= QMAX && v.abs() <= VMAX, "нарушение границ");
    }

    (
        [s.regs[0] as i64, s.regs[1] as i64],
        (commits, holds, dists, leaves_fwd, leaves_rev, max_q, max_v),
    )
}

fn main() {
    let t0 = Instant::now();
    let (final1, stats1) = run(0xABCD_1234);
    let dt = t0.elapsed();
    // детерминизм: тот же seed → та же траектория
    let (final2, _) = run(0xABCD_1234);
    assert_eq!(final1, final2, "детерминизм нарушен");

    let (commits, holds, dists, lf, lr, max_q, max_v) = stats1;
    let leaves = lf + lr;
    let ns_leaf = dt.as_nanos() as f64 / leaves as f64;
    let cand_per_tick = CAND.len();
    // классика (MPC-стиль): копия состояния на кандидата перед каждым пробным
    // применением = NTICKS * cand * sizeof(state). Состояние: q,v = 16 байт.
    let snapshot_bytes = NTICKS as u128 * cand_per_tick as u128 * 16;

    println!(
        "spec_control: NTICKS={NTICKS}, кандидатов/тик={cand_per_tick}, symplectic Euler (целые)"
    );
    println!("тиков закоммичено={commits}  hold (нет валидных)={holds}  возмущений={dists}");
    println!("|q|max={max_q} (лимит {QMAX})  |v|max={max_v} (лимит {VMAX}) — инвариант держался всё время");
    println!(
        "leaves: fwd={lf}  rev(отклонённые кандидаты)={lr}  ({ns_leaf:.2} ns/leaf, {:?})",
        dt
    );
    println!("финал: q={}, v={}", final1[0], final1[1]);
    println!(
        "MPC-снапшоты скопировали бы {} ({} MiB) — здесь 0 байт логов/копий",
        snapshot_bytes,
        snapshot_bytes / (1024 * 1024)
    );
    println!("детерминизм: OK (одинаковый seed → одинаковый финал)");
}
