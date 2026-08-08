# Development

Stage 3 changes should additionally run the four MemoryIR JSONL examples and verify fresh/reuse/guarded outputs agree, the unsafe reuse request is rejected without preventing the following transaction, v6 fixtures retain their pinned bytes, and v7 save/replay reproduces memory IDs/hashes. `cargo test --workspace` includes core atomicity/replay and protocol branch-laziness coverage.

## Repository layout

```text
crates/agentir-core       canonical data and compiler state machine
crates/agentir-eval       CPU reference semantics
crates/agentir-store      versioned archive I/O and replay verification
crates/agentir-protocol   wire requests, responses, workspace registry
crates/agentir-cli        stdin/stdout JSONL process
examples                  reproducible protocol sessions
docs                      project and reference documentation
```

## Quality gate

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo run -p agentir-cli --bin agentir < examples/saxpy.jsonl
cargo run -p agentir-cli --bin agentir < examples/candidate_identity.jsonl
cargo run -p agentir-cli --bin agentir < examples/candidate_rewrite.jsonl
cargo run -p agentir-cli --bin agentir < examples/speculative_open.jsonl
cargo run -p agentir-cli --bin agentir < examples/speculative_promote.jsonl
cargo run -p agentir-cli --bin agentir < examples/guarded_candidate.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_saturate.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_discharge.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_materialize.jsonl
cargo run -p agentir-cli --bin agentir < examples/memory_fresh.jsonl
cargo run -p agentir-cli --bin agentir < examples/memory_reuse.jsonl
cargo run -p agentir-cli --bin agentir < examples/memory_guarded_reuse.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_to_memory.jsonl
cargo run --release -p agentir-protocol --example baseline
```

Tests are split by responsibility: small inference/shape/canonical unit tests in core; workspace, candidate and equality atomicity, revisions, continuations and replay as core integration tests; semantic execution plus the fixed-seed known-rewrite oracle in eval; archive round-trip/tamper handling and pinned corruption fixtures in store; line-level envelopes, persistence commands and full examples in protocol.

## Baseline harness

Run the intentionally dependency-free measurement harness in release mode:

```bash
cargo run --release -p agentir-protocol --example baseline
```

Benchmark schema v2 reports warm-ups plus min/median/p95/max for SpecIR, candidate, proposal/debt/guard, equality creation/saturation/explanation/materialization, rewrite, evidence, migration and replay paths. It records crate/git/dirty/target/OS/rustc/build metadata and separates SpecIR/ImplIR/proposal/candidate/equality/archive byte sizes from timings. See [benchmarking.md](benchmarking.md); timing changes never fail CI.

Golden v1 archives came from commit `97c821a`; v2, v3, v4, v5 and v6 inputs are immutable and pinned by SHA-256. Never regenerate them with a newer writer. V5, v6 and v7 fixture corpora have separate reproducible generators: `cargo run -p agentir-store --example generate_v5_fixtures`, `cargo run -p agentir-store --example generate_v6_fixtures`, and `cargo run -p agentir-store --example generate_v7_fixtures`; review their pinned hashes in the fixture README.

## Adding a known rewrite

Add one stable rule ID, deterministic matcher, exact side-condition checker, staged transform and certificate builder. Cover applicability/non-applicability, atomic rejection, exact evaluator behavior, proof-chain composition, continuation order, archive replay and bounded soundness. Agent-provided proof claims are never accepted as correctness evidence.

The production matcher/transform is shared by direct candidate application, translation validation and equality expansion. Any new exact rule must preserve identical descriptors and side conditions across all three paths, including materialization replay and equality corruption fixtures.

For a new speculative validator path, keep proposal parsing/type verification separate from proof recognition. Reproduce the proposed result through compiler-owned production logic, add exact evidence/debt/frontier replay checks, and cover spoofed provenance, counterexamples, budgets and archive corruption.

## Adding an opcode

Update the canonical `Opcode`, type/shape inference, region policy if relevant, evaluator, continuation domains, protocol examples and tests. Keep result types compiler-owned. If semantics have alternatives, add an explicit attribute or numeric-contract field instead of a hidden default.

## Dependency policy

Dependencies are deliberately small: `serde`, `serde_json` and `sha2`. Prefer the standard library and deterministic collections. GPU/compiler-framework dependencies belong to later stages and should enter behind a new crate boundary.
# Stage 4 checks

Run every `schedule_*.jsonl` example and `equality_to_schedule.jsonl` in addition to the earlier quality gate. A valid scheduled SAXPY result remains `[12.0,24.0,36.0,48.0]`. Stage 4 changes require coverage for atomic rejection, hash independence, target/resource limits, schedule replay, v7 → v8 migration, and corrupted v8 state.
