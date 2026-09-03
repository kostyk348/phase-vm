//! Полезное применение #3: фазовая арена — «аллокатор на обратимых идеях».
//!
//! Гипотеза (честная, measure-first): на phase-shaped нагрузках (тик /
//! транзакция / ECS) откат пачки аллокаций к метке стоит **O(1)**, тогда как
//! malloc+free платит O(K) вызовов free. Замеряем масштабирование по K.
//!
//! Сравнение: арена (alloc + rollback O(1)) против std malloc
//! (alloc + free × K). Плюс режим commit (объекты живут до reset).
//! Плюс проверка: rollback оставляет ноль следов — следующий alloc берёт
//! тот же offset (детерминированное переиспользование).

use std::alloc::{alloc, dealloc, Layout};
use std::time::Instant;

use phase_vm::alloc::Arena;

fn bench_malloc_free(rounds: u64, k: usize) -> f64 {
    let layout = Layout::from_size_align(64, 8).unwrap();
    let mut ptrs: Vec<*mut u8> = Vec::with_capacity(k);
    let t0 = Instant::now();
    for _ in 0..rounds {
        unsafe {
            for _ in 0..k {
                ptrs.push(alloc(layout));
            }
            for &p in ptrs.iter() {
                dealloc(p, layout);
            }
        }
        ptrs.clear();
    }
    let dt = t0.elapsed();
    dt.as_nanos() as f64 / (rounds as f64 * k as f64) // ns на (alloc+free)
}

fn bench_arena_rollback(rounds: u64, k: usize) -> f64 {
    let mut a = Arena::new();
    let t0 = Instant::now();
    for _ in 0..rounds {
        let m = a.push_mark();
        for _ in 0..k {
            let o = a.alloc(64).unwrap();
            a.write_u64(o, 1);
        }
        a.rollback(m); // O(1) — независимо от K
    }
    let dt = t0.elapsed();
    dt.as_nanos() as f64 / (rounds as f64 * k as f64) // ns на (alloc + доля rollback)
}

fn bench_arena_commit(rounds: u64, k: usize) -> f64 {
    // commit: объекты живут → раз в rounds/4 reset всей фазы (bulk)
    let mut a = Arena::new();
    let t0 = Instant::now();
    let mut since_reset = 0u64;
    for _ in 0..rounds {
        let _m = a.push_mark();
        for _ in 0..k {
            let o = a.alloc(64).unwrap();
            a.write_u64(o, 1);
        }
        a.commit();
        since_reset += 1;
        if since_reset >= rounds / 4 {
            a.reset(); // O(1) bulk-free границы фазы
            since_reset = 0;
        }
    }
    let dt = t0.elapsed();
    dt.as_nanos() as f64 / (rounds as f64 * k as f64)
}

fn main() {
    println!("phase-alloc: фазовая арена vs malloc/free (ns на alloc-операцию)");
    println!(
        "{:<10} {:>12} {:>14} {:>14} {:>10}",
        "K/фазу", "malloc+free", "arena+rollback", "arena+commit", "выигрыш×"
    );
    for k in [8usize, 64, 512, 4096, 32768] {
        let rounds = 2_000_000u64 / k as u64;
        let rounds = rounds.max(64);
        let mf = bench_malloc_free(rounds, k);
        let ar = bench_arena_rollback(rounds, k);
        let ac = bench_arena_commit(rounds, k);
        let gain = mf / ar;
        println!(
            "{:<10} {:>12.2} {:>14.2} {:>14.2} {:>9.1}×",
            k, mf, ar, ac, gain
        );
    }

    // Нулевой след: детерминированное переиспользование offset после rollback
    let mut a = Arena::new();
    let m = a.push_mark();
    let o1 = a.alloc(64).unwrap();
    a.write_u64(o1, 0xDEAD);
    a.rollback(m);
    let o2 = a.alloc(64).unwrap();
    assert_eq!(o1, o2, "rollback оставил след (offset не переиспользован)");
    println!(
        "нулевой след отката: OK (offset {:#x} переиспользован детерминированно)",
        o1
    );

    println!("итог: rollback-фазы O(1) при любом K; commit-фазы — bulk reset на границе.");
    println!(
        "честно: выигрыш ТОЛЬКО на phase-shaped нагрузках; произвольный free вне фазы — не обратим."
    );
}
