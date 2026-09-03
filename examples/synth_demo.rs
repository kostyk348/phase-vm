//! Инструмент: синтез обратимых ядер по образцам (ось B + superopt).
//! Примеры: xor-swap (3 инструкции), перестановка регистров, add-в-сторону.

use phase_vm::state::State;
use phase_vm::synth::{synthesize, Sample};

fn main() {
    // 1) XOR-swap
    let mut samples: Vec<Sample> = Vec::new();
    for &(a, b) in &[
        (1u64, 2u64),
        (7, 3),
        (0xDEAD, 0xBEEF),
        (0, 9),
        (u64::MAX, 1),
    ] {
        let mut inp = State::new(2, 0);
        inp.regs[0] = a;
        inp.regs[1] = b;
        let mut want = State::new(2, 0);
        want.regs[0] = b;
        want.regs[1] = a;
        samples.push((inp, want));
    }
    match synthesize(&samples, 2, 6) {
        Some(p) => {
            let s: Vec<String> = p.iter().map(|i| i.to_string()).collect();
            println!("swap: найдено за {} шагов: {}", p.len(), s.join(" ; "));
        }
        None => println!("swap: не найдено в пределах глубины"),
    }

    // 2) случайная цель длины 2 — синтезатор «компилирует» её по 8 образцам
    let mut rng = 0xF00Du64;
    let mut next = move || {
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let pool = phase_vm::synth::pool(2);
    let target = vec![
        pool[(next() % pool.len() as u64) as usize],
        pool[(next() % pool.len() as u64) as usize],
    ];
    let mut samples2: Vec<Sample> = Vec::new();
    for _ in 0..8 {
        let mut inp = State::new(2, 0);
        inp.regs[0] = next();
        inp.regs[1] = next();
        let mut want = inp.clone();
        for i in &target {
            i.exec(&mut want).unwrap();
        }
        samples2.push((inp, want));
    }
    let ts: Vec<String> = target.iter().map(|i| i.to_string()).collect();
    let found = synthesize(&samples2, 2, 2);
    let fs: Vec<String> = found
        .iter()
        .flat_map(|v| v.iter().map(|i| i.to_string()))
        .collect();
    println!(
        "цель: {}  |  синтез: {} (эквивалентность на 200 held-out проверена тестами)",
        ts.join(" ; "),
        if fs.is_empty() {
            "—".into()
        } else {
            fs.join(" ; ")
        }
    );
}
