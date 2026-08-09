# Development

Stage 5 offline checks require no GPU. Run `cargo test --workspace` and the backend examples normally; set `AGENTIR_RUN_GPU_TESTS=1` only to opt into compatible real-device differential tests. The workspace pins the wgpu/Naga 24 family to retain the repository Rust 1.85 MSRV while using matching WGSL validation/runtime APIs.

Stage 6A adds an offline `agentir-policy-eval` library and `agentir-eval` JSONL CLI. Run every `eval_*.jsonl` example plus `cargo run --release -p agentir-policy-eval --example evaluation_baseline`; no provider account, network, GPU, or model call is required.

Stage 6B remains offline. Run every `eval_ranked_*.jsonl` example and the `stage6b` policy-eval tests; ranking replay must report zero external/device calls and Stage 6A pinned episode hashes must remain unchanged.

Stage 6C remains offline and dependency-light. Run `cargo test -p agentir-policy-eval --test stage6c` and `--test contract_registry`. Generate two studies with `stage6c_study --output target/stage6c-study/run-1` and `run-2`, then compare them with `stage6c_compare`. Semantic files and evaluation archives must be byte-identical; timing samples are excluded.

Stage 7A remains offline and evaluation-only. Run `cargo test -p agentir-policy-eval --test stage7a`, generate `target/stage7a-study/run-1` and `run-2` with `stage7a_study`, and compare with `stage7a_compare`. Semantic, search-result, checkpoint, mutation classification and evaluation archive bytes must be identical. No provider, network, GPU/device or hardware benchmark is used.

Stage 7B remains offline and consumes only verified pre-existing measurement records. Run `cargo test -p agentir-policy-eval --test stage7b`, generate `target/stage7b-study/run-1` and `run-2` with `stage7b_study`, and compare with `stage7b_compare`. The comparator excludes timing observations and requires every semantic/recommendation/checkpoint/mutation/archive-v5 file to be byte-identical.

The expanded local Stage 6B.1 study is generated with `cargo run --release -p agentir-policy-eval --example stage6b_study -- --output target/stage6b-study/run-1`. Repeat with a second directory, then run `cargo run --release -p agentir-policy-eval --example stage6b_compare -- target/stage6b-study/run-1 target/stage6b-study/run-2`. The comparator requires byte-identical `semantic.json`; timing samples are retained separately as expected machine noise. All outputs stay under ignored `target/` and are never correctness evidence.

Stage 3 changes should additionally run the four MemoryIR JSONL examples and verify fresh/reuse/guarded outputs agree, the unsafe reuse request is rejected without preventing the following transaction, v6 fixtures retain their pinned bytes, and v7 save/replay reproduces memory IDs/hashes. `cargo test --workspace` includes core atomicity/replay and protocol branch-laziness coverage.

## Repository layout

```text
crates/agentir-core       canonical data and compiler state machine
crates/agentir-eval       CPU reference semantics
crates/agentir-store      versioned archive I/O and replay verification
crates/agentir-backend-wgsl deterministic BackendIR/WGSL compiler and offline validator
crates/agentir-runtime-wgpu optional WebGPU discovery, execution and measurements
crates/agentir-protocol   wire requests, responses, workspace registry
crates/agentir-cli        stdin/stdout JSONL process
crates/agentir-policy-eval immutable corpus, ranking/learning/search, replay, metrics, evaluation archive v4
crates/agentir-eval-cli   Stage 6A stdin/stdout JSONL process
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
cargo run -p agentir-cli --bin agentir < examples/schedule_serial.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_tiled.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_remainder.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_fused.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_vectorized.jsonl
cargo run -p agentir-cli --bin agentir < examples/schedule_guarded_memory.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_to_schedule.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_saxpy_wgsl.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_serial.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_tiled.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_remainder.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_fused.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_vectorized.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_reuse.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_guarded_memory.jsonl
cargo run -p agentir-cli --bin agentir < examples/equality_to_artifact.jsonl
cargo run -p agentir-cli --bin agentir < examples/backend_rejected_reduce.jsonl
cargo run --release -p agentir-protocol --example baseline
cargo run -p agentir-eval -- < examples/eval_free_saxpy.jsonl
cargo run -p agentir-eval -- < examples/eval_compare_policies.jsonl
cargo run -p agentir-eval -- < examples/eval_replay.jsonl
cargo run -p agentir-eval -- < examples/eval_search_start.jsonl
cargo run --release -p agentir-policy-eval --example evaluation_baseline
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
