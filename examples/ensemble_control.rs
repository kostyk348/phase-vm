//! Прививка из DSO Control: **ensemble-стабильность** в обратимом тике.
//!
//! DSO Control (незакрытая нить: ensemble fitness был Python-only) дал урок:
//! оптимизация под номинальное растение хрупка — нужен worst-case по
//! распределению. Здесь E вариантов растения (разная инерция g_i) живут
//! ОДНОВРЕМЕННО как лейны в одном состоянии; ядро обратимо гонит все лейны
//! разом. Кандидат u принимается, только если ПОСЛЕ применения ВСЕ лейны в
//! жёстком инварианте |v|<=VMAX (скорость — физический предел). Любой
//! нарушитель → откат пачки reverse-ом. Копий на лейн — ноль.
//!
//! Прививка SINT: hash-chain границ фаз доказывает идентичность двух прогонов.

use std::time::Instant;

use phase_vm::machine::{reverse_all, run_forward};
use phase_vm::program::parse;
use phase_vm::state::State;

const E: usize = 6;
const VMAX: i64 = 55;
const CAND: [i64; 7] = [-3, -2, -1, 0, 1, 2, 3];
const GAINS: [i64; E] = [1, 2, 3, 4, 6, 8]; // инерция вариантов растения
const NTICKS: u64 = 200_000;
const DMAX: i64 = 45;

struct Xs(u64);
impl Xs {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

// q_i = r0..r(E-1); v_i = rE..r(2E-1); u_i = r(2E)..r(3E-1)
fn qreg(i: usize) -> usize {
    i
}
fn vreg(i: usize) -> usize {
    E + i
}
fn ureg(i: usize) -> usize {
    2 * E + i
}

fn make_kernel() -> Vec<phase_vm::Node> {
    let mut t = String::new();
    for i in 0..E {
        // v_i += u_i; q_i += v_i  (symplectic Euler, обратим)
        t.push_str(&format!("add r{} r{}\n", vreg(i), ureg(i)));
        t.push_str(&format!("add r{} r{}\n", qreg(i), vreg(i)));
    }
    parse(&t, 3 * E + 1).unwrap().nodes
}

type Stats = (u64, u64, u64, i64, u64, u64, u64); // commits, holds, lf, maxq, maxv, lr, nt
type Final = (u64, u64, u64); // chain, maxq, maxv

fn run(seed: u64) -> (Final, Stats) {
    let kernel = make_kernel();
    let mut rng = Xs(seed);
    let mut s = State::new(3 * E + 1, 0);
    for i in 0..E {
        s.regs[qreg(i)] = (300 + 500 * i as i64) as u64;
        s.regs[vreg(i)] = ((i as i64) * 3 - 7) as u64;
    }
    let (mut commits, mut holds) = (0u64, 0u64);
    let (mut lf, mut lr) = (0u64, 0u64);
    let mut chain = 0xcbf2_9ce4_8422_2325u64;
    let (mut maxq, mut maxv) = (0i64, 0i64);

    for tick in 0..NTICKS {
        // возмущение на случайный лейн (граница)
        if tick % 20 == 0 {
            let lane = (rng.next() % E as u64) as usize;
            let d = (rng.next() % (2 * DMAX as u64 + 1)) as i64 - DMAX;
            s.regs[qreg(lane)] = (s.regs[qreg(lane)] as i64).wrapping_add(d) as u64;
        }

        let mut best: Option<(usize, i128)> = None;
        for (k, &u) in CAND.iter().enumerate() {
            for (i, &g) in GAINS.iter().enumerate() {
                s.regs[ureg(i)] = (u * g) as u64; // per-lane вход (u·g_i)
            }
            run_forward(&mut s, &kernel).unwrap();
            lf += 2 * E as u64;

            // ensemble-инвариант: ВСЕ лейны в |v|<=VMAX после применения
            let mut ok = true;
            let mut cost: i128 = 0;
            for i in 0..E {
                let q = s.regs[qreg(i)] as i64;
                let v = s.regs[vreg(i)] as i64;
                if v.abs() > VMAX {
                    ok = false;
                }
                cost += (q as i128) * (q as i128) + (v as i128) * (v as i128);
            }
            if ok && best.is_none_or(|(_, bc)| cost < bc) {
                best = Some((k, cost));
            }
            reverse_all(&mut s, &kernel).unwrap();
            lr += 2 * E as u64;
        }
        match best {
            Some((k, _)) => {
                for (i, &g) in GAINS.iter().enumerate() {
                    s.regs[ureg(i)] = (CAND[k] * g) as u64;
                }
                run_forward(&mut s, &kernel).unwrap();
                lf += 2 * E as u64;
                commits += 1;
            }
            None => holds += 1,
        }
        for i in 0..E {
            let q = (s.regs[qreg(i)] as i64).abs();
            let v = (s.regs[vreg(i)] as i64).abs();
            maxq = maxq.max(q);
            maxv = maxv.max(v);
            // жёсткий worst-case инвариант держится на каждом тике, на каждом лейне
            assert!(v <= VMAX, "лейн {i}: |v|={v} > VMAX — инвариант нарушен");
        }
        chain = State::chain_step(chain, s.hash());
    }

    (
        (chain, maxq as u64, maxv as u64),
        (commits, holds, lf, maxq, maxv as u64, lr, NTICKS),
    )
}

fn main() {
    let t0 = Instant::now();
    let ((c1, maxq, maxv), (commits, holds, lf, _, _, lr, _)) = run(0xC0FFEE);
    let dt = t0.elapsed();
    let ((c2, _, _), _) = run(0xC0FFEE);
    assert_eq!(c1, c2, "hash-chain разошёлся — детерминизм нарушен");

    let leaves = lf + lr;
    let ns = dt.as_nanos() as f64 / leaves as f64;
    println!("ensemble_control: E={E} вариантов растения (инерции {GAINS:?}), тиков={NTICKS}");
    println!("тиков OK={commits}  hold={holds}");
    println!(
        "worst-case: макс |q|={maxq}, макс |v|={maxv} (лимит {VMAX}, держался на каждом лейне/тике)"
    );
    println!("leaves fwd={lf} rev={lr} ({ns:.2} ns/leaf, {:?})", dt);
    println!("hash-chain границ: 0x{c1:016x} — идентичен при повторе (SINT-аудит)");
    println!("копий: 0 на лейн/кандидата (классика: E копий состояния на кандидата)");
}
