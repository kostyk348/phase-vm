# ⬡ phase-vm

**A reversible register machine.** Every instruction modifies exactly one
destination; all sources stay intact and are sufficient to invert it. So the
inverse is always computable from the *current* state — **no logs, no
checkpoints, zero extra memory**. Running a program `--reverse` unwinds it to
the last boundary, bit-for-bit.

```text
cargo test        # 18 tests, incl. property fuzz F⁻¹(F(S)) == S
```

[![CI](https://img.shields.io/github/actions/workflow/status/kostyk348/phase-vm/ci.yml?branch=main&label=CI)](https://github.com/kostyk348/phase-vm/actions)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.90-orange.svg)]()
[![deps](https://img.shields.io/badge/dependencies-0-green.svg)]()
[![release](https://img.shields.io/github/v/release/kostyk348/phase-vm?include_prereleases&label=release)]()

---

## Why

Manifesto-grade pitch says: *"a symplectic VM where every instruction is a
phase rotation and any program is reversible to machine epsilon, without a
single byte of logs."* That is not how IEEE-754 or destructive stores work.
This crate is the **honest core** of that idea:

| claim | reality implemented here |
|---|---|
| reversible to machine epsilon | reversible **on bits/integers** (`u64`); floats are not invertible and are excluded by design |
| no logs at all | no logs **inside a phase** — sources of an instruction *are* the log; erasure (`set`/`mset`) is the one **boundary**, paid once at input |
| free time travel | `rev --steps K`, `runrev`, interactive `dbg` with `f`/`b` — backward stepping needs no recording |

## The invariant

1. State = register file + linear word memory (`u64`).
2. Every instruction writes **one** destination; all read operands stay intact
   and are sufficient to invert → `inverse()` exists for every non-boundary op.
3. `set` / `mset` destroy a value with no source → **boundary**. A verifier
   (`check`) classifies every leaf as invertible / boundary / aliasing-violation
   (`dst == src`, `target == control` are not bijective).
4. `rep N … end` is a static sweep loop: a bijection repeated N times; the
   reverse is N passes of the inverse body. Expanded lazily by a job stack —
   million-iteration loops allocate nothing.

Bennett-style **clean computation** falls out: compute into a fresh cell,
copy the result out, uncompute temporaries by running their inverse.

## Quickstart

```bash
cargo build --release
B=./target/release/phase-vm

# verify + property fuzz a purely reversible kernel
$B check  programs/mixer.phase            # "ЧИСТО обратима"
$B roundtrip programs/mixer.phase --trials 5000   # F⁻¹(F(S)) == S

# clean compute: c = a + b, temporaries uncomputed to zero
$B run programs/clean.phase --set r0=5 --set r1=7     # → r2=0xc, temp 0
$B rev programs/clean.phase --set r0=5 --set r1=7     # → kernel unwound, input intact

# transactional memory rollback (#11-flavoured): A→B transfer, abort = reverse
$B run programs/ledger.phase --mem 8      # A=993 B=507
$B rev programs/ledger.phase --mem 8      # abort → A=1000 B=500

# PC1 cipher (#12-flavoured): decrypt IS the backward run — no separate algorithm
$B cipher --key 000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f \
          --pt  0123456789abcdeffedcba987654321000112233445566778899aabbccddeeff

# interactive time-travel debugger (#9-flavoured): f/b/g/c/br/p/pm
$B dbg programs/mixer.phase
```

## CLI

| command | purpose |
|---|---|
| `check` | classify leaves: invertible / boundary / aliasing; verdict "чисто обратима" |
| `run` | forward execution |
| `runrev` | backward execution **from a given state** (no forward first) |
| `rev [--steps K]` | forward, then unwind K leaves (or the whole reversible suffix) |
| `roundtrip` | property fuzz: random states → `F⁻¹(F(S)) == S` |
| `dbg` | interactive debugger: `f`/`b` step, `g N` goto, `c` continue, `br/bl/bd`, `p`, `pm` |
| `bench` | ns/leaf forward vs backward vs full-state snapshot |
| `cipher` | PC1 demo: encrypt = forward, decrypt = backward |

Text format: one instruction per line, `#` comments, registers `r0..rN`,
immediates as decimal / `0x…` / `0b…` / negative.

**ISA:** `nop not inc dec xor add sub swp rotl rotr toff cswp` (reversible) ·
`madd msub mxor rmadd rmsub` (word memory) · `set mset` (boundaries).

## Measured (release, i7-class)

| | cost | extra memory |
|---|---|---|
| reverse | **~6.8 ns/leaf, flat** for state 64 B → 1 MB | **0 B** |
| forward | ~5.5 ns/leaf | 0 B |
| snapshot (CoW) | 7.8 ns @ 64 B → 1.9 µs @ 64 KB → **25 µs @ 1 MB** | full copy |

**Rollback costs O(steps taken), not O(bytes of state).** That is the
measurable heart of the reversible paradigm — the property later projects
(transactional stores, WAL-less in-memory DB, reverse debugging) build on.

PC1: 12-round ARX kernel, 256-bit block/key. Avalanche measured at
~0.5 bit-flip rate; instruction stream contains **no data-dependent branches**
(no `cswp`), so no data-dependent timing at the code level.

## Application: transactional batches with free rollback

`examples/batch_tx.rs` — the pattern behind WAL-less in-memory transactions
(`#11`) and speculative DSO ticks: the machine stays **purely reversible**,
the host decides commit/abort from invariants checked *after* applying, and
abort is just the backward run.

```bash
cargo run --release --example batch_tx
```

Scenario: 256 accounts, batches of 16 transfers sized at 60–95% of the
sender's balance **at batch-build time** — so a sender used twice inside a
batch causes a cascade overdraft that is invisible until the batch is applied.
Host validates afterwards and aborts the whole batch.

```text
пачек OK=12840  откачено=7160 (35.8%)
leaves: fwd=640000  rev(abort)=229120   (15.5 ns/leaf)
snapshot-подход скопировал бы 39 MiB — здесь 0
```

Rollback of a failed batch: **0 log bytes, 0 state copies**, and the final
state provably equals the commit-only reference simulation (asserted).

## Application: speculative safety-filtered tick control

`examples/spec_control.rs` — a DSO-flavoured tick loop where **a tick is a
phase**: 7 control candidates per tick are *applied* to the state (integer
symplectic Euler `v += u; q += v` — a genuinely reversible phase step on
integers), validated against `|q| ≤ QMAX, |v| ≤ VMAX`, and every rejected
candidate is unwound by the backward run. No state copies between candidates.

```bash
cargo run --release --example spec_control
```

```text
тиков закоммичено=300000  hold=0  возмущений=7500
|q|max=1011 (лимит 3000)  |v|max=60 (лимит 60) — инвариант держался всё время
leaves: fwd=4.8M  rev(отклонённые)=4.2M   (11.8 ns/leaf, 106 ms)
финал: q=-2, v=0
MPC-снапшоты скопировали бы 32 MiB — здесь 0 байт логов/копий
детерминизм: OK (одинаковый seed → одинаковый финал)
```

Zero-trace rollback is proven per candidate (`assert_eq!` state == base after
every unwinding); determinism is proven by double-run. External disturbances
enter as boundaries (inputs, not rolled back) — exactly how real control loops
treat them.

## Application: phase-scoped allocator (`src/alloc.rs`)

What reversibility actually buys an allocator — and what it does not:
bump-allocation is already a reversible op (`malloc` = pointer advance),
so **rolling back a whole phase of allocations to a mark is O(1)**, with zero
per-object headers. Arbitrary-order `free` is *not* reversible without a
journal, so this is honestly scoped to **phase-shaped workloads** (ticks,
transactions, ECS) — object lifetimes end at a phase boundary.

```bash
cargo run --release --example alloc_bench
```

```text
K/фазу      malloc+free  arena+rollback  arena+commit  выигрыш×
8                4.76           1.15           7.93        4.1×
512             14.03           1.08           7.36       12.9×
32768           35.27           1.80           7.56       19.6×
нулевой след отката: OK (offset переиспользован детерминированно)
```

`rollback(mark)` is a single `truncate` — cost does not grow with K, while
malloc+free degrades as K grows. Commit-mode phases pay one `reset` (O(1)) at
the boundary. Zero-trace: after rollback the next allocation deterministically
reuses the same offset (asserted).

## L3: decision journal — data-dependent control flow, still reversible

`src/ctl.rs` (`ifnz rN … end`, `wz rN … end`, `rep N … end`): structured
branching/loops whose *decision log* is **O(decisions), never O(state)** —
an `if` records one flag, a `while` records one trip count. Reverse trusts the
journal (LIFO, post-order), `reverse_checked` additionally re-verifies
conditions. Straight-line leaves stay log-free.

```bash
cargo run --release --example ctl_demo
```

```text
итераций цикла: 500000 (данные! rep статически не смог бы)
журнал: 1 запись = 8 B      (наивный трейс шагов: 8 000 008 B)
forward:  3.7 ns/leaf   reverse: 5.7 ns/leaf — состояние == граница, журнал пуст
```

Data-dependent multiply over a 500k-iteration `while` rolls back with a
single 8-byte decision — the architecture axiom "journal = decisions, never
state" measured.


## P2: AOT — compile kernels to native code (`src/aot.rs`)

Interpreter ~3.7–16 ns/leaf; generated C (`cc -O2 -shared`) + dlopen:

```bash
cargo run --release --example aot_bench
# forward : native 0.06 ns/leaf | interpreter 6.08 | 94.8×
# reverse : native 0.09 ns/leaf | interpreter 8.21 | 94.9×
```

`phase_fwd`/`phase_rev` mirror the reversible semantics; native == interpreter
asserted; `rev` requires a purely reversible kernel (boundaries = checkpoint).

## L4: phase capsule in `.eml` (`src/cap.rs`)

State + kernel travel as an RFC-5322 envelope: headers carry the invariants
(kernel-hash, boundary-hash, state-hash), body carries state + program. The
receiving host verifies without trusting the sender: state-hash → reverse →
boundary-hash → re-forward → state-hash.

```bash
cargo run --release --example capsule
# капсула: 612 B · верификация OK · финал после миграции == чистый прогон
```

## P10: wide lanes AVX2 (`examples/wide_bench.rs`)

Reversible primitives (xor/add/rot) are componentwise → vectorizable. Measured
AVX2 vs scalar on reversible xor+rotl passes: correctness asserted
(AVX2 == scalar); on x86 the autovectorizer already closes most of the gap
(~1.3× hand-AVX2) — wide gain becomes real at SoA multi-register kernels.


## The paradigm for typical tasks

Not "where to bolt PHASE into old programs" — what ordinary programming looks
like in the paradigm (`phase-arch/TYPICAL.md` maps 9 typical task classes →
phase form → gain):

- **A/B config selection without copies** — `examples/typical_ab.rs`: apply
  candidate → metric → rollback loser (reverse run, 4 leaves) → commit winner.
  500k rounds, 0 state copies (classic: 32 MB of clones), 17 ns/leaf.
- **Parsing with O(1) error recovery** — `examples/typical_parse.rs`: each
  record parsed into its own Arena mark; semantic failure → `rollback(mark)`.
  54 515 bad records recovered 2.6 MB with 54 515 O(1) rollbacks instead of
  327 090 individual frees.

The pattern for all of them: allocate in a phase, transform reversibly,
**validate at the boundary**, commit or roll back. Failure is a control-flow
branch — for everyday code too.
## Phase math: reversible functions with analytic inverses

`src/pmath.rs` + `examples/math_demo.rs` — ordinary math rewritten for the
paradigm: inside a phase, math = bijections on Z/2^64 whose inverse is
*computed*, not stored: `MulOdd(m)` (odd multiplier — a bijection; inverse via
Newton mod-inverse), `Add`, `Xor`, `RotL`, ARX rounds. Compositions are
bijections, so any formula is undoable exactly (asserted on random round-trips).
Float math stays at the boundary (it is not invertible).

## Axis B: meet-in-the-middle (`examples/mitm_bfs.rs`)

Bidirectional BFS on 200×200 mazes (~25% walls): bi expands **1.6× fewer**
vertices than uni (330k vs 535k across 40 mazes); path lengths match
(asserted). Honest framing: sqrt-style reduction where decomposition exists,
not a universal 2^N/2 promise.
## Application: rollback netcode / server reconciliation

`examples/rollback_netcode.rs` — deterministic authoritative tick where late
client inputs are fixed by **rollback**, never by snapshots. Physics = integer
symplectic Euler (reversible); inputs are boundaries stored as a 1-byte-per-
entity-per-tick log. On a late input that differs from the prediction, the
server reverses to that tick, applies the input, resims deterministically.

```bash
cargo run --release --example rollback_netcode
```

```text
реконсиляций: 142253   окно отката 1..8 тиков (откачено 639715 тиков)
память: журнал входов 240 000 B  против снапшотов мира 3 840 000 B (16×)
корректность: hash-цепочка всех 60 001 тиков == эталон (ground truth)
```

The final timeline provably equals a zero-delay reference run — hash-chain of
every tick boundary matches. This is the N3/N4 von-Neumann-ailment cure:
reconciliation without world snapshots, sealed by the SINT hash-chain audit.

## Honest limits

- Reversible semantics live on bits/integers. IEEE-754 rounding/NaN is not
  invertible; floats stay out of the ISA.
- "No logs" holds *inside a phase*. Reversing **across** a `set`/`mset`
  boundary requires a checkpoint — that is the price of erasure, paid at the
  boundary, once per phase (see `dbg`, which refuses to step back over a
  boundary without one).
- PC1 is a paradigm demonstrator (`decrypt = reverse`), **not** a
  cryptographically audited cipher.
- Static `rep` only — data-dependent control flow (reversible `if`/loops)
  is the next chapter.

## Roadmap / lineage

POC-1 of a broader reversible-computing programme. Clusters:

- **A core** — this crate (`phase-vm`): reversible ISA, boundary model,
  verifier. Seeds: `#9` reverse-step debugger (`dbg`), `#11` rollback
  transactions (`ledger.phase`), `#3/#11` boundary-as-checkpoint semantics.
- **C crypto** — `PC1`: ARX kernel whose decryption is the backward run (`#12`).
- Next: reversible control flow (Janus-style), frame/call model, CA round
  function, constant-weight variants.

## License

MIT — see [LICENSE](LICENSE).
