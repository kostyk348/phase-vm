//! Полезное применение: транзакционная обработка с бесплатным откатом.
//!
//! Паттерн (задел под #11 «СУБД без WAL» и спекулятивные DSO-тики):
//!   машина остаётся ЧИСТО обратимой, решение commit/abort принимает хост
//!   по инвариантам ПОСЛЕ применения, abort = обратный прогон пачки →
//!   0 байт логов, 0 копий состояния.
//!
//! Сценарий: N счетов в памяти, пачки по B переводов. Сумма каждого перевода
//! — 60..95% баланса отправителя НА МОМЕНТ ПОСТРОЕНИЯ пачки. Внутри пачки
//! один счёт может быть отправителем дважды → каскадный овердрафт, который
//! НЕВОЗМОЖНО увидеть до применения (нужна была бы симуляция всей пачки).
//! После применения хост проверяет: нет ли wrap-ухода в минус. Если есть —
//! откатывается ВСЯ пачка обратным прогоном (2*B листьев).
//!
//! Классика копировала бы стол (N*8 байт) перед каждой пачкой. Здесь — 0.

use std::time::Instant;

use phase_vm::machine::{reverse_all, run_forward};
use phase_vm::program::parse;
use phase_vm::state::State;

const N: usize = 256; // счетов (слов памяти)
const B: usize = 16; // переводов в пачке
const NBATCH: u64 = 20_000; // пачек
const MAXBAL: u64 = 1 << 40; // порог: выше = wrap-уход в минус

struct Xs(u64);
impl Xs {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

fn main() {
    // Программа пачки: для каждого перевода k свои регистры
    // addrF=r(3k) addrT=r(3k+1) amt=r(3k+2):
    //   msub addrF amt; madd addrT amt
    let mut prog_text = String::new();
    for k in 0..B {
        let (af, at, am) = (3 * k, 3 * k + 1, 3 * k + 2);
        prog_text.push_str(&format!("msub r{af} r{am}\nmadd r{at} r{am}\n"));
    }
    let kernel = parse(&prog_text, 3 * B + 3).unwrap().nodes;
    let leaves_per_batch = 2 * B as u64;

    let mut rng = Xs(0x5EED_1234);
    let mut s = State::new(3 * B + 3, N);
    for b in s.mem.iter_mut() {
        *b = 100 + rng.next() % 5000;
    }
    let mut expected = s.mem.clone(); // эталонная симуляция коммитов

    let total_start: u128 = s.mem.iter().map(|&v| v as u128).sum();
    let mut batches_ok = 0u64;
    let mut batches_rev = 0u64;
    let mut ops_committed = 0u64;
    let mut leaves_fwd = 0u64;
    let mut leaves_rev = 0u64;

    let t0 = Instant::now();
    for _ in 0..NBATCH {
        // --- построение пачки: суммы = 60..95% баланса НА СЕЙЧАС ---
        // (без применения: каскад внутри пачки не виден)
        let mut specs = Vec::with_capacity(B);
        let mut base = [0u64; N]; // балансы на момент построения
        base.copy_from_slice(&s.mem[..N]);
        for k in 0..B {
            let (af, at, am) = (3 * k, 3 * k + 1, 3 * k + 2);
            let from = (rng.next() % N as u64) as usize;
            let to = loop {
                let t = (rng.next() % N as u64) as usize;
                if t != from {
                    break t;
                }
            };
            let pct = 60 + rng.next() % 36; // 60..95 %
            let amt = (base[from] * pct) / 100;
            s.regs[af] = from as u64;
            s.regs[at] = to as u64;
            s.regs[am] = amt;
            specs.push((from, to, amt));
        }

        // --- применение пачки (чисто обратимое) ---
        run_forward(&mut s, &kernel).unwrap();
        leaves_fwd += leaves_per_batch;

        // --- инвариант ПОСЛЕ применения: ни один счёт не ушёл в минус ---
        let ok = specs.iter().all(|&(from, _, _)| s.mem[from] <= MAXBAL);
        if ok {
            for &(from, to, amt) in &specs {
                expected[from] -= amt;
                expected[to] += amt;
            }
            ops_committed += B as u64;
            batches_ok += 1;
        } else {
            // ABORT пачки: обратный прогон 2*B листьев
            reverse_all(&mut s, &kernel).unwrap();
            leaves_rev += leaves_per_batch;
            batches_rev += 1;
        }
        debug_assert_eq!(s.mem.iter().map(|&v| v as u128).sum::<u128>(), total_start);
    }
    let dt = t0.elapsed();

    assert_eq!(s.mem, expected, "состояние разошлось с эталоном");
    assert_eq!(s.mem.iter().map(|&v| v as u128).sum::<u128>(), total_start);

    let leaves_total = leaves_fwd + leaves_rev;
    let ns_leaf = dt.as_nanos() as f64 / leaves_total as f64;
    let rev_rate = batches_rev as f64 / NBATCH as f64;
    let snapshot_bytes = NBATCH as u128 * (N * 8) as u128;

    println!("batch_tx: N={N} счетов, пачек={NBATCH} по {B} переводов");
    println!(
        "пачек OK={batches_ok}  откачено={batches_rev} ({:.1}%)",
        rev_rate * 100.0
    );
    println!("переводов закоммичено: {ops_committed}");
    println!(
        "leaves: fwd={leaves_fwd}  rev(abort пачек)={leaves_rev}  ({:.2} ns/leaf)",
        ns_leaf
    );
    println!(
        "откат пачки: 0 байт логов / 0 копий состояния (каскадные овердрафты ловятся после применения)"
    );
    println!(
        "snapshot-подход скопировал бы {} ({} MiB) — здесь 0",
        snapshot_bytes,
        snapshot_bytes / (1024 * 1024)
    );
    println!("OK: состояние == эталонная симуляция коммитов; сумма консервативна");
}
