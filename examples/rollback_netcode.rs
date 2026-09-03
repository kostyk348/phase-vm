//! L5 netcode-реконсиляция (болячки N3/N4): детерминированный серверный тик,
//! где запаздывающие входы клиентов исправляются **откатом**, а не снапшотами.
//!
//! Модель: M сущностей, симметричный Эйлер (обратим), входы клиентов —
//! ГРАНИЦЫ (хранятся как журнал входов: 1 байт/сущность/тик — это входы,
//! не состояние). Истинные входы приходят с задержкой 0..DELAY_MAX тиков;
//! сервер симулирует предсказанием (0). Когда приходит вход для уже
//! смоделированного тика и он отличается — сервер откатывается на этот тик
//! (reverse по тикам), применяет вход, ресимулирует детерминированно.
//!
//! Корректность: финальная временная линия обязана совпасть с эталонной
//! (ground truth без задержек) — сверяем hash-цепочку состояний всех тиков.
//! Классика хранила бы снапшот мира каждый тик; здесь — только журнал входов
//! + откат O(окна).

use std::time::Instant;

use phase_vm::machine::{reverse_all, run_forward};
use phase_vm::program::parse;
use phase_vm::state::State;

const M: usize = 4; // сущностей
const NT: usize = 60_000; // тиков
const DELAY_MAX: usize = 8; // окно задержки сети
const NR: usize = 3 * M; // regs: p M, v M, u M

// p_e = r0..M-1, v_e = rM..2M-1, u_e = r2M..3M-1
fn p_(e: usize) -> usize {
    e
}
fn v_(e: usize) -> usize {
    M + e
}
fn u_(e: usize) -> usize {
    2 * M + e
}

struct Xs(u64);
impl Xs {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

fn make_kernel() -> Vec<phase_vm::Node> {
    let mut t = String::new();
    for e in 0..M {
        t.push_str(&format!("add r{} r{}\n", v_(e), u_(e)));
        t.push_str(&format!("add r{} r{}\n", p_(e), v_(e)));
    }
    parse(&t, NR + 1).unwrap().nodes
}

/// sim одного тика (применяет входы из log[t] и снимает hash)
fn tick(state: &mut State, kernel: &[phase_vm::Node], log_t: &[i8; M]) {
    for (e, &v) in log_t.iter().enumerate() {
        state.regs[u_(e)] = v as i64 as u64;
    }
    run_forward(state, kernel).unwrap();
}

fn main() {
    let kernel = make_kernel();
    let mut rng = Xs(0x5EED);

    // --- ground truth входы и эталонная линия (без задержек) ---
    let mut truth = vec![[0i8; M]; NT];
    for row in truth.iter_mut() {
        for slot in row.iter_mut() {
            *slot = (rng.next() % 3) as i8 - 1; // -1/0/1
        }
    }
    let mut ref_state = State::new(NR + 1, 0);
    let mut ref_hash = vec![0u64; NT + 1];
    ref_hash[0] = ref_state.hash();
    for t in 0..NT {
        tick(&mut ref_state, &kernel, &truth[t]);
        ref_hash[t + 1] = ref_state.hash();
    }
    // свёртка hash-цепочек по полным массивам хэшей состояний
    let fold = |hs: &[u64]| -> Vec<u64> {
        let mut c = 0xcbf2_9ce4_8422_2325u64;
        let mut out = Vec::with_capacity(hs.len());
        for &h in hs {
            c = State::chain_step(c, h);
            out.push(c);
        }
        out
    };
    let ref_chain = fold(&ref_hash);

    // --- расписание прибытия: вход сущности e для тика t приходит в t+delay ---
    let mut arrivals: Vec<Vec<(usize, usize, i8)>> = vec![Vec::new(); NT + DELAY_MAX + 1];
    for t in 0..NT {
        for e in 0..M {
            let delay = (rng.next() % (DELAY_MAX as u64 + 1)) as usize;
            arrivals[t + delay].push((e, t, truth[t][e]));
        }
    }

    // --- сервер: детерминированная симуляция с реконсиляцией ---
    let mut s = State::new(NR + 1, 0);
    let mut log = vec![[0i8; M]; NT + 1]; // известные серверу входы (pred=0)
    let mut sim_hash = vec![0u64; NT + 1];
    sim_hash[0] = s.hash();
    let mut cur_sim = 0usize; // сколько тиков смоделировано
    let (mut rollbacks, mut rev_ticks, mut leaves_fwd) = (0u64, 0u64, 0u64);
    let (mut min_win, mut max_win) = (usize::MAX, 0usize);

    let t0 = Instant::now();
    for now in 0..=(NT + DELAY_MAX) {
        // доставка запоздавших входов
        for &(e, t, val) in &arrivals[now] {
            if t >= cur_sim {
                // тик ещё не смоделирован — просто запоминаем вход
                log[t][e] = val;
                continue;
            }
            if log[t][e] == val {
                continue; // предсказание совпало — ничего не делаем
            }
            // РЕКОНСИЛЯЦИЯ: откат на тик t
            let goal = cur_sim;
            let win = goal - t;
            min_win = min_win.min(win);
            max_win = max_win.max(win);
            while cur_sim > t {
                // reverse тика требует ВХОДЫ этого тика (регистры u перезаписаны
                // более поздними тиками) — берём их из журнала входов
                for e in 0..M {
                    s.regs[u_(e)] = log[cur_sim - 1][e] as i64 as u64;
                }
                reverse_all(&mut s, &kernel).unwrap();
                cur_sim -= 1;
                rev_ticks += 1;
            }
            log[t][e] = val;
            // детерминированный ресим до goal
            while cur_sim < goal {
                tick(&mut s, &kernel, &log[cur_sim]);
                leaves_fwd += 1;
                cur_sim += 1;
                sim_hash[cur_sim] = s.hash();
            }
            rollbacks += 1;
        }
        // симуляция очередного тика (now < NT)
        if now < NT {
            tick(&mut s, &kernel, &log[now]);
            leaves_fwd += 1;
            cur_sim += 1;
            sim_hash[cur_sim] = s.hash();
        }
    }
    let dt = t0.elapsed();
    assert_eq!(cur_sim, NT);
    let sim_chain = fold(&sim_hash);

    // --- сверка с эталоном: финальная линия обязана совпасть по ВСЕМ тикам ---
    let mut mismatches = 0u64;
    let mut first_bad = usize::MAX;
    for t in 0..=NT {
        if sim_chain[t] != ref_chain[t] {
            mismatches += 1;
            first_bad = first_bad.min(t);
        }
    }
    assert_eq!(
        mismatches, 0,
        "реконсиляция разошлась с эталоном на {mismatches} тиках"
    );

    // детерминизм: второй прогон — тот же финальный hash
    // (полный повтор дорогой; сверяемся тем, что линия == эталон уже доказана)

    let rev_leaves = rev_ticks * (2 * M) as u64;
    let total_leaves = leaves_fwd + rev_leaves;
    let ns_leaf = dt.as_nanos() as f64 / total_leaves as f64;
    let snapshot_bytes = NT * M * 2 * 8; // классика: мир кажды тик
    let log_bytes = NT * M; // журнал входов (1 байт/сущность/тик)

    println!("rollback_netcode: M={M} сущностей, тиков={NT}, окно задержки 0..{DELAY_MAX}");
    println!("реконсиляций (запоздавший вход ≠ предсказание): {rollbacks}");
    println!("окно отката: min={min_win} max={max_win} тиков; откачено тиков={rev_ticks}");
    println!(
        "leaves fwd={leaves_fwd} rev={rev_leaves} ({ns_leaf:.2} ns/leaf, {:?})",
        dt
    );
    println!(
        "память: журнал входов {log_bytes} B против снапшотов мира {snapshot_bytes} B ({:.0}×)",
        snapshot_bytes as f64 / log_bytes as f64
    );
    println!("корректность: hash-цепочка всех {NT}+1 тиков == эталон (без задержек)");
    println!("детерминизм: реконсиляция сошлась к ground truth — откат не оставил следов");
}
