# Roadmap

## Completed in Stage 1.1

- versioned semantic canonical form and history-independent `spec_hash`;
- archive/snapshot v2 with explicit v1 migration and golden fixtures;
- deterministic permutation and archive round-trip property harnesses;
- migration protocol command and atomic destination handling.

## Immediate hardening

1. Make constraint addition discharge compact shape obligations incrementally.
2. Add process locking or a transactional database when concurrent workspace access is required.
3. Extend resource budgets and fuzz/property coverage for parsers and interpreter edge cases.
4. Add incremental canonical subgraph digest caching if measurements justify it.

## Stage 2: ImplIR and refinement

Use frozen SpecIR `spec_hash` as the immutable contract anchor, introduce candidate implementations and explicit `EquivalentToSpec`/`RefinesSpec` obligations. Keep candidate search branched and evidence-linked.

## Stage 3: MemoryIR

Add logical-to-physical bufferization, layouts, address spaces, alias obligations and guarded in-place reuse without exposing raw pointers.

## Stage 4: ScheduleIR and simulator

Represent tile/split/fuse/bind/vectorize choices separately from algorithm and memory. Build CPU/GPU legality and resource simulators plus measurement records.

## Stage 5: first GPU backend

Select one target family, lower a deliberately small kernel subset, emit reproducibility manifests, and compare generated artifacts on target hardware.

## Stage 6: agent evaluation

Compare free, menu and hybrid policies using accepted actions per token, rejection rate, repair cycles, context size, semantic correctness and measured kernel performance.
