# phase-vm — formal ISA specification (L1 core)

**Status:** generated from the implementation, not from the manifesto. Every
statement below corresponds to code in `src/inst.rs`, `src/state.rs`,
`src/program.rs`, `src/machine.rs` and `src/lib.rs` (`check` module) as of this
commit. Where the verifier is *stricter* than strict necessity, this is marked
explicitly (see the remark on `madd/msub/mxor` aliasing in §5).

Machine-level control flow (`ifnz` / `wz` / data-dependent loops, the L3
decision journal in `src/ctl.rs`) is out of scope of this document except for
a short summary in §7; the L1 ISA below assumes a static instruction stream.

---

## 1. Machine state

Defined in `src/state.rs`:

```rust
pub struct State {
    pub regs: Vec<u64>,   // register file
    pub mem:  Vec<u64>,   // linear word-addressable memory
}
```

- **Word model:** every cell (register or memory word) is a `u64`, i.e. an
  element of the ring **Z/2^64**. All arithmetic is wrapping (`wrapping_add`,
  `wrapping_sub`, `wrapping_mul`). There are no flags, no carry, no saturation.
- **Registers:** `r0 .. r(nregs-1)`. `nregs` is fixed per program at parse
  time (`--regs N`, default 16) and is a *parse-time* bound: an instruction
  referencing `rN` with `N >= nregs` is rejected by the parser
  (`program::parse_reg`), before execution.
- **Memory:** `m[0] .. m(nmem-1)`, word-addressed. `nmem` is a *runtime*
  allocation (`--mem N`, default 0): the address operand is checked at
  execution time and out-of-range addresses abort with an error (§6).
- **No program counter in the state.** A program is a static instruction tree
  (§4); the machine state itself contains only `regs` and `mem`.

## 2. The core invariant

1. Every instruction modifies **exactly one destination** — one register or
   one memory word. All source operands are left intact.
2. Because the sources survive, the pre-state of the destination is
   recoverable from the post-state *plus the sources that are still there*.
   Formally: on its domain of applicability (§5) each reversible instruction
   `f` is a **bijection** on `State`, and `Inst::inverse()` (src/inst.rs)
   returns a concrete instruction computing `f⁻¹`. The inverse is computed
   from the *current* state — no undo log, no snapshot, no checkpoint.
3. Wrapping does **not** break bijectivity. On Z/2^64, `x ↦ x + a`,
   `x ↦ x − a`, `x ↦ x ⊕ a`, `x ↦ rotl(x,k)`, `x ↦ m·x` (m odd) are all
   bijections; "overflow" is just modular arithmetic, and the modular inverse
   exists. What *does* break bijectivity is (a) **erasure** — overwriting a
   value that has no surviving source (`set`/`mset`), and (b) **operand
   aliasing** — making a source and the destination the same cell, so the
   map stops being injective (§5).
4. The only information that is ever destroyed in L1 is destroyed at
   `set`/`mset`. These are the **boundaries** (§8).

## 3. Instruction reference

Syntax follows `program::parse_inst`; the text format is one instruction per
line, `#` comments, immediates in decimal / `0x…` / `0b…` / negative
(negative `imm` is stored as its two's-complement `u64`: `-1 → 0xFFFF…FF`,
`program::parse_imm`). Mnemonics are case-insensitive.

Notation: `R[i]` = register `i`, `M[i]` = memory word `i`, `+ − ⊗ ·` are
wrapping operations on Z/2^64, `rotl(x,k) = (x ≪ k) | (x ≫ (64−k))` on 64 bits.
`Pre` is the precondition checked by the *verifier* (and respected by the
generators); `exec` itself performs only the range checks of §6.

| # | Mnemonic | Syntax | Semantics (forward) | Pre | Inverse instruction |
|---|----------|--------|---------------------|-----|---------------------|
| 1 | `nop`  | `nop` | nothing | — | `nop` |
| 2 | `not`  | `not r` | `R[r] := ¬R[r]` | — | `not r` (self-inverse) |
| 3 | `inc`  | `inc r` | `R[r] := R[r] + 1` | — | `dec r` |
| 4 | `dec`  | `dec r` | `R[r] := R[r] − 1` | — | `inc r` |
| 5 | `xor`  | `xor d s` | `R[d] := R[d] ⊕ R[s]` | `d ≠ s` | `xor d s` (self-inverse) |
| 6 | `add`  | `add d s` | `R[d] := R[d] + R[s]` | `d ≠ s` | `sub d s` |
| 7 | `sub`  | `sub d s` | `R[d] := R[d] − R[s]` | `d ≠ s` | `add d s` |
| 8 | `swp`  | `swp a b` | swap `R[a] ↔ R[b]` | — | `swp a b` (self-inverse; `a = b` is the identity) |
| 9 | `rotl` | `rotl r k` | `R[r] := rotl(R[r], k′)` | — | `rotr r k` |
| 10 | `rotr` | `rotr r k` | `R[r] := rotr(R[r], k′)` | — | `rotl r k` |
| 11 | `mulc` | `mulc r m` | `R[r] := R[r] · m` | `m` **odd** | `mulc r m⁻¹` (§3.1) |
| 12 | `toff` | `toff c1 c2 t` | `R[t] := R[t] ⊕ (R[c1] ∧ R[c2])` | `t ∉ {c1, c2}` | `toff c1 c2 t` (self-inverse) |
| 13 | `cswp` | `cswp c a b` | if bit0(`R[c]`) = 1: swap `R[a] ↔ R[b]` | `c ∉ {a, b}` | `cswp c a b` (self-inverse) |
| 14 | `set`  | `set r imm` | `R[r] := imm` | **boundary** | — (`inverse() = None`) |
| 15 | `madd` | `madd a v` | `M[R[a]] := M[R[a]] + R[v]` | `a ≠ v`, `R[a] < nmem` | `msub a v` |
| 16 | `msub` | `msub a v` | `M[R[a]] := M[R[a]] − R[v]` | `a ≠ v`, `R[a] < nmem` | `madd a v` |
| 17 | `mxor` | `mxor a v` | `M[R[a]] := M[R[a]] ⊕ R[v]` | `a ≠ v`, `R[a] < nmem` | `mxor a v` (self-inverse) |
| 18 | `rmadd`| `rmadd r a` | `R[r] := R[r] + M[R[a]]` | `r ≠ a`, `R[a] < nmem` | `rmsub r a` |
| 19 | `rmsub`| `rmsub r a` | `R[r] := R[r] − M[R[a]]` | `r ≠ a`, `R[a] < nmem` | `rmadd r a` |
| 20 | `mset` | `mset addr imm` | `M[addr] := imm` | **boundary**, `addr < nmem` | — (`inverse() = None`) |

Notes on specific rows:

- **Rotate amount (rows 9–10).** `k` is parsed as a `u64` immediate and
  **truncated to `u32`**, then masked: the effective amount is
  `k′ = (k as u32) & 63` (`Inst::exec`, `rotate_left(k & 63)`). Since forward
  and inverse mask the same way, `rotr(r, k′) ∘ rotl(r, k′) = id` for any `k`.
  `rotl r 0` is the identity.
- **Conditional swap (row 13).** `cswp c a b` is self-inverse *because* the
  condition bit is not touched by the swap (guaranteed by `c ∉ {a, b}`):
  applying it twice swaps twice and the condition evaluates identically.
- **Toffoli (row 12).** `R[t] ⊕= R[c1] ∧ R[c2]` is the AND controlled onto the
  target by XOR; XOR is involutive and the controls are untouched, so the same
  instruction inverts it.
- **Memory addressing.** In rows 15–19 the address is the *value* of a
  register (`M[R[a]]`), read before any write; the register file is never
  modified by rows 15–17, and `R[a]` itself is not the destination of
  rows 18–19 (enforced by `r ≠ a`).

### 3.1 `mulc`: the modular inverse

`mulc r m` multiplies by a constant on Z/2^64. Multiplication by `m` is a
bijection **iff `m` is odd** (a unit of the ring: `gcd(m, 2^64) = 1`). The
inverse is `mulc r m⁻¹` where `m⁻¹ = mod_inv_odd(m)` (`src/pmath.rs`) is
computed by Newton iteration `x ← x·(2 − m·x)`, 6 doubling steps starting from
`x₀ = m`: `x₀` is correct mod 2, and each step doubles the number of correct
bits, so `m · mod_inv_odd(m) ≡ 1 (mod 2^64)`. `Inst::inverse()` computes `m⁻¹`
at spec time, so no state is needed to invert a multiplication.
An even constant is rejected by the verifier (`operand_alias()` flags
`mulc` with `m & 1 == 0`) — an even `m` maps two values differing by `2^63`
to the same product.

## 4. Programs, leaf stream, rollback

Defined in `src/program.rs` / `src/machine.rs`.

- A program is a tree: `Node::Inst(Inst) | Node::Rep { count: u64, body }`.
  `rep N … end` is a **static sweep loop** — the body (itself a bijection
  sequence) repeated `N` times; `N = 0` is allowed (no-op). There is no
  data-dependent control flow in L1; the leaf stream is fully determined by
  the program text.
- The **expanded leaf stream** is what executes. `count_leaves` computes its
  length (saturating); `iter_forward` / `iter_reverse` stream the leaves
  forward / backward, expanding `Rep` lazily with a job stack, so
  million-iteration loops allocate nothing.
- **Forward run** (`run_forward`): apply each leaf's `exec` in order.
- **Rollback** (`reverse_n` / `reverse_all`): walk the leaf stream *backward*
  and apply `inverse()` of each leaf, **stopping** at the first leaf (from the
  end) that has no inverse or violates an alias rule (`invertible_for_rollback
  = inverse().is_some() && operand_alias().is_none()`). The number of leaves
  that can be undone this way is the **reversible suffix**
  (`reversible_suffix_len`). Undoing K ≤ suffix leaves lands exactly on the
  state after the K-th leaf from the end.
- Because each inverse needs only the *current* state, rollback costs
  O(steps taken), not O(bytes of state), and uses 0 log bytes inside a
  reversible region. Reversing *across* a boundary (§8) is impossible without
  a checkpoint — that is the price of erasure.

## 5. Invertibility table

Operation → domain of applicability (precondition) → reverse operation →
possible violations. This is the table the verifier (`check::check`) implements:
a leaf is **invertible** iff `operand_alias() = None` and `!is_irreversible()`;
otherwise it is reported as `ViolationKind::Alias` (alias rule hit, checked
first) or `ViolationKind::Boundary` (`set`/`mset`).

| Operation | Domain of applicability (bijection holds) | Reverse | Violations that break invertibility |
|---|---|---|---|
| `nop`, `not`, `inc`, `dec` | always | same / `dec`/`inc` | none |
| `xor d s` | `d ≠ s` | `xor d s` | `d = s`: `R[d] ↦ R[d] ⊕ R[d] = 0` — collapses everything |
| `add d s` | `d ≠ s` | `sub d s` | `d = s`: `x ↦ 2x` is not injective on Z/2^64 (`x` and `x + 2^63` collide) |
| `sub d s` | `d ≠ s` | `add d s` | `d = s`: `x ↦ 0` |
| `swp a b` | always (`a = b` = identity) | `swp a b` | none |
| `rotl r k` / `rotr r k` | always (amount masked to `k & 63`) | `rotr`/`rotl` | none |
| `mulc r m` | `m` odd | `mulc r m⁻¹` | even `m`: `x ↦ m·x` is not injective (`x` and `x + 2^63` collide) |
| `toff c1 c2 t` | `t ∉ {c1, c2}` | `toff c1 c2 t` | `t = c1` (or `c2`): `x ↦ x ⊕ (x ∧ c) = x ∧ ¬c` — bits masked away are lost |
| `cswp c a b` | `c ∉ {a, b}` | `cswp c a b` | `c ∈ {a, b}`: the swap moves the condition register itself; two inputs can converge |
| `set r imm` | **never invertible** — boundary | — | erasure: the old `R[r]` has no surviving source |
| `madd a v` / `msub a v` / `mxor a v` | `a ≠ v` (verifier rule), `R[a] < nmem` | `msub`/`madd`/`mxor` | see remark below |
| `rmadd r a` / `rmsub r a` | `r ≠ a`, `R[a] < nmem` | `rmsub`/`rmadd` | `r = a`: `x ↦ x ± M[x]`; two addresses `x, y` with `M[x] − M[y] = y − x` collide |
| `mset addr imm` | **never invertible** — boundary | — | erasure: the old `M[addr]` has no surviving source |

**Remark (implemented alias rule is conservative for `madd/msub/mxor`).**
For `madd a v` with `a = v` the executed map is `M[R[a]] += R[a]`; the address
register is read before the write and never modified, so a strict injectivity
argument says the map *is* bijective. Nevertheless the verifier rejects
`a = v` uniformly (`operand_alias()`, reason "index == value"), and the
rollback machinery refuses such leaves. This spec documents the **implemented**
rule; the strictness is a deliberate simplification, kept visible rather than
silent. For `rmadd/rmsub` (`r = a`) the restriction is *necessary*, not
conservative — the destination register is also the address source, and the
map genuinely collapses states.

**Domain violations vs. non-bijectivity.** A register index beyond `nregs` is
a parse-time error; a memory address beyond `nmem` is a *runtime* error
(§6). These are failures of the precondition `R[a] < nmem`, not
non-bijectivity: on its domain the instruction stays a bijection.

## 6. Runtime errors

`Inst::exec` returns `Err` (and the run aborts) when:

- a register operand `r` satisfies `r as usize >= state.regs.len()` —
  unreachable through the text parser (§1) but possible via the library API;
- a memory address (the value `R[a]` for rows 15–19, the immediate `addr` for
  `mset`) satisfies `addr as usize >= state.mem.len()` — e.g. running a
  memory program with `--mem 0`.

There is no silent wraparound of addresses and no partial effect: the failing
leaf reports its index and text (`machine::run_forward`), and the state keeps
all effects of the leaves before it.

## 7. Beyond L1 (pointers, not spec)

- **L3 decision journal** (`src/ctl.rs`): `ifnz rN … end` (execute body iff
  `R[N] ≠ 0`; journal = 1 flag) and `wz rN … end` (repeat body while
  `R[N] ≠ 0`; journal = 1 trip count, capped at 2^24 iterations). Reverse
  replays the journal LIFO (post-order) and applies inverse leaves;
  `reverse_checked` additionally re-verifies the recorded conditions against
  the recovered state. The journal is O(decisions), never O(state); a
  *static* `rep` needs no journal entry at all. Fuzzed round-trips for this
  layer: `tests/ctl_fuzz.rs`.
- **`pmath::Op`** (`src/pmath.rs`): the same bijection idea as standalone
  functions with analytic inverses (`MulOdd`, `Add`, `Xor`, `RotL`, ARX
  rounds) — the mathematical core the ISA rows 5–11 are built on.
- **PC1 cipher** (`src/cipher.rs`): an ARX kernel with no data-dependent
  branches, where decryption *is* the backward run. A paradigm demonstrator,
  **not** a cryptographically audited cipher.

## 8. Boundary semantics

The audit vocabulary (reversible region / boundary input / commit boundary /
externally observable effects) mapped to what the code does:

- **Reversible region.** A maximal run of consecutive leaves in the expanded
  stream in which every leaf is invertible (no boundary, no alias violation).
  Inside a region, `reverse_n`/`reverse_all` reconstruct any earlier state of
  that region bit-for-bit with zero log bytes and zero state copies. The
  region structure of a program is reported by `phase-vm check` and by
  `phase-vm explore` (`src/explore.rs`).
- **Boundary input.** `set` / `mset` are how external data *enters* the
  machine: each erases the previous content of its destination, so it is the
  one point where information is paid for. A boundary is a one-way door:
  rollback stops at it (`reversible_suffix_len` counts only the leaves after
  the last boundary). Input data that must survive a rollback therefore lives
  in cells the kernel never overwrites (see `programs/ledger.phase`,
  `examples/batch_tx.rs`).
- **Commit boundary.** The end of a phase is where the host decides:
  *commit* = keep the state, *abort* = run the inverse of the whole
  reversible region (Bennett-style uncompute). The core exposes the two
  halves (`run_forward`, `reverse_all`); the decision policy is host-side
  (`examples/batch_tx.rs`, `examples/spec_control.rs`). State hashes
  (`State::hash`, `State::chain_step` — FNV-1a) can seal the decision into an
  audit chain. Crossing a boundary backwards requires a checkpoint taken at
  the boundary — the interactive debugger (`dbg`) refuses to step back over
  one without it.
- **Externally observable effects.** The L1 ISA has no I/O: nothing outside
  `State` changes while leaves execute. All observation happens at
  boundaries, by the host (printing state, hashing, copying it out). By
  construction, anything externally observable is therefore boundary-priced:
  it is either an input (erasure at `set`/`mset`) or a host-side read at a
  commit boundary. This is the precise content of the "zero log" claim:
  **zero log inside a reversible region** — boundaries and external effects
  require storing the data they erase or expose.

## 9. What the tests verify (traceability)

| Claim | Where it is proven |
|---|---|
| per-instruction `inverse(exec(S)) == S`, every opcode, random states | `tests/reversibility_props.rs` (generated per-opcode programs) |
| `inverse(inverse(i)) == i` for every invertible instruction | `tests/reversibility_props.rs` |
| random *valid* programs (all invertible opcodes, `rep` nesting, memory): `F⁻¹(F(S)) == S` bit-exact | `tests/reversibility_props.rs`, `tests/roundtrip.rs` |
| rollback of K leaves == naive inverse application of the last K leaves | `tests/roundtrip.rs` (`partial_reverse_matches_naive_undo`) |
| a wrong inverse is caught and shrunk to a minimal counterexample | `tests/reversibility_props.rs` (hand-rolled shrinker) |
| boundary stops the suffix; `mset` behaves like `set` | `tests/roundtrip.rs` (`mset_is_boundary`), `tests/roundtrip.rs::boundary…` and `src/machine.rs` tests |
| even `mulc`, aliasing leaves are rejected by the verifier | `tests/roundtrip.rs` (`mulc_even_rejected_by_checker`), `src/lib.rs::check` |
| L3 journal round-trips (data-dependent control flow) | `tests/ctl_fuzz.rs` |
| memory (`madd/msub/mxor/rmadd/rmsub`) round-trips including ledger scenario | `tests/roundtrip.rs` |
| CLI end-to-end round-trip fuzz on a file | `phase-vm roundtrip <file>` |

## 10. Honest limits

- Integer-level reversibility only. IEEE-754 rounding, NaN, and float
  comparison are not invertible; **floats are not in the ISA** and this
  document does not claim they are.
- "No logs" is a statement about reversible regions. Across boundaries you
  need checkpoints; externally observable effects are boundary-priced (§8).
- Integer reversibility is **not** reversible physics and **not**
  cryptography: a bijective map on bit states preserves no physical
  Hamiltonian, and `decrypt = reverse` does not make PC1 secure (§7).
- `rep` is static. Data-dependent control flow exists only in the separate L3
  layer with its decision journal (§7); L1 programs are straight-line trees.
- The alias rule for `madd/msub/mxor` with `a = v` is stricter than strict
  necessity (§5, remark); for `rmadd/rmsub` with `r = a` it is necessary.
- Memory addresses are checked, not wrapped (§6): a program is only reversible
  on states where every address it computes is in range.
