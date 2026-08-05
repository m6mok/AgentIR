# Roadmap

## Immediate hardening

1. Persist canonical revisions and replay transaction logs.
2. Add schema/version negotiation and golden protocol fixtures.
3. Make constraint addition discharge compact shape obligations incrementally.
4. Add canonical renumbering so equivalent construction histories converge on one semantic hash.
5. Add resource budgets and fuzz/property tests for parser, transactions and interpreter.

## Stage 2: ImplIR and refinement

Freeze SpecIR as an immutable contract, introduce candidate implementations and explicit `EquivalentToSpec`/`RefinesSpec` obligations. Keep candidate search branched and evidence-linked.

## Stage 3: MemoryIR

Add logical-to-physical bufferization, layouts, address spaces, alias obligations and guarded in-place reuse without exposing raw pointers.

## Stage 4: ScheduleIR and simulator

Represent tile/split/fuse/bind/vectorize choices separately from algorithm and memory. Build CPU/GPU legality and resource simulators plus measurement records.

## Stage 5: first GPU backend

Select one target family, lower a deliberately small kernel subset, emit reproducibility manifests, and compare generated artifacts on target hardware.

## Stage 6: agent evaluation

Compare free, menu and hybrid policies using accepted actions per token, rejection rate, repair cycles, context size, semantic correctness and measured kernel performance.

