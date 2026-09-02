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
