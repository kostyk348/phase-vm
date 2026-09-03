//! Математическая оптимизация (алгебра слов) на практике:
//! сколько листьев срезает оптимизатор на шумной программе и на реальных
//! ядрах (mixer, PC1-раунд), и как это ускоряет исполнение.

use phase_vm::machine::count_leaves;
use phase_vm::opt;
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

fn main() {
    // 1) «шумная» программа: много обратимых пар вперемешку
    let mut rng = Xs(0xC0DE);
    let mut insts = Vec::new();
    let n0 = 10_000usize;
    for _ in 0..n0 {
        let op = rng.next() % 6;
        let (d, s) = ((rng.next() % 8) as u8, (rng.next() % 8) as u8);
        if d == s {
            continue;
        }
        use phase_vm::inst::Inst::*;
        insts.push(match op {
            0 => Add(d, s),
            1 => Sub(d, s),
            2 => Xor(d, s),
            3 => Swap(d, s),
            4 => RotL(d, (rng.next() % 63) as u32),
            _ => RotR(d, (rng.next() % 63) as u32),
        });
    }
    let before = insts.len();
    let optd = opt::optimize(&insts);
    println!(
        "шумная программа: {} -> {} листьев (срезано {:.1}%)",
        before,
        optd.len(),
        100.0 * (before - optd.len()) as f64 / before as f64
    );

    // эквивалентность на random
    let mut a = State::random(8, 0, 5);
    let orig = a.clone();
    let mut b = orig.clone();
    for i in &insts {
        i.exec(&mut a).unwrap();
    }
    for i in &optd {
        i.exec(&mut b).unwrap();
    }
    assert_eq!(a, b);

    // 2) реальные ядра: mixer-раунд и PC1-раунд
    for (name, src) in [
        (
            "mixer-round",
            "add r0 r1\nrotl r1 7\nxor r1 r0\nswp r0 r1\nadd r2 r3\nrotl r3 11\nxor r3 r2\nswp r2 r3\n",
        ),
        (
            "pc1-round",
            phase_vm::cipher::PC1_ROUND_BODY,
        ),
    ] {
        let nodes = parse(src, 16).unwrap().nodes;
        let leaves_before = count_leaves(&nodes);
        // собрать плоский список листьев из rep-тела? здесь без rep — уже плоские
        let mut flat = Vec::new();
        for n in &nodes {
            match n {
                phase_vm::program::Node::Inst(i) => flat.push(*i),
                phase_vm::program::Node::Rep { body, .. } => {
                    for nn in body {
                        if let phase_vm::program::Node::Inst(i) = nn {
                            flat.push(*i);
                        }
                    }
                }
            }
        }
        let _ = leaves_before;
        let optd = opt::optimize(&flat);
        println!(
            "{}: {} -> {} листьев",
            name,
            flat.len(),
            optd.len()
        );
    }
}
