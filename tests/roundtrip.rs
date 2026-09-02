//! Property-тесты: F⁻¹(F(S)) == S на случайных обратимых программах.
//! Детерминированный xorshift — без внешних зависимостей.

use phase_vm::inst::Inst;
use phase_vm::machine::{count_leaves, iter_reverse, reverse_all, reverse_n, run_forward};
use phase_vm::program::Node;
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

fn rand_reg(rng: &mut Xs, regs: u8) -> u8 {
    rng.below(regs as u64) as u8
}

fn rand_two(rng: &mut Xs, regs: u8) -> (u8, u8) {
    let a = rand_reg(rng, regs);
    loop {
        let b = rand_reg(rng, regs);
        if a != b {
            return (a, b);
        }
    }
}

/// Случайная чисто обратимая программа без `set` и без алиасинга.
fn random_reversible_prog(rng: &mut Xs, regs: u8, len: usize) -> Vec<Node> {
    let mut nodes = Vec::new();
    for _ in 0..len {
        let op = rng.below(8);
        let inst = match op {
            0 => Inst::Not(rand_reg(rng, regs)),
            1 => Inst::Inc(rand_reg(rng, regs)),
            2 => Inst::Dec(rand_reg(rng, regs)),
            3 => {
                let (a, b) = rand_two(rng, regs);
                Inst::Xor(a, b)
            }
            4 => {
                let (a, b) = rand_two(rng, regs);
                Inst::Add(a, b)
            }
            5 => {
                let (a, b) = rand_two(rng, regs);
                Inst::Sub(a, b)
            }
            6 => {
                let (a, b) = rand_two(rng, regs);
                Inst::Swap(a, b)
            }
            _ => {
                let (a, _b) = rand_two(rng, regs);
                Inst::RotL(a, (1 + rng.below(63)) as u32)
            }
        };
        debug_assert!(inst.operand_alias().is_none());
        nodes.push(Node::Inst(inst));
    }
    // изредка оборачиваем весь хвост в rep — проверка потоковой развёртки
    if len > 4 && rng.below(4) == 0 {
        let split = 2 + rng.below((len - 2) as u64) as usize;
        let body: Vec<Node> = nodes.split_off(split);
        let count = 1 + rng.below(4);
        nodes.push(Node::Rep { count, body });
    }
    nodes
}

#[test]
fn random_programs_roundtrip_bit_exact() {
    let mut rng = Xs(0xC0FFEE);
    for trial in 0..300 {
        let nregs: u8 = 4 + rng.below(12) as u8;
        let len = 1 + rng.below(40) as usize;
        let nodes = random_reversible_prog(&mut rng, nregs, len);
        let leaves = count_leaves(&nodes);
        if leaves == 0 {
            continue;
        }
        let orig = State::random(nregs as usize, 0, trial * 7919 + 13);
        let mut s = orig.clone();
        run_forward(&mut s, &nodes).unwrap();
        let after = s.clone();
        let done = reverse_all(&mut s, &nodes).unwrap();
        assert_eq!(done, leaves, "trial {trial}: не весь поток откачен");
        assert_eq!(s, orig, "trial {trial}: F⁻¹(F(S)) != S (leaves={leaves})");
        if after != orig {
            assert_eq!(s, orig);
        }
    }
}

#[test]
fn partial_reverse_matches_naive_undo() {
    // Откат K листьев должен совпасть с наивным применением inverses
    // последних K инструкций — контроль семантики reverse_n.
    let mut rng = Xs(0xBEEF);
    for trial in 0..200 {
        let nregs: u8 = 8;
        let len = 20 + rng.below(30) as usize;
        let nodes = random_reversible_prog(&mut rng, nregs, len);
        let leaves = count_leaves(&nodes);
        if leaves < 3 {
            continue;
        }
        let orig = State::random(nregs as usize, 0, trial + 7);
        let mut s = orig.clone();
        run_forward(&mut s, &nodes).unwrap();

        let k = 1 + rng.below(leaves.min(15));
        let mut expected = s.clone();
        {
            let rev: Vec<Inst> = iter_reverse(&nodes).take(k as usize).collect();
            for inst in rev {
                let inv = inst.inverse().unwrap();
                inv.exec(&mut expected).unwrap();
            }
        }
        let mut got = s.clone();
        let done = reverse_n(&mut got, &nodes, k).unwrap();
        assert_eq!(done, k, "trial {trial}");
        assert_eq!(got, expected, "trial {trial}: partial reverse расходится");
    }
}

// ---------- память (проект b): обратимость mem-операций ----------

#[test]
fn ledger_transfer_rollback_in_memory() {
    // Транзакция-перевод: откат возвращает память к границе, ввод цел.
    let nodes = phase_vm::program::parse(
        "set r0 1\nset r1 2\nmset 1 1000\nmset 2 500\nset r2 7\nmsub r0 r2\nmadd r1 r2\n",
        8,
    )
    .unwrap()
    .nodes;
    let mut s = State::new(8, 8);
    run_forward(&mut s, &nodes).unwrap();
    assert_eq!((s.mem[1], s.mem[2]), (993, 507));
    assert_eq!(s.regs[2], 7, "сумма перевода — источник, не уничтожена");

    let done = reverse_all(&mut s, &nodes).unwrap();
    assert_eq!(done, 2, "откатывается только ядро (msub+madd)");
    assert_eq!((s.mem[1], s.mem[2]), (1000, 500), "память откачена");
    assert_eq!((s.regs[0], s.regs[1], s.regs[2]), (1, 2, 7), "ввод цел");
}

#[test]
fn pure_mem_kernel_roundtrip_from_random() {
    // Чисто обратимое ядро с памятью: F⁻¹(F(S)) == S, включая память.
    let nodes = phase_vm::program::parse(
        "madd r5 r0\nmsub r5 r1\nmxor r5 r2\nrmadd r3 r5\nrmsub r3 r5\n",
        8,
    )
    .unwrap()
    .nodes;
    for seed in 1..100u64 {
        let mut s = State::random(8, 16, seed);
        s.regs[5] = seed % 16; // адрес всегда в диапазоне памяти
        let orig = s.clone();
        run_forward(&mut s, &nodes).unwrap();
        assert_ne!(s.mem, orig.mem, "seed {seed}: память не менялась");
        let done = reverse_all(&mut s, &nodes).unwrap();
        assert_eq!(done, 5);
        assert_eq!(s, orig, "seed {seed}: F⁻¹(F(S)) != S с памятью");
    }
}

#[test]
fn mset_is_boundary() {
    // mset в ядре = граница: суффикс после неё откатывается, сама — нет.
    let nodes = phase_vm::program::parse("mset 3 42\nmadd r0 r1\n", 4)
        .unwrap()
        .nodes;
    let mut s = State::new(4, 8);
    run_forward(&mut s, &nodes).unwrap();
    assert_eq!(s.mem[3], 42);
    let done = reverse_all(&mut s, &nodes).unwrap();
    assert_eq!(done, 1, "mset не откатывается");
    assert_eq!(s.mem[3], 42, "граница цела");
}
