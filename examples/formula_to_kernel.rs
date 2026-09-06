//! Замыкание петли: МАТЕМАТИЧЕСКАЯ ФОРМУЛА (биекции) → обратимый kernel
//! phase-vm → обратная инверсия машины == аналитическая инверсия математики.
//!
//! Формула = композиция унарных биекций над x: rotl(k), add(c), xor(k)
//! (MulOdd не выражается в ISA без умножения — см. phase-math). Компиляция:
//! константы c уходят на границу (регистры-ключи), ядро = add/xor/rotl над r0.
//!   math.apply(x) == VM.forward(x)
//!   VM.reverse(forward(x)) == x == math.inverse(forward(x))

use std::time::Instant;

use phase_vm::machine::{reverse_all, run_forward};
use phase_vm::pmath::{self, Op};
use phase_vm::program::parse;
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

fn apply_all(ops: &[Op], mut x: u64) -> u64 {
    for o in ops {
        x = o.apply(x);
    }
    x
}

/// Скомпилировать формулу в текст: ВСЕ константы — граница (set в начале),
/// затем чисто обратимое ядро (без границ) — откат работает по всему ядру.
fn compile(ops: &[Op]) -> (String, Vec<u64>) {
    let mut keys: Vec<u64> = Vec::new();
    let mut body = String::new();
    for op in ops {
        match *op {
            Op::RotL(r) => body.push_str(&format!("rotl r0 {}\n", r & 63)),
            Op::Add(c) => {
                keys.push(c);
                let reg = keys.len();
                body.push_str(&format!("add r0 r{reg}\n"));
            }
            Op::Xor(k) => {
                keys.push(k);
                let reg = keys.len();
                body.push_str(&format!("xor r0 r{reg}\n"));
            }
            Op::MulOdd(m) => body.push_str(&format!("mulc r0 {:#x}\n", m)),
        }
    }
    let mut text = String::new();
    for (i, k) in keys.iter().enumerate() {
        text.push_str(&format!("set r{} {}\n", i + 1, k));
    }
    text.push_str(&body);
    (text, keys)
}

fn main() {
    let mut rng = Xs(0xF0F0);
    let mut ok = 0u64;
    let t0 = Instant::now();
    for trial in 0..2000u64 {
        let n = 4 + (rng.next() % 7) as usize;
        let mut ops = Vec::with_capacity(n);
        for _ in 0..n {
            ops.push(match rng.next() % 4 {
                0 => Op::RotL((rng.next() % 63) as u32 + 1),
                1 => Op::Add(rng.next()),
                2 => Op::Xor(rng.next()),
                _ => Op::MulOdd(rng.next() | 1), // нечётное
            });
        }
        let (text, keys) = compile(&ops);
        let nodes = parse(&text, 16).unwrap().nodes;

        let x = rng.next();
        let math_fwd = apply_all(&ops, x);
        let math_inv = pmath::inverse(&ops);

        let mut s = State::new(16, 0);
        for (i, k) in keys.iter().enumerate() {
            s.regs[1 + i] = *k;
        }
        s.regs[0] = x;
        run_forward(&mut s, &nodes).unwrap();
        let vm_fwd = s.regs[0];

        // 1) математика == машина
        assert_eq!(vm_fwd, math_fwd, "trial {trial}: VM != математика");
        // 2) обратный прогон машины возвращает x (0 байт логов)
        reverse_all(&mut s, &nodes).unwrap();
        assert_eq!(s.regs[0], x, "trial {trial}: VM reverse не вернул x");
        // 3) математическая инверсия даёт x
        assert_eq!(
            apply_all(&math_inv, math_fwd),
            x,
            "trial {trial}: math inverse"
        );
        ok += 1;
    }
    let dt = t0.elapsed();

    println!("формула→kernel: {ok} случайных формул (rotl/add/xor/mulc, 4..10 звеньев)");
    println!("VM.forward == math.apply; VM.reverse == x == math.inverse (assert)");
    println!("инверсия машины БЕЗ логов == аналитическая инверсия математики");
    println!(
        "время: {:?} ({:.0} ns/формула)",
        dt,
        dt.as_nanos() as f64 / ok as f64
    );
}
