//! Типовая задача: A/B-выбор конфигурации с откатом проигравшего.
//!
//! Классика: копия состояния на каждого кандидата. Парадигма: кандидат
//! ПРИМЕНЯЕТСЯ к базе (обратимые madd-сдвиги 4 полей), метрика считается,
//! проигравший откатывается обратным прогоном — 0 копий. Лучший коммитится.

use std::time::Instant;

use phase_vm::machine::{reverse_all, run_forward};
use phase_vm::program::parse;
use phase_vm::state::State;

const ROUNDS: u64 = 500_000;
// два «конфига»: аддитивные правки 4 полей
const DELTA_A: [i64; 4] = [3, -1, 2, 0];
const DELTA_B: [i64; 4] = [-2, 1, 0, 3];

fn main() {
    // 4 поля в памяти mem[0..4]; индексы полей — регистры r0..r3 (0..3),
    // значения дельт — r4..r7. Ядро: mem[r_j] += r_{4+j}  (обратимо: msub)
    let kernel = parse("madd r0 r4\nmadd r1 r5\nmadd r2 r6\nmadd r3 r7\n", 12)
        .unwrap()
        .nodes;
    let mut s = State::new(12, 8);
    for j in 0..4 {
        s.regs[j] = j as u64; // адреса полей
        s.mem[j] = 500 + 100 * j as u64;
    }
    let metric = |s: &State| -> i128 {
        // качество: близость к целевому профилю 1500/1400/1300/1200
        (0..4)
            .map(|j| {
                let t = 1500 - 100 * j as i128;
                let v = s.mem[j] as i128;
                (v - t).pow(2)
            })
            .sum()
    };
    let mut best_metric = i128::MAX;

    let (mut win_a, mut win_b) = (0u64, 0u64);
    let t0 = Instant::now();
    for _ in 0..ROUNDS {
        // кандидат A: применяем к базе
        for (j, &d) in DELTA_A.iter().enumerate() {
            s.regs[4 + j] = d as u64;
        }
        run_forward(&mut s, &kernel).unwrap();
        let ma = metric(&s);
        // кандидат B: откат A + применение B
        reverse_all(&mut s, &kernel).unwrap();
        for (j, &d) in DELTA_B.iter().enumerate() {
            s.regs[4 + j] = d as u64;
        }
        run_forward(&mut s, &kernel).unwrap();
        let mb = metric(&s);

        if ma <= mb {
            // A лучше: B откатываем и применяем A
            reverse_all(&mut s, &kernel).unwrap();
            for (j, &d) in DELTA_A.iter().enumerate() {
                s.regs[4 + j] = d as u64;
            }
            run_forward(&mut s, &kernel).unwrap();
            win_a += 1;
        } else {
            win_b += 1; // B коммитится (уже применён)
        }
        best_metric = best_metric.min(metric(&s)); // лучший достигнутый профиль
    }
    let dt = t0.elapsed();
    // листьев: A-apply(4)+A-rev(4)+B-apply(4)+ [rev(4)+apply(4) если A] ≈ 12-16 на раунд
    let leaves_est = ROUNDS * 12;
    let ns = dt.as_nanos() as f64 / leaves_est as f64;
    let clone_bytes = ROUNDS as u128 * 2 * (4 * 8);

    println!("typical A/B: ROUNDS={ROUNDS}, 2 кандидата, выбор по метрике");
    println!("побед: A={win_a} B={win_b} | {:?} ({ns:.2} ns/leaf)", dt);
    println!(
        "копии состояния: 0 (классика: {} B клонов на раунды)",
        clone_bytes
    );
    println!("откат проигравшего = обратный прогон 4 листьев, не копия полей");
    println!("лучшая достигнутая метрика: {best_metric}");
}
