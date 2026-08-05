# Development

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
cargo run -p agentir-cli --bin agentir < examples/saxpy.jsonl
```

Tests are split by responsibility: small inference/shape/canonical unit tests in core; workspace atomicity, revisions, continuations and replay as core integration tests; semantic execution in eval; archive round-trip/tamper handling in store; line-level envelopes, persistence commands and the full example in protocol.

## Baseline harness

Run the intentionally dependency-free measurement harness in release mode:

```bash
cargo run --release -p agentir-protocol --example baseline
```

It reports transaction apply for 1/10/100 operations, repeated shape queries, canonical serialization, continuation generation and SAXPY evaluation at several sizes. These are local baselines, not stable performance promises. Record hardware, compiler version and git revision when comparing runs.

## Adding an opcode

Update the canonical `Opcode`, type/shape inference, region policy if relevant, evaluator, continuation domains, protocol examples and tests. Keep result types compiler-owned. If semantics have alternatives, add an explicit attribute or numeric-contract field instead of a hidden default.

## Dependency policy

Dependencies are deliberately small: `serde`, `serde_json` and `sha2`. Prefer the standard library and deterministic collections. GPU/compiler-framework dependencies belong to later stages and should enter behind a new crate boundary.
