//! Reversible property testing (audit deliverable):
//!
//! 1. a generator of random **valid** programs covering every invertible
//!    opcode (registers, memory, `mulc`, `toff`, `cswp`, `rep` nesting);
//! 2. the core property `inverse(forward(S)) == S` — per instruction, for
//!    random program fragments, and for partial rollback of K leaves;
//! 3. a hand-rolled **shrinker** (delta-debugging): planted bugs must be
//!    shrunk to a minimal counterexample. No external dependencies — the
//!    crate keeps its zero-dependency claim.

use phase_vm::check;
use phase_vm::inst::Inst;
use phase_vm::machine::{count_leaves, iter_reverse, reverse_all, reverse_n, run_forward};
use phase_vm::program::Node;
use phase_vm::state::State;

// ---------- deterministic rng (xorshift64*) ----------

struct Xs(u64);
impl Xs {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }
}

// ---------- generator of random VALID programs ----------

const NMEM: usize = 8;

/// Registers that only ever hold memory addresses (never written by the
/// generator), so generated memory ops always address valid memory.
fn addr_regs(nregs: u8) -> Vec<u8> {
    vec![nregs - 1, nregs - 2]
}

/// A random state where address registers hold valid addresses.
fn gen_state(rng: &mut Xs, nregs: u8) -> State {
    let mut s = State::random(nregs as usize, NMEM, rng.next());
    for (p, &r) in addr_regs(nregs).iter().enumerate() {
        s.regs[r as usize] = rng.below(NMEM as u64) + p as u64; // distinct, in range
        if s.regs[r as usize] >= NMEM as u64 {
            s.regs[r as usize] %= NMEM as u64;
        }
    }
    s
}

fn rand_reg(rng: &mut Xs, regs: u8) -> u8 {
    // normal registers only: never the address registers
    rng.below((regs - 2) as u64) as u8
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

/// A normal register different from `except` (needs >= 2 normal registers,
/// guaranteed for every `regs` used by the generator).
fn rand_reg_except(rng: &mut Xs, regs: u8, except: u8) -> u8 {
    loop {
        let r = rand_reg(rng, regs);
        if r != except {
            return r;
        }
    }
}

/// One random invertible leaf over all reversible opcodes.
fn rand_inst(rng: &mut Xs, regs: u8) -> Inst {
    let addrs = addr_regs(regs);
    match rng.below(16) {
        0 => Inst::Not(rand_reg(rng, regs)),
        1 => Inst::Inc(rand_reg(rng, regs)),
        2 => Inst::Dec(rand_reg(rng, regs)),
        3 => {
            let (d, s) = rand_two(rng, regs);
            Inst::Xor(d, s)
        }
        4 => {
            let (d, s) = rand_two(rng, regs);
            Inst::Add(d, s)
        }
        5 => {
            let (d, s) = rand_two(rng, regs);
            Inst::Sub(d, s)
        }
        6 => {
            let (a, b) = rand_two(rng, regs);
            Inst::Swap(a, b)
        }
        7 => Inst::RotL(rand_reg(rng, regs), rng.below(64) as u32),
        8 => Inst::RotR(rand_reg(rng, regs), rng.below(64) as u32),
        9 => Inst::MulC(rand_reg(rng, regs), rng.next() | 1), // odd: bijection
        10 => {
            // toff c1 c2 t with t != c1, t != c2 (c1 == c2 is allowed by the
            // verifier) — draw the constrained target first
            let t = rand_reg(rng, regs);
            let c1 = rand_reg_except(rng, regs, t);
            let c2 = rand_reg_except(rng, regs, t);
            Inst::Toff(c1, c2, t)
        }
        11 => {
            // cswp c a b with c != a, c != b (a == b allowed: identity) —
            // draw the constrained control first
            let c = rand_reg(rng, regs);
            let a = rand_reg_except(rng, regs, c);
            let b = rand_reg_except(rng, regs, c);
            Inst::CSwap(c, a, b)
        }
        12 => Inst::MAdd(addrs[rng.below(2) as usize], rand_reg(rng, regs)),
        13 => Inst::MSub(addrs[rng.below(2) as usize], rand_reg(rng, regs)),
        14 => Inst::MXor(addrs[rng.below(2) as usize], rand_reg(rng, regs)),
        _ => {
            // rmadd/rmsub: dst is a normal register, address from addr regs
            let dst = rand_reg(rng, regs);
            let a = addrs[rng.below(2) as usize];
            if rng.below(2) == 0 {
                Inst::RAdd(dst, a)
            } else {
                Inst::RSub(dst, a)
            }
        }
    }
}

/// Random program: flat leaves plus `rep` nesting (1..=2 levels).
fn random_prog(rng: &mut Xs, regs: u8, len: usize) -> Vec<Node> {
    let mut nodes: Vec<Node> = (0..len).map(|_| Node::Inst(rand_inst(rng, regs))).collect();
    if nodes.len() > 4 && rng.below(3) == 0 {
        let split = 1 + rng.below((nodes.len() - 1) as u64) as usize;
        let body: Vec<Node> = nodes.split_off(split);
        let count = 1 + rng.below(4);
        nodes.push(Node::Rep { count, body });
    }
    if nodes.len() > 8 && rng.below(5) == 0 {
        let split = 1 + rng.below((nodes.len() - 1) as u64) as usize;
        let body: Vec<Node> = nodes.split_off(split);
        let count = 1 + rng.below(3);
        nodes.push(Node::Rep { count, body });
    }
    nodes
}

// ---------- the core properties ----------

/// inverse(forward(S)) == S for a single random instruction, per opcode.
#[test]
fn per_instruction_roundtrip_all_opcodes() {
    let mut rng = Xs(0xC0FF_EE01);
    // one representative per opcode family (+ parametric variants)
    let cases: Vec<Inst> = vec![
        Inst::Nop,
        Inst::Not(3),
        Inst::Inc(3),
        Inst::Dec(3),
        Inst::Xor(1, 2),
        Inst::Add(1, 2),
        Inst::Sub(1, 2),
        Inst::Swap(1, 2),
        Inst::Swap(1, 1), // identity
        Inst::RotL(3, 0),
        Inst::RotL(3, 17),
        Inst::RotL(3, 63),
        Inst::RotR(3, 1),
        Inst::MulC(3, 3),
        Inst::MulC(3, u64::MAX),
        Inst::MulC(3, 0x9E37_79B9_7F4A_7C15),
        Inst::Toff(1, 2, 3),
        Inst::CSwap(1, 2, 3),
        Inst::CSwap(1, 2, 2), // a == b: conditional identity
        Inst::MAdd(6, 1),
        Inst::MSub(6, 2),
        Inst::MXor(7, 3),
        Inst::RAdd(1, 6),
        Inst::RSub(2, 7),
    ];
    for inst in &cases {
        // inverse(inverse(i)) == i at the instruction level
        let inv = inst
            .inverse()
            .unwrap_or_else(|| panic!("{inst}: no inverse"));
        assert_eq!(
            inv.inverse().unwrap(),
            *inst,
            "{inst}: inverse is not involutive"
        );
        for seed in 0..150u64 {
            let nregs = 8u8;
            let mut s = gen_state(&mut rng, nregs);
            let orig = s.clone();
            inst.exec(&mut s)
                .unwrap_or_else(|e| panic!("{inst} seed {seed}: {e}"));
            inv.exec(&mut s)
                .unwrap_or_else(|e| panic!("inv({inst}) seed {seed}: {e}"));
            assert_eq!(s, orig, "{inst} seed {seed}: F⁻¹(F(S)) != S");
        }
    }
}

/// inverse(forward(S)) == S for random valid programs (all opcodes + rep).
#[test]
fn random_valid_programs_roundtrip_bit_exact() {
    let mut rng = Xs(0xFEED_1234);
    for trial in 0..400u64 {
        let nregs: u8 = 6 + rng.below(10) as u8;
        let len = 1 + rng.below(30) as usize;
        let nodes = random_prog(&mut rng, nregs, len);
        let leaves = count_leaves(&nodes);
        if leaves == 0 {
            continue;
        }
        // the generator promises validity: the verifier must agree
        let rep = check::check(&nodes);
        assert!(
            rep.reversible(),
            "trial {trial}: generator produced an invalid program: {:?}",
            rep.violations
        );
        let orig = gen_state(&mut rng, nregs);
        let mut s = orig.clone();
        run_forward(&mut s, &nodes).unwrap();
        let done = reverse_all(&mut s, &nodes).unwrap();
        assert_eq!(done, leaves, "trial {trial}: not all leaves rolled back");
        assert_eq!(s, orig, "trial {trial}: F⁻¹(F(S)) != S (leaves={leaves})");
    }
}

/// Rollback of K leaves == naive application of the last K inverses.
#[test]
fn partial_reverse_matches_naive_undo_all_opcodes() {
    let mut rng = Xs(0xBEEF_4242);
    for trial in 0..250u64 {
        let nregs: u8 = 8;
        let len = 10 + rng.below(25) as usize;
        let nodes = random_prog(&mut rng, nregs, len);
        let leaves = count_leaves(&nodes);
        if leaves < 2 {
            continue;
        }
        let orig = gen_state(&mut rng, nregs);
        let mut s = orig.clone();
        run_forward(&mut s, &nodes).unwrap();
        let k = 1 + rng.below(leaves.min(12));
        let mut expected = s.clone();
        for inst in iter_reverse(&nodes).take(k as usize) {
            let inv = inst.inverse().unwrap();
            inv.exec(&mut expected).unwrap();
        }
        let mut got = s.clone();
        let done = reverse_n(&mut got, &nodes, k).unwrap();
        assert_eq!(done, k, "trial {trial}");
        assert_eq!(got, expected, "trial {trial}: partial reverse diverges");
    }
}

// ---------- shrinker (hand-rolled delta-debugging) ----------

/// A complexity measure: accepted shrink steps must strictly decrease it.
fn complexity(i: &Inst) -> u64 {
    match *i {
        Inst::Nop => 0,
        Inst::Not(r) | Inst::Inc(r) | Inst::Dec(r) => 1 + r as u64,
        Inst::Xor(d, s) | Inst::Add(d, s) | Inst::Sub(d, s) => 2 + d as u64 * 64 + s as u64,
        Inst::Swap(a, b) => 2 + a as u64 * 64 + b as u64,
        Inst::RotL(r, k) | Inst::RotR(r, k) => 3 + r as u64 * 64 + k as u64,
        Inst::MulC(r, m) => {
            4 + r as u64 * 64
                + if m == 1 {
                    0
                } else if m == 3 {
                    1
                } else {
                    2
                }
        }
        Inst::Toff(a, b, t) => 5 + a as u64 * 4096 + b as u64 * 64 + t as u64,
        Inst::CSwap(a, b, c) => 5 + a as u64 * 4096 + b as u64 * 64 + c as u64,
        Inst::Set(r, imm) => 6 + r as u64 * 64 + u64::from(imm != 0),
        Inst::MAdd(a, v) | Inst::MSub(a, v) | Inst::MXor(a, v) => 7 + a as u64 * 64 + v as u64,
        Inst::RAdd(r, a) | Inst::RSub(r, a) => 7 + r as u64 * 64 + a as u64,
        Inst::MSet(addr, imm) => 8 + addr + u64::from(imm != 0),
    }
}

/// Strictly simpler variants of an instruction (each lowers `complexity`).
fn simpler(i: &Inst) -> Vec<Inst> {
    let mut out = Vec::new();
    match *i {
        Inst::Nop => {}
        Inst::Not(r) | Inst::Inc(r) | Inst::Dec(r) => {
            if r > 0 {
                out.push(match i {
                    Inst::Not(..) => Inst::Not(r - 1),
                    Inst::Inc(..) => Inst::Inc(r - 1),
                    _ => Inst::Dec(r - 1),
                });
            }
        }
        Inst::Xor(d, s) | Inst::Add(d, s) | Inst::Sub(d, s) => {
            if s > 0 {
                out.push(match i {
                    Inst::Xor(..) => Inst::Xor(d, s - 1),
                    Inst::Add(..) => Inst::Add(d, s - 1),
                    _ => Inst::Sub(d, s - 1),
                });
            }
            if d > 0 {
                out.push(match i {
                    Inst::Xor(..) => Inst::Xor(d - 1, s),
                    Inst::Add(..) => Inst::Add(d - 1, s),
                    _ => Inst::Sub(d - 1, s),
                });
            }
        }
        Inst::Swap(a, b) => {
            if b > 0 {
                out.push(Inst::Swap(a, b - 1));
            }
            if a > 0 {
                out.push(Inst::Swap(a - 1, b));
            }
        }
        Inst::RotL(r, k) | Inst::RotR(r, k) => {
            if k > 0 {
                out.push(match i {
                    Inst::RotL(..) => Inst::RotL(r, k - 1),
                    _ => Inst::RotR(r, k - 1),
                });
            }
            if r > 0 {
                out.push(match i {
                    Inst::RotL(..) => Inst::RotL(r - 1, k),
                    _ => Inst::RotR(r - 1, k),
                });
            }
        }
        Inst::MulC(r, m) => {
            if m > 3 {
                out.push(Inst::MulC(r, 3));
            }
            if m > 1 {
                out.push(Inst::MulC(r, 1));
            }
            if r > 0 {
                out.push(Inst::MulC(r - 1, m));
            }
        }
        Inst::Toff(a, b, t) => {
            if t > 0 {
                out.push(Inst::Toff(a, b, t - 1));
            }
            if b > 0 {
                out.push(Inst::Toff(a, b - 1, t));
            }
            if a > 0 {
                out.push(Inst::Toff(a - 1, b, t));
            }
        }
        Inst::CSwap(a, b, c) => {
            if c > 0 {
                out.push(Inst::CSwap(a, b, c - 1));
            }
            if b > 0 {
                out.push(Inst::CSwap(a, b - 1, c));
            }
            if a > 0 {
                out.push(Inst::CSwap(a - 1, b, c));
            }
        }
        Inst::Set(r, imm) => {
            if imm > 0 {
                out.push(Inst::Set(r, 0));
            }
            if r > 0 {
                out.push(Inst::Set(r - 1, imm));
            }
        }
        Inst::MAdd(a, v) | Inst::MSub(a, v) | Inst::MXor(a, v) => {
            if v > 0 {
                out.push(match i {
                    Inst::MAdd(..) => Inst::MAdd(a, v - 1),
                    Inst::MSub(..) => Inst::MSub(a, v - 1),
                    _ => Inst::MXor(a, v - 1),
                });
            }
            if a > 0 {
                out.push(match i {
                    Inst::MAdd(..) => Inst::MAdd(a - 1, v),
                    Inst::MSub(..) => Inst::MSub(a - 1, v),
                    _ => Inst::MXor(a - 1, v),
                });
            }
        }
        Inst::RAdd(r, a) | Inst::RSub(r, a) => {
            if a > 0 {
                out.push(match i {
                    Inst::RAdd(..) => Inst::RAdd(r, a - 1),
                    _ => Inst::RSub(r, a - 1),
                });
            }
            if r > 0 {
                out.push(match i {
                    Inst::RAdd(..) => Inst::RAdd(r - 1, a),
                    _ => Inst::RSub(r - 1, a),
                });
            }
        }
        Inst::MSet(addr, imm) => {
            if imm > 0 {
                out.push(Inst::MSet(addr, 0));
            }
            if addr > 0 {
                out.push(Inst::MSet(addr - 1, imm));
            }
        }
    }
    out
}

/// Delta-debugging shrinker: `prop` returns true when the program SATISFIES
/// the property; `fail` violates it. Returns a smaller program that still
/// violates it (local minimum: no single deletion/simplification helps).
fn shrink(fail: &[Inst], prop: &dyn Fn(&[Inst]) -> bool) -> Vec<Inst> {
    assert!(!prop(fail), "shrink: input must violate the property");
    let mut cur: Vec<Inst> = fail.to_vec();
    loop {
        let mut changed = false;
        // 1) delete chunks (halves, then quarters, ..., then singles)
        let mut chunk = (cur.len() / 2).max(1);
        while chunk >= 1 {
            let mut i = 0;
            while i < cur.len() {
                let mut cand = Vec::with_capacity(cur.len() - chunk.min(cur.len() - i));
                cand.extend_from_slice(&cur[..i]);
                cand.extend_from_slice(&cur[(i + chunk).min(cur.len())..]);
                if !cand.is_empty() && !prop(&cand) {
                    cur = cand;
                    changed = true;
                    i = 0; // restart scan at the new length
                } else {
                    i += chunk;
                }
            }
            if changed {
                break;
            }
            if chunk == 1 {
                break;
            }
            chunk = (chunk / 2).max(1);
        }
        // 2) simplify single instructions
        for i in 0..cur.len() {
            for cand_inst in simpler(&cur[i]) {
                if complexity(&cand_inst) >= complexity(&cur[i]) {
                    continue; // shrink steps must strictly simplify (termination)
                }
                let mut cand = cur.clone();
                cand[i] = cand_inst;
                if !prop(&cand) {
                    cur = cand;
                    changed = true;
                    break;
                }
            }
            if changed {
                break;
            }
        }
        if !changed {
            return cur;
        }
    }
}

/// A "buggy machine" property: like the real round-trip but with a WRONG
/// inverse for `Add` (simulates a semantics bug to be caught and shrunk).
/// Runs on states with all-normal-registers non-zero so the bug always bites.
fn buggy_roundtrip_prop(prog: &[Inst]) -> bool {
    let mut rng = Xs(0x5EED_0001);
    for _ in 0..3 {
        let mut s = State::random(8, NMEM, rng.next());
        for r in 0..6 {
            s.regs[r] |= 1;
        }
        for (p, &r) in addr_regs(8).iter().enumerate() {
            s.regs[r as usize] = (1 + p) as u64;
        }
        let orig = s.clone();
        for i in prog {
            if i.exec(&mut s).is_err() {
                return true; // out of domain: not a counterexample
            }
        }
        for i in prog.iter().rev() {
            let inv = match *i {
                Inst::Add(d, s_) => Inst::Add(d, s_), // THE BUG: add is its own inverse
                other => other.inverse().unwrap(),
            };
            if inv.exec(&mut s).is_err() {
                return true;
            }
        }
        if s != orig {
            return false; // property violated
        }
    }
    true
}

#[test]
fn shrinker_minimizes_planted_bug_to_single_add() {
    let mut rng = Xs(0xFA11_0001);
    for trial in 0..20u64 {
        // generate a random program until it fails under the buggy machine
        let mut attempts = 0;
        let prog: Vec<Inst> = loop {
            attempts += 1;
            assert!(attempts < 1000, "no failing program found");
            let n = 2 + rng.below(10) as usize;
            let cand: Vec<Inst> = (0..n)
                .map(|_| match rng.below(4) {
                    // Xor with DISTINCT operands (Xor(d,d) loses information
                    // even under a correct inverse and would be a different
                    // counterexample family than the planted bug)
                    0 => {
                        let (d, s) = rand_two(&mut rng, 8);
                        Inst::Xor(d, s)
                    }
                    1 => Inst::RotL(rand_reg(&mut rng, 8), rng.below(8) as u32),
                    2 => Inst::Inc(rand_reg(&mut rng, 8)),
                    _ => {
                        let (d, s) = rand_two(&mut rng, 8);
                        Inst::Add(d, s)
                    }
                })
                .collect();
            if !buggy_roundtrip_prop(&cand) {
                break cand;
            }
        };
        let shrunk = shrink(&prog, &buggy_roundtrip_prop);
        assert!(
            !buggy_roundtrip_prop(&shrunk),
            "shrunk program must still fail"
        );
        assert_eq!(
            shrunk.len(),
            1,
            "trial {trial}: not minimal: {prog:?} -> {shrunk:?}"
        );
        assert!(
            matches!(shrunk[0], Inst::Add(..)),
            "trial {trial}: expected a single add, got {shrunk:?}"
        );
        // idempotence: shrinking the fixpoint changes nothing
        assert_eq!(shrink(&shrunk, &buggy_roundtrip_prop), shrunk);
    }
}

/// The checker-based property: "program is purely reversible". A planted
/// `set` must shrink to exactly one boundary leaf.
#[test]
fn shrinker_minimizes_planted_boundary() {
    let mut rng = Xs(0xB0A_0001);
    for _trial in 0..20u64 {
        let n = 3 + rng.below(8) as usize;
        let mut prog: Vec<Inst> = (0..n).map(|_| rand_inst(&mut rng, 8)).collect();
        let pos = rng.below(n as u64) as usize;
        prog[pos] = Inst::Set(rand_reg(&mut rng, 8), rng.next());
        let prop = |p: &[Inst]| -> bool {
            let nodes: Vec<Node> = p.iter().map(|i| Node::Inst(*i)).collect();
            check::check(&nodes).reversible()
        };
        assert!(!prop(&prog));
        let shrunk = shrink(&prog, &prop);
        assert_eq!(shrunk.len(), 1, "expected a single leaf, got {shrunk:?}");
        assert!(matches!(shrunk[0], Inst::Set(..)), "got {shrunk:?}");
    }
}

/// Generated programs are always accepted by the verifier (generator validity
/// is the contract the properties rely on).
#[test]
fn generator_never_produces_violations() {
    let mut rng = Xs(0xDA7A_0001);
    for _ in 0..500 {
        let nregs: u8 = 4 + rng.below(12) as u8;
        let len = 1 + rng.below(20) as usize;
        let nodes = random_prog(&mut rng, nregs, len);
        let rep = check::check(&nodes);
        assert!(
            rep.reversible(),
            "generator produced violations: {:?}",
            rep.violations
        );
        // and every address used is in range: forward must not fail
        let mut s = gen_state(&mut rng, nregs);
        run_forward(&mut s, &nodes)
            .unwrap_or_else(|e| panic!("generator produced an out-of-range program: {e}"));
    }
}
