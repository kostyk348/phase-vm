//! Типовая задача: парсинг с восстановлением после ошибок.
//!
//! Классика: на семантической ошибке записи — вручную чистить частично
//! созданные объекты. Парадигма: каждая запись парсится в СВОЮ mark-область
//! Arena; провал → rollback(mark) — объекты записи исчезают за O(1),
//! ошибка логируется, парсинг продолжается со следующей записи.

use phase_vm::alloc::Arena;
use phase_vm::state::State;

struct Xs(u64);
impl Xs {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

const RECORDS: usize = 200_000;
const FIELDS: usize = 6;
const FAIL_RATE: u64 = 15; // % записей «семантически невалидны»

fn main() {
    let mut rng = Xs(0xFEE1);
    // Поток «токенов»: каждая запись — FIELDS чисел; генерим заранее (детерминизм)
    let mut stream = Vec::with_capacity(RECORDS * FIELDS);
    for _ in 0..RECORDS * FIELDS {
        stream.push(rng.next() % 1000);
    }

    let mut arena = Arena::new();
    let mut ok_records = 0u64;
    let mut bad_records = 0u64;
    let mut bytes_recovered = 0usize;

    let mut k = 0usize;
    for _r in 0..RECORDS {
        let rec_mark = arena.push_mark();
        // «парсим» запись: выделяем FIELDS слов под неё (типичный объект-запись)
        let mut fields = Vec::with_capacity(FIELDS);
        for _ in 0..FIELDS {
            let off = arena.alloc(8).unwrap();
            let v = stream[k];
            k += 1;
            arena.write_u64(off, v);
            fields.push(off);
        }
        // семантическая валидация (типичное правило: сумма полей кратна 7?)
        let sum: u64 = fields.iter().map(|&o| arena.read_u64(o)).sum();
        if rng.next() % 100 < FAIL_RATE || sum % 7 == 1 {
            // запись невалидна → O(1)-откат её объектов
            let used = arena.used_bytes();
            arena.rollback(rec_mark);
            bytes_recovered += used - arena.used_bytes();
            bad_records += 1;
        } else {
            arena.commit(); // запись остаётся (живёт до конца фазы-пакета)
            ok_records += 1;
        }
    }
    println!("typical parse: RECORDS={RECORDS}, FIELDS={FIELDS}, ~{FAIL_RATE}% невалидных");
    println!("валидных={ok_records} отброшено={bad_records}");
    println!(
        "Arena живая: {} B; восстановлено откатами: {} B (O(1) на запись, без per-object free)",
        arena.used_bytes(),
        bytes_recovered
    );
    // классика: per-object drop FIELDS слов × bad_records освобождений
    println!(
        "классика сделала бы {} индивидуальных free; здесь — {} rollback-ов по O(1)",
        bad_records * FIELDS as u64,
        bad_records
    );
    let _ = State::new(0, 0);
}
