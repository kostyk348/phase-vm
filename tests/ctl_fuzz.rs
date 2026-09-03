//! L3 decision-журнал: fuzz-проверка F⁻¹(F(S))==S на случайных программах
//! с ifnz/wz/rep (data-dependent control-flow). Журнал = только решения.

use phase_vm::ctl::{self, Node};
use phase_vm::inst::Inst;
use phase_vm::state::State;

struct Xs(u64);
impl Xs {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn reg(rng: &mut Xs, n: u8) -> u8 {
    rng.below(n as u64) as u8
}
fn two(rng: &mut Xs, n: u8) -> (u8, u8) {
    let a = reg(rng, n);
    loop {
        let b = reg(rng, n);
        if a != b {
            return (a, b);
        }
    }
}

/// Случайная обратимая «прямая» инструкция (без set/mset), не трогающая r5.
fn rand_inst(rng: &mut Xs, n: u8) -> Inst {
    loop {
        let inst = match rng.below(8) {
            0 => Inst::Not(reg(rng, n)),
            1 => Inst::Inc(reg(rng, n)),
            2 => Inst::Dec(reg(rng, n)),
            3 => {
                let (a, b) = two(rng, n);
                Inst::Xor(a, b)
            }
            4 => {
                let (a, b) = two(rng, n);
                Inst::Add(a, b)
            }
            5 => {
                let (a, b) = two(rng, n);
                Inst::Sub(a, b)
            }
            6 => {
                let (a, b) = two(rng, n);
                Inst::Swap(a, b)
            }
            _ => Inst::RotL(reg(rng, n), (1 + rng.below(63)) as u32),
        };
        // while-счётчик r5 в «ядре» не трогаем
        let touches5 = match inst {
            Inst::Not(a) | Inst::Inc(a) | Inst::Dec(a) | Inst::RotL(a, _) => a == 5,
            Inst::Xor(a, b) | Inst::Add(a, b) | Inst::Sub(a, b) | Inst::Swap(a, b) => {
                a == 5 || b == 5
            }
            _ => false,
        };
        if !touches5 {
            return inst;
        }
    }
}

fn kernel(rng: &mut Xs, n: u8, len: usize) -> Vec<Node> {
    (0..len).map(|_| Node::Inst(rand_inst(rng, n))).collect()
}

fn random_program(rng: &mut Xs, n: u8) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::new();
    let stmts = 3 + rng.below(10) as usize;
    for _ in 0..stmts {
        match rng.below(10) {
            0..=5 => out.push(Node::Inst(rand_inst(rng, n))),
            6..=7 => {
                // ifnz на случайном регистре
                let r = reg(rng, n);
                let bl = 1 + rng.below(3) as usize;
                let body = kernel(rng, n, bl);
                out.push(Node::IfN { reg: r, body });
            }
            _ => {
                // rep со статическим телом
                let count = 1 + rng.below(3);
                let bl2 = 1 + rng.below(3) as usize;
                let body = kernel(rng, n, bl2);
                out.push(Node::Rep { count, body });
            }
        }
    }
    // финальный data-dependent цикл: счётчик r5, тело убывает ровно на 1
    let bl3 = 1 + rng.below(2) as usize;
    let mut body = kernel(rng, n, bl3);
    body.push(Node::Inst(Inst::Dec(5)));
    out.push(Node::WhileN { reg: 5, body });
    out
}

#[test]
fn random_ctl_programs_roundtrip() {
    let mut rng = Xs(0xDEC1DE);
    for trial in 0..300 {
        let n: u8 = 8;
        let nodes = random_program(&mut rng, n);
        let mut s = State::random(n as usize, 0, trial * 104_729 + 7);
        s.regs[5] = rng.below(6); // счётчик небольшой → цикл завершается
        let orig = s.clone();
        let mut j = Vec::new();
        let mut steps = 0u64;
        ctl::forward(&mut s, &nodes, &mut j, &mut steps).unwrap();
        assert!(s != orig, "trial {trial}: программа ничего не сделала");
        let mut steps2 = 0u64;
        ctl::reverse(&mut s, &nodes, &mut j, &mut steps2).unwrap();
        assert_eq!(s, orig, "trial {trial}: F⁻¹(F(S)) != S (ctl)");
        assert!(j.is_empty(), "trial {trial}: журнал не исчерпан");
    }
}

#[test]
fn checked_reverse_matches_on_good_journal() {
    let mut rng = Xs(0x600D);
    for trial in 0..150 {
        let n: u8 = 8;
        let nodes = random_program(&mut rng, n);
        let mut s = State::random(n as usize, 0, trial + 99);
        s.regs[5] = rng.below(5);
        let orig = s.clone();
        let mut j = Vec::new();
        let mut st = 0u64;
        ctl::forward(&mut s, &nodes, &mut j, &mut st).unwrap();
        let mut st2 = 0u64;
        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ctl::reverse_checked(&mut s, &nodes, &mut j, &mut st2).unwrap();
        }));
        assert!(
            ok.is_ok(),
            "trial {trial}: reverse_checked упал на корректном журнале"
        );
        assert_eq!(s, orig, "trial {trial}");
    }
}
