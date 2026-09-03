//! phase-guard — глубокий системный слой: краш-консистентный
//! детерминированный stateful-процесс БЕЗ WAL.
//!
//! Модель: N рабочих юнитов (фазы) применяются к состоянию (phase_server::KV)
//! из детерминированного потока. Каждый CP-тый юнит пишется ЧЕКПОИНТ-КАПСУЛА
//! (атомарный файл: снапшот состояния + индекс юнита + цепочка аудитов).
//! «Падение» процесса в любой момент НЕ теряет консистентность: рестарт
//! грузит последнюю капсулу и ДОИГРЫВАЕТ юниты детерминированно — финальный
//! аудит обязан совпасть с непрерывным прогоном (crash-consistency без
//! журнала операций, перезапуск не = «с нуля», а = replay из капсулы).
//!
//!   cargo run --release --example phase_guard        (демо: crash в 60%)

use phase_vm::phase_server::KV;
use std::path::PathBuf;

const N_UNITS: usize = 10_000;
const CP_EVERY: usize = 1000; // чекпоинт раз в N юнитов
const KEYS: [&str; 8] = ["a", "b", "c", "d", "e", "f", "g", "h"];

struct Xs(u64);
impl Xs {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

fn chain_step(prev: u64, x: u64) -> u64 {
    let prime = 0x100000001b3u64;
    (0xcbf2_9ce4_8422_2325u64 ^ prev)
        .wrapping_add(x)
        .wrapping_mul(prime)
}

/// Капсула на диске: "unit:<i>\naudit:<hex>\n---\n<снапшот>"
struct Capsule {
    unit: usize,
    audit: u64,
    state: String,
}

fn write_capsule(dir: &std::path::Path, i: usize, audit: u64, kv: &KV) {
    let body = format!("unit:{i}\naudit:{audit:016x}\n---\n{}", kv.snapshot());
    let tmp = dir.join("capsule.tmp");
    let fin = dir.join("capsule.txt");
    std::fs::write(&tmp, &body).unwrap();
    std::fs::rename(&tmp, &fin).unwrap(); // атомарно
}

fn read_capsule(dir: &std::path::Path) -> Option<Capsule> {
    let p = dir.join("capsule.txt");
    let body = std::fs::read_to_string(&p).ok()?;
    let (head, state) = body.split_once("\n---\n")?;
    let unit = head
        .lines()
        .find_map(|l| l.strip_prefix("unit:"))?
        .parse()
        .ok()?;
    let audit =
        u64::from_str_radix(head.lines().find_map(|l| l.strip_prefix("audit:"))?, 16).ok()?;
    Some(Capsule {
        unit,
        audit,
        state: state.to_string(),
    })
}

/// Прогнать юниты [start..end) детерминированного потока.
fn run_units(kv: &mut KV, start: usize, end: usize, chain: &mut u64, rng: &mut Xs) -> u64 {
    let mut units = 0u64;
    for i in start..end {
        // юнит = фаза: команда + валидация (аудит после каждого юнита)
        let k = KEYS[(rng.next() % KEYS.len() as u64) as usize];
        let v = (rng.next() % 1000).to_string();
        if rng.next().is_multiple_of(5) {
            kv.handle(&format!("DEL {k}"));
        } else {
            kv.handle(&format!("SET {k} {v}"));
        }
        let a = kv.audit_digest();
        *chain = chain_step(*chain, a);
        units += 1;
        let _ = i;
    }
    units
}

fn main() {
    let dir = PathBuf::from("/tmp/phase_guard_data");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // 1) ЭТАЛОН: непрерывный прогон всех юнитов от генезиса
    let mut ref_kv = KV::new();
    let mut ref_chain = 0xcbf2_9ce4_8422_2325u64;
    let mut rng = Xs(0x6A11);
    run_units(&mut ref_kv, 0, N_UNITS, &mut ref_chain, &mut rng);
    let ref_final = ref_kv.audit_digest();

    // 2) «живой» процесс: пишет капсулы, «падает» на 60%
    let crash_at = N_UNITS * 60 / 100;
    let mut kv = KV::new();
    let mut chain = 0xcbf2_9ce4_8422_2325u64;
    let mut rng = Xs(0x6A11);
    let mut checkpointed = 0u64;
    let mut last_cp_unit = 0usize;
    let mut last_cp_chain = chain;
    let mut i = 0;
    while i < crash_at {
        // батч до чекпоинта
        let end = ((i / CP_EVERY) + 1) * CP_EVERY;
        let end = end.min(crash_at);
        run_units(&mut kv, i, end, &mut chain, &mut rng);
        // валидация и капсула на границе
        if end.is_multiple_of(CP_EVERY) {
            write_capsule(&dir, end, kv.audit_digest(), &kv);
            checkpointed += 1;
            last_cp_unit = end;
            last_cp_chain = chain;
        }
        i = end;
    }
    // «процесс упал» на юните crash_at; на диске последняя капсула = last_cp_unit

    // 3) РЕСТАРТ: грузим капсулу, доигрываем до конца
    let cap = read_capsule(&dir).expect("капсула должна быть");
    assert_eq!(cap.unit, last_cp_unit);
    let mut kv2 = KV::from_snapshot(&cap.state);
    assert_eq!(kv2.audit_digest(), cap.audit, "капсула повреждена");
    // реплей с ГЕНЕЗИСА до last_cp_unit (детерминизм: тот же seed)
    let mut chain2 = 0xcbf2_9ce4_8422_2325u64;
    let mut rng2 = Xs(0x6A11);
    let mut skip = KV::new();
    run_units(&mut skip, 0, last_cp_unit, &mut chain2, &mut rng2);
    assert_eq!(
        chain2, last_cp_chain,
        "цепочка на момент капсулы не совпала"
    );
    let units_rest = run_units(&mut kv2, last_cp_unit, N_UNITS, &mut chain2, &mut rng2);
    let final_audit = kv2.audit_digest();

    // 4) СВЕРКА: финал после «смерти+рестарта» == непрерывный прогон
    assert_eq!(final_audit, ref_final, "crash-consistency нарушена");
    assert_eq!(chain2, ref_chain, "цепочка аудитов разошлась");
    println!("phase-guard: N_UNITS={N_UNITS}, чекпоинт раз в {CP_EVERY}");
    println!(
        "«падение» на юните {crash_at}/{N_UNITS} ({}%); капсул записано: {checkpointed}",
        crash_at * 100 / N_UNITS
    );
    println!("рестарт: resume с юнита {last_cp_unit}, доиграно юнитов: {units_rest}");
    println!("финальный аудит == непрерывный прогон (assert): 0x{final_audit:016x}");
    println!("crash-consistency БЕЗ WAL: перезапуск = replay из капсулы, не «с нуля»");
}
