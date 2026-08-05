# Getting started

## Prerequisites

- stable Rust 1.85+;
- Cargo;
- no GPU toolchain is required.

## Build and test

```bash
cargo build --workspace
cargo test --workspace
```

## Run the end-to-end example

```bash
cargo run -p agentir-cli --bin agentir < examples/saxpy.jsonl
```

The CLI keeps workspaces in memory while stdin remains open. Each input line is one request and each output line is one response. The example performs this lifecycle:

```text
workspace.open (r0)
→ spec.apply (r1)
→ spec.check
→ spec.freeze (r2)
→ program.evaluate
```

SAXPY is constructed exclusively through ActionIR. Its `zip_map` region receives `xi` and `yi` as block arguments and explicitly captures scalar parameter `a`. For inputs `a=2`, `x=[1,2,3,4]`, `y=[10,20,30,40]`, output `out` is `[12,24,36,48]`.

## Explore failure and partial-program behavior

```bash
cargo run -p agentir-cli --bin agentir < examples/invalid_type.jsonl
cargo run -p agentir-cli --bin agentir < examples/hole_continuation.jsonl
cargo run -p agentir-cli --bin agentir < examples/revision_branch.jsonl
```

The invalid transaction leaves `r0` unchanged. The hole example returns a parameteric continuation frame and then demonstrates that freeze is blocked. The branch example creates two independent children of one revision.

## Save and reopen a workspace

The first process writes an archive v6 at `/tmp/agentir-example.agentir.json`; the second verifies its checksum, SpecIR, mixed candidate/equality event semantics, proposal/proof state, semantic hash caches and complete replay before serving `r1`:

```bash
cargo run -p agentir-cli --bin agentir < examples/persistence_save.jsonl
cargo run -p agentir-cli --bin agentir < examples/persistence_load.jsonl
```

Use a project-controlled path for real work. Archive files can contain constants, names and provenance from the graph.
