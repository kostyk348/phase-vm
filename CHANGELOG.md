# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `docs/isa.md` — formal ISA specification written from the implementation:
  machine state (`State { regs, mem }` on Z/2^64), per-instruction
  pre/postconditions and `inverse` functions, the invertibility table
  (operation → domain of applicability → reverse operation → possible
  violations), boundary semantics (reversible region / boundary input /
  commit boundary / externally observable effects), a claim-to-test
  traceability matrix and the honest limits.
- Proof Explorer MVP (CLI + core): `phase-vm explore <program.phase>` and
  `src/explore.rs` — maximal reversible regions of the expanded leaf stream,
  boundary/aliasing leaves, a "why cannot be inverted" explanation per
  flagged leaf (information loss via erasure or aliasing; wrapping overflow
  is *not* a failure mode on Z/2^64), and a bounded search for a minimal
  counterexample: two distinct states producing the same output (a concrete
  non-injectivity witness). The conservative `madd/msub/mxor` index==value
  rule is reported as such when no witness exists.
- `tests/reversibility_props.rs` — property-based tests with zero new
  dependencies: generator of random *valid* programs over all invertible
  opcodes (registers, word memory, odd `mulc`, `toff`, `cswp`, nested
  `rep`), the core property `inverse(forward(S)) == S` per instruction (24
  opcode variants × 150 random states) and per random program fragment,
  partial-rollback equivalence with naive inverse application, and a
  hand-rolled delta-debugging shrinker validated on planted bugs (a wrong
  `add` inverse and a planted `set` boundary both shrink to a single minimal
  leaf).
- `README.ru.md` — full Russian mirror of the README; language switcher on
  top of both files.
- `CHANGELOG.md` (this file).

### Changed

- README: "Honesty / scope" section (integer-level reversibility is not
  reversible physics; no cryptographic claims; every claim is backed by
  tests in this repo), `explore` row in the CLI table, link to
  `docs/isa.md`, refreshed test count (54 → 66).
- CI: `cargo test --workspace`, added a debug `cargo build --workspace`
  step (release test/build unchanged).

### Fixed

- Replaced the unstable `u64::is_multiple_of` with `%` comparisons in
  `src/alloc.rs` and `examples/phase_guard.rs` (builds on rustc 1.83 and on
  newer toolchains alike).
