//! Proof Explorer (MVP, CLI): reversibility analysis of a program.
//!
//! Audit deliverable: for a parsed program answer three questions —
//! 1. which leaf ranges are reversible (maximal reversible regions);
//! 2. where the boundaries are (`set`/`mset`) and which leaves violate an
//!    alias rule;
//! 3. WHY a non-invertible leaf cannot be inverted, with a *minimal
//!    counterexample*: two distinct states that produce the same output
//!    (a concrete non-injectivity witness), found by bounded search.
//!
//! Classification reuses the verifier rules (`Inst::operand_alias`,
//! `Inst::is_irreversible`) so `explore` can never disagree with `check`.

use std::collections::HashMap;

use crate::inst::Inst;
use crate::machine::{count_leaves, reversible_suffix_len};
use crate::program::Node;
use crate::state::State;

/// Why a leaf cannot be inverted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    /// `set`/`mset`: the overwritten value has no surviving source (erasure).
    Boundary,
    /// Operand aliasing: the leaf's map is not injective on the state space.
    Alias,
}

impl IssueKind {
    pub fn label(self) -> &'static str {
        match self {
            IssueKind::Boundary => "boundary",
            IssueKind::Alias => "aliasing",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Issue {
    /// Index of the leaf in the expanded stream (0-based).
    pub leaf: u64,
    pub kind: IssueKind,
    /// Verifier's reason (`operand_alias()` text; for boundaries a fixed text).
    pub reason: String,
}

/// A maximal run of invertible leaves: `[start, start + len)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub start: u64,
    pub len: u64,
}

#[derive(Debug, Clone)]
pub struct Analysis {
    pub total_leaves: u64,
    pub regions: Vec<Region>,
    pub issues: Vec<Issue>,
    /// Leaves that can be rolled back from the end (`reversible_suffix_len`).
    pub suffix: u64,
    /// True if huge `rep` bodies were not fully expanded (budget).
    pub truncated: bool,
}

/// Default expansion budget for the leaf map.
const DEFAULT_BUDGET: u64 = 1 << 20;

impl Analysis {
    pub fn of(nodes: &[Node]) -> Analysis {
        Self::of_with_budget(nodes, DEFAULT_BUDGET)
    }

    pub fn of_with_budget(nodes: &[Node], max_leaves: u64) -> Analysis {
        let mut w = Walker {
            regions: Vec::new(),
            issues: Vec::new(),
            leaf: 0,
            cur_start: None,
            truncated: false,
            budget: max_leaves,
        };
        w.walk(nodes);
        w.close_region();
        Analysis {
            total_leaves: count_leaves(nodes),
            regions: w.regions,
            issues: w.issues,
            suffix: reversible_suffix_len(nodes),
            truncated: w.truncated,
        }
    }

    /// True when the verifier's rules classify every leaf as invertible.
    pub fn purely_reversible(&self) -> bool {
        self.issues.is_empty()
    }
}

fn boundary_reason(inst: &Inst) -> String {
    match inst {
        Inst::Set(..) => {
            "set: the old register value is overwritten with no surviving source".to_string()
        }
        Inst::MSet(..) => {
            "mset: the old memory word is overwritten with no surviving source".to_string()
        }
        _ => "boundary".to_string(),
    }
}

/// All leaves of the (sub)tree are invertible (no boundary, no alias rule hit).
fn subtree_invertible(nodes: &[Node]) -> bool {
    nodes.iter().all(|n| match n {
        Node::Inst(i) => i.operand_alias().is_none() && !i.is_irreversible(),
        Node::Rep { body, .. } => subtree_invertible(body),
    })
}

struct Walker {
    regions: Vec<Region>,
    issues: Vec<Issue>,
    leaf: u64,
    cur_start: Option<u64>,
    truncated: bool,
    budget: u64,
}

impl Walker {
    fn close_region(&mut self) {
        if let Some(s) = self.cur_start.take() {
            self.regions.push(Region {
                start: s,
                len: self.leaf - s,
            });
        }
    }

    fn step(&mut self, inst: &Inst) {
        if let Some(reason) = inst.operand_alias() {
            self.close_region();
            self.issues.push(Issue {
                leaf: self.leaf,
                kind: IssueKind::Alias,
                reason: reason.to_string(),
            });
        } else if inst.is_irreversible() {
            self.close_region();
            self.issues.push(Issue {
                leaf: self.leaf,
                kind: IssueKind::Boundary,
                reason: boundary_reason(inst),
            });
        } else if self.cur_start.is_none() {
            self.cur_start = Some(self.leaf);
        }
        self.leaf += 1;
    }

    fn walk(&mut self, nodes: &[Node]) {
        for node in nodes {
            match node {
                Node::Inst(i) => self.step(i),
                Node::Rep { count, body } => {
                    let body_leaves = count_leaves(body);
                    let total = count.saturating_mul(body_leaves);
                    if self.leaf.saturating_add(total) <= self.budget {
                        // fully expand
                        for _ in 0..*count {
                            self.walk(body);
                        }
                    } else if subtree_invertible(body) {
                        // the whole rep is one invertible block: no need to unroll
                        if self.cur_start.is_none() {
                            self.cur_start = Some(self.leaf);
                        }
                        self.leaf = self.leaf.saturating_add(total);
                    } else {
                        // issues inside a huge rep: expand the first copy only
                        self.truncated = true;
                        self.close_region();
                        let first = if self.leaf.saturating_add(body_leaves) <= self.budget {
                            self.walk(body);
                            self.close_region();
                            body_leaves
                        } else {
                            0
                        };
                        self.leaf = self.leaf.saturating_add(total.saturating_sub(first));
                    }
                }
            }
        }
    }
}

/// English explanation of why a flagged instruction cannot be inverted.
/// Mirrors the classification of `Inst::operand_alias` / `is_irreversible`
/// (the detection itself always uses those functions; only the wording lives here).
pub fn why_not_invertible(inst: &Inst) -> &'static str {
    match *inst {
        Inst::Set(..) => {
            "boundary (erasure): the instruction overwrites the destination register and the old value is kept nowhere, so the inverse would need information that no longer exists"
        }
        Inst::MSet(..) => {
            "boundary (erasure): the instruction overwrites the memory word and the old value is kept nowhere, so the inverse would need information that no longer exists"
        }
        Inst::Xor(d, s) | Inst::Add(d, s) | Inst::Sub(d, s) if d == s => {
            "information loss via aliasing: with dst == src the map becomes x -> x^x (constant 0) for xor, x -> x-x (constant 0) for sub, or x -> 2x for add — 2x is not injective on Z/2^64 (x and x+2^63 collide)"
        }
        Inst::MulC(_, m) if m & 1 == 0 => {
            "information loss: an even multiplier is not a unit of Z/2^64, x -> m*x is not injective (x and x+2^63 collide)"
        }
        Inst::Toff(c1, c2, t) if t == c1 || t == c2 => {
            "information loss via aliasing: the target XORs itself with a function of itself (x -> x ^ (x & c) = x & !c), masked bits are destroyed"
        }
        Inst::CSwap(c, a, b) if c == a || c == b => {
            "aliasing: the condition register is also a swapped operand — the swap moves the very bit the swap decision reads, two distinct inputs can converge"
        }
        Inst::MAdd(a, v) | Inst::MSub(a, v) | Inst::MXor(a, v) if a == v => {
            "verifier rule (conservative): index register == value register. The executed map reads the address before the write and stays injective, but the model rejects this shape uniformly as a safety restriction (docs/isa.md, section 5)"
        }
        Inst::RAdd(r, a) | Inst::RSub(r, a) if r == a => {
            "information loss via aliasing: the destination is also the address source, the map becomes x -> x +/- M[x]; two addresses with M[x] - M[y] = y - x collide"
        }
        _ => "non-invertible (verifier flag)",
    }
}

/// Bounded witness search: for a flagged instruction find two DISTINCT states
/// with the SAME output under `exec` — a concrete proof of non-injectivity.
///
/// Returns `None` when the bounded search finds no collision. That happens
/// (a) for the conservative `madd/msub/mxor` index==value rule, where the map
/// is in fact injective, and (b) when `nmem` is too small for memory
/// instructions to execute at all.
pub fn collision_witness(inst: &Inst, nregs: usize, nmem: usize) -> Option<Witness> {
    if inst.operand_alias().is_none() && !inst.is_irreversible() {
        return None; // the verifier does not flag it: assumed bijective
    }

    // Memory words available to the witness state (bounded; a collision inside
    // the first cells exists whenever one exists at all).
    let nmem_w = nmem.min(8);

    // Registers taking part in the instruction (deduplicated: with aliasing
    // two operands can be the same cell).
    let mut involved: Vec<u8> = Vec::new();
    let push = |r: u8, out: &mut Vec<u8>| {
        if (r as usize) < nregs && !out.contains(&r) {
            out.push(r);
        }
    };
    match *inst {
        Inst::Nop | Inst::MSet(..) => {}
        Inst::Not(r)
        | Inst::Inc(r)
        | Inst::Dec(r)
        | Inst::RotL(r, _)
        | Inst::RotR(r, _)
        | Inst::MulC(r, _)
        | Inst::Set(r, _) => push(r, &mut involved),
        Inst::Xor(d, s) | Inst::Add(d, s) | Inst::Sub(d, s) => {
            push(d, &mut involved);
            push(s, &mut involved);
        }
        Inst::Swap(a, b) => {
            push(a, &mut involved);
            push(b, &mut involved);
        }
        Inst::Toff(c1, c2, t) => {
            push(c1, &mut involved);
            push(c2, &mut involved);
            push(t, &mut involved);
        }
        Inst::CSwap(c, a, b) => {
            push(c, &mut involved);
            push(a, &mut involved);
            push(b, &mut involved);
        }
        Inst::MAdd(a, v) | Inst::MSub(a, v) | Inst::MXor(a, v) => {
            push(a, &mut involved);
            push(v, &mut involved);
        }
        Inst::RAdd(r, a) | Inst::RSub(r, a) => {
            push(a, &mut involved);
            push(r, &mut involved);
        }
    }
    // Registers that hold a MEMORY ADDRESS are constrained to valid addresses;
    // pure value registers range over a small set containing the collision
    // pairs (0 vs 2^63 for x -> 2x; 1 vs 1+2^63 for even multipliers; big
    // values for x -> x - M[x] with M[x] = -1).
    const VALS: [u64; 7] = [0, 1, 2, 3, u64::MAX, 1 << 63, (1 << 63) + 1];
    let addr_regs: Vec<u8> = match *inst {
        Inst::MAdd(a, _)
        | Inst::MSub(a, _)
        | Inst::MXor(a, _)
        | Inst::RAdd(_, a)
        | Inst::RSub(_, a) => vec![a],
        _ => Vec::new(),
    };
    let addr_vals: Vec<u64> = (0..nmem_w as u64).collect();
    if involved.iter().any(|r| addr_regs.contains(r)) && addr_vals.is_empty() {
        return None; // memory instruction but no memory: nothing can execute
    }
    let cands: Vec<Vec<u64>> = involved
        .iter()
        .map(|r| {
            if addr_regs.contains(r) {
                addr_vals.clone()
            } else {
                VALS.to_vec()
            }
        })
        .collect();

    // Memory variants: all-zero, or exactly one non-zero cell (cells that can
    // be addressed, plus the mset target when it is an immediate).
    let mut mem_vars: Vec<Option<(usize, u64)>> = vec![None];
    let mut cells: Vec<usize> = (0..nmem_w).collect();
    if let Inst::MSet(addr, _) = *inst {
        if (addr as usize) < nmem {
            cells.push(addr as usize);
        }
    }
    const MEM_VALS: [u64; 6] = [1, 2, 3, u64::MAX, 1 << 63, (1 << 63) + 1];
    for &c in &cells {
        for &v in &MEM_VALS {
            mem_vars.push(Some((c, v)));
        }
    }

    let wr = nregs.max(involved.iter().map(|&r| r as usize + 1).max().unwrap_or(0));
    let mut seen: HashMap<Vec<u64>, State> = HashMap::new();
    let mut idx = vec![0usize; involved.len()];
    loop {
        for mv in &mem_vars {
            let mut s1 = State::new(wr, nmem_w);
            for (p, &r) in involved.iter().enumerate() {
                s1.regs[r as usize] = cands[p][idx[p]];
            }
            if let Some((c, v)) = mv {
                s1.mem[*c] = *v;
            }
            let mut out = s1.clone();
            if inst.exec(&mut out).is_err() {
                continue; // out-of-domain candidate (should not happen, be safe)
            }
            let mut key = out.regs.clone();
            key.extend_from_slice(&out.mem);
            match seen.get(&key) {
                Some(prev) if *prev != s1 => {
                    return Some(Witness {
                        inst: *inst,
                        s1: prev.clone(),
                        s2: s1,
                        out,
                    });
                }
                _ => {
                    seen.insert(key, s1);
                }
            }
        }
        // odometer step over register candidate combinations
        let mut advanced = false;
        for p in (0..idx.len()).rev() {
            if idx[p] + 1 < cands[p].len() {
                idx[p] += 1;
                advanced = true;
                break;
            }
            idx[p] = 0;
        }
        if !advanced {
            break;
        }
    }
    None
}

/// A concrete non-injectivity witness: `s1 != s2`, `exec(s1) == exec(s2) == out`.
#[derive(Debug, Clone)]
pub struct Witness {
    pub inst: Inst,
    pub s1: State,
    pub s2: State,
    pub out: State,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::leaf_at;
    use crate::program::parse;

    fn prog(src: &str, nregs: usize) -> Vec<Node> {
        parse(src, nregs).unwrap().nodes
    }

    /// A witness is valid iff the two inputs differ and both map to `out`.
    fn witness_valid(w: &Witness) -> bool {
        if w.s1 == w.s2 {
            return false;
        }
        let mut a = w.s1.clone();
        let mut b = w.s2.clone();
        w.inst.exec(&mut a).is_ok() && w.inst.exec(&mut b).is_ok() && a == w.out && b == w.out
    }

    #[test]
    fn pure_program_is_one_region() {
        let nodes = prog("inc r0\nxor r1 r0\nrep 3\nadd r2 r0\nend\nswp r0 r1\n", 8);
        let a = Analysis::of(&nodes);
        assert_eq!(a.total_leaves, 6);
        assert!(a.purely_reversible());
        assert_eq!(a.regions, vec![Region { start: 0, len: 6 }]);
        assert_eq!(a.suffix, 6);
    }

    #[test]
    fn boundary_splits_regions() {
        let nodes = prog("inc r0\nset r1 5\ninc r0\n", 8);
        let a = Analysis::of(&nodes);
        assert_eq!(
            a.regions,
            vec![Region { start: 0, len: 1 }, Region { start: 2, len: 1 }]
        );
        assert_eq!(a.issues.len(), 1);
        assert_eq!(a.issues[0].kind, IssueKind::Boundary);
        assert_eq!(a.issues[0].leaf, 1);
        assert_eq!(a.suffix, 1);
    }

    #[test]
    fn alias_is_flagged_with_witness() {
        let nodes = prog("xor r0 r0\n", 8);
        let a = Analysis::of(&nodes);
        assert_eq!(a.issues[0].kind, IssueKind::Alias);
        let inst = leaf_at(&nodes, a.issues[0].leaf).unwrap();
        let w = collision_witness(&inst, 8, 0).unwrap();
        assert!(witness_valid(&w));
        assert_eq!(w.out.regs[0], 0);
    }

    #[test]
    fn witness_for_each_violation_family() {
        // add dst==src: x -> 2x collides via 2^63
        let w = collision_witness(&Inst::Add(0, 0), 8, 0).unwrap();
        assert!(witness_valid(&w));
        // even multiplier
        let w = collision_witness(&Inst::MulC(1, 4), 8, 0).unwrap();
        assert!(witness_valid(&w));
        // toff target aliases control
        let w = collision_witness(&Inst::Toff(0, 1, 0), 8, 0).unwrap();
        assert!(witness_valid(&w));
        // cswp control aliases operand
        let w = collision_witness(&Inst::CSwap(0, 0, 1), 8, 0).unwrap();
        assert!(witness_valid(&w));
        // set is a boundary with a trivial witness
        let w = collision_witness(&Inst::Set(2, 7), 8, 0).unwrap();
        assert!(witness_valid(&w));
        assert_eq!(w.out.regs[2], 7);
        // mset likewise (memory)
        let w = collision_witness(&Inst::MSet(3, 9), 8, 8).unwrap();
        assert!(witness_valid(&w));
        assert_eq!(w.out.mem[3], 9);
        // rmadd with reg==index: x -> x + M[x] collapses
        let w = collision_witness(&Inst::RAdd(0, 0), 8, 8).unwrap();
        assert!(witness_valid(&w));
        let w = collision_witness(&Inst::RSub(0, 0), 8, 8).unwrap();
        assert!(witness_valid(&w));
        // madd with index==value: verifier flag is conservative -> no collision
        assert!(collision_witness(&Inst::MAdd(0, 0), 8, 8).is_none());
        // unflagged instruction: never searched
        assert!(collision_witness(&Inst::Add(0, 1), 8, 0).is_none());
        // memory instruction without memory: cannot execute -> no witness
        assert!(collision_witness(&Inst::RAdd(0, 0), 8, 0).is_none());
    }

    #[test]
    fn rep_merges_and_huge_rep_does_not_unroll() {
        let nodes = prog("rep 1000000\ninc r0\nend\n", 8);
        let a = Analysis::of(&nodes);
        assert_eq!(a.total_leaves, 1_000_000);
        assert_eq!(
            a.regions,
            vec![Region {
                start: 0,
                len: 1_000_000
            }]
        );
        assert!(!a.truncated);
        // issues inside a huge rep: first copy expanded, rest skipped
        let nodes = prog("rep 1000000\ninc r0\nset r1 1\nend\n", 8);
        let a = Analysis::of(&nodes);
        assert!(a.truncated);
        assert_eq!(a.issues.len(), 1);
        assert_eq!(a.issues[0].leaf, 1);
    }

    #[test]
    fn mset_boundary_in_ledger_layout() {
        let nodes = prog("mset 1 1000\nmadd r0 r1\n", 4);
        let a = Analysis::of(&nodes);
        assert_eq!(a.issues.len(), 1);
        assert_eq!(a.issues[0].kind, IssueKind::Boundary);
        assert_eq!(a.regions, vec![Region { start: 1, len: 1 }]);
        assert_eq!(a.suffix, 1);
    }
}
