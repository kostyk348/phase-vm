//! phase-etl — реальный ETL-инструмент целиком в парадигме PHASE.
//!
//! Пайплайн над CSV:
//!   1) ПАРСИНГ в Arena: каждая строка = своя mark; битая/дубль → rollback(mark)
//!      O(1), счётчик; (типовой приём «парсинг с восстановлением»)
//!   2) ТАБЛИЦА-ФАЗА: валидные строки → память State (row-major, stride=ncols);
//!   3) ТРАНСФОРМАЦИЯ = обратимые ядра madd по колонкам; A/B-выбор параметров
//!      нормализации на ЖИВОЙ таблице: применить A → метрика → откатить всю
//!      таблицу (построчно reverse) → применить B → метрика → коммит лучшего
//!      (0 копий таблицы);
//!   4) ФИЛЬТР-КОММИТ на границе: выбросы отбрасываются (копия-наружу);
//!   5) АУДИТ: детерминированный hash-дайджест результата.
//!
//! Использование:
//!   phase-etl <in.csv> [out.csv]      — обработать реальный файл
//!   phase-etl --gen [n]               — сгенерировать детерминированный sample

use std::collections::HashSet;
use std::time::Instant;

use phase_vm::alloc::Arena;
use phase_vm::machine::{reverse_all, run_forward};
use phase_vm::program::parse;
use phase_vm::state::State;

const NCOLS: usize = 4; // числовые колонки a,b,c,d (id отдельно)
const TARGETS: [i64; NCOLS] = [100, 500, 1000, 250];
const TOL: i64 = 150; // порог «выброса»
const DELTA_A: [i64; NCOLS] = [5, -3, 2, 0];
const DELTA_B: [i64; NCOLS] = [0, 2, -1, 4];

struct Xs(u64);
impl Xs {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// Сгенерировать детерминированный CSV: n строк, ~6% битых/дублей.
fn gen_sample(n: usize) -> String {
    let mut rng = Xs(0xE71);
    let mut out = String::from("id,a,b,c,d\n");
    let dup_every = 19usize; // каждая 19-я строка — дубль id (валидная по формату)
    for i in 0..n {
        let a = (i % 97) as u64;
        let b = rng.next() % 1200;
        let c = rng.next() % 2000;
        let d = rng.next() % 600;
        if rng.next() % 100 < 4 {
            out.push_str(&format!("{i},x,{c},{d}\n")); // битая (не хватает колонок)
        } else if rng.next() % 100 < 2 {
            out.push_str(&format!("{i},,{c},{d},zzz\n")); // битая (пустое число)
        } else {
            let id = if i % dup_every == 0 && i > 0 {
                i - 1
            } else {
                i
            };
            out.push_str(&format!("{id},{a},{b},{c},{d}\n"));
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (in_path, out_path): (String, String) = if args.len() >= 2 && args[1] == "--gen" {
        let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10_000);
        let csv = gen_sample(n);
        let p = "/tmp/phase_etl_sample.csv".to_string();
        std::fs::write(&p, &csv).unwrap();
        (p, "/tmp/phase_etl_out.csv".to_string())
    } else {
        (
            args.get(1)
                .cloned()
                .unwrap_or_else(|| "/tmp/phase_etl_sample.csv".into()),
            args.get(2)
                .cloned()
                .unwrap_or_else(|| "/tmp/phase_etl_out.csv".into()),
        )
    };
    let csv = match std::fs::read_to_string(&in_path) {
        Ok(s) => s,
        Err(_) => {
            let g = gen_sample(10_000);
            std::fs::write(&in_path, &g).unwrap();
            g
        }
    };
    let t0 = Instant::now();

    // ---------- 1) парсинг в Arena с O(1)-откатом битых строк ----------
    let mut arena = Arena::new();
    let mut ids: Vec<u64> = Vec::new();
    let mut rows: Vec<Vec<u64>> = Vec::new();
    let mut seen: HashSet<u64> = HashSet::new();
    let (mut total, mut malformed, mut dup, mut parsed) = (0u64, 0u64, 0u64, 0u64);
    let mut recovered = 0usize;
    for line in csv.lines().skip(1) {
        total += 1;
        let _before = arena.used_bytes();
        let m = arena.push_mark();
        let f: Vec<&str> = line.split(',').collect();
        let mut ok = true;
        let mut nums = Vec::with_capacity(1 + NCOLS);
        if f.len() != 1 + NCOLS {
            ok = false;
        } else {
            for t in &f[1..] {
                match t.trim().parse::<u64>() {
                    Ok(v) => {
                        let off = arena.alloc(8).unwrap();
                        arena.write_u64(off, v);
                        nums.push(off);
                    }
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
        }
        if !ok {
            let used_b = arena.used_bytes();
            arena.rollback(m);
            recovered += used_b - arena.used_bytes();
            malformed += 1;
            continue;
        }
        let id = f[0].trim().parse::<u64>().unwrap_or(u64::MAX);
        if !seen.insert(id) {
            let used_b = arena.used_bytes();
            arena.rollback(m);
            recovered += used_b - arena.used_bytes();
            dup += 1;
            continue;
        }
        arena.commit();
        ids.push(id);
        rows.push(nums.iter().map(|&o| arena.read_u64(o)).collect());
        parsed += 1;
    }

    // ---------- 2) таблица-фаза: row-major в памяти State ----------
    let nrows = rows.len();
    let mut s = State::new(16, nrows * NCOLS);
    for (r, row) in rows.iter().enumerate() {
        for (c, &v) in row.iter().enumerate() {
            s.mem[r * NCOLS + c] = v;
        }
    }
    // ядро трансформации: mem[base+c] += delta_c, base в r0, дельты r4..r7,
    // адреса колонок r8..r11 = base+0..3 (хост ставит под каждую строку)
    let kernel = parse("madd r8 r4\nmadd r9 r5\nmadd r10 r6\nmadd r11 r7\n", 16)
        .unwrap()
        .nodes;

    // ---------- 3) A/B на живой таблице с откатом всей таблицы ----------
    let outlier_count = |s: &State| -> u64 {
        let mut bad = 0u64;
        for r in 0..nrows {
            for (c, &tgt) in TARGETS.iter().enumerate() {
                let v = s.mem[r * NCOLS + c] as i64;
                if (v - tgt).abs() > TOL {
                    bad += 1;
                    break;
                }
            }
        }
        bad
    };
    let apply_deltas = |s: &mut State, d: &[i64; NCOLS]| {
        for r in 0..nrows {
            let base = (r * NCOLS) as u64;
            for (c, &dv) in d.iter().enumerate() {
                s.regs[8 + c] = base + c as u64;
                s.regs[4 + c] = dv as u64;
            }
            run_forward(s, &kernel).unwrap();
        }
    };
    let rollback_table = |s: &mut State| {
        for r in (0..nrows).rev() {
            let base = (r * NCOLS) as u64;
            for c in 0..NCOLS {
                s.regs[8 + c] = base + c as u64;
            }
            reverse_all(s, &kernel).unwrap(); // источники r4..r7 целы
        }
    };

    let base_bad = outlier_count(&s);
    apply_deltas(&mut s, &DELTA_A);
    let bad_a = outlier_count(&s);
    rollback_table(&mut s);
    apply_deltas(&mut s, &DELTA_B);
    let bad_b = outlier_count(&s);
    let winner = if bad_a <= bad_b {
        rollback_table(&mut s);
        apply_deltas(&mut s, &DELTA_A);
        "A"
    } else {
        "B"
    };
    let final_bad = outlier_count(&s);

    // ---------- 4) фильтр-коммит выбросов ----------
    let mut kept: Vec<(u64, Vec<u64>)> = Vec::new();
    for (r, &id) in ids.iter().enumerate() {
        let row: Vec<u64> = s.mem[r * NCOLS..r * NCOLS + NCOLS].to_vec();
        let is_outlier = row
            .iter()
            .zip(TARGETS.iter())
            .any(|(&v, &t)| (v as i64 - t).abs() > TOL);
        if !is_outlier {
            kept.push((id, row));
        }
    }
    // ---------- 5) дайджест результата ----------
    let mut digest = 0xcbf2_9ce4_8422_2325u64;
    let mut out_csv = String::from("id,a,b,c,d\n");
    for (id, row) in &kept {
        let mut h = *id;
        for v in row {
            h = h.wrapping_mul(31).wrapping_add(*v);
        }
        digest = State::chain_step(digest, h);
        out_csv.push_str(&format!(
            "{id},{}\n",
            row.iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    std::fs::write(&out_path, &out_csv).unwrap();
    let dt = t0.elapsed();

    println!("phase-etl: {in_path} -> {out_path}");
    println!("строк: всего={total} битых={malformed} дублей={dup} распарсено={parsed}");
    println!(
        "парсинг: восстановлено откатами {} B (O(1) на строку)",
        recovered
    );
    println!(
        "A/B нормализация: выбросов base={base_bad} A={bad_a} B={bad_b} -> победил {winner}, финал={final_bad} (0 копий таблицы)"
    );
    println!(
        "коммит: оставлено строк={} (выбросы отброшены на границе)",
        kept.len()
    );
    println!("аудит: детерминированный дайджест 0x{digest:016x}");
    println!("время: {:?}", dt);
    let _ = dup;
}
