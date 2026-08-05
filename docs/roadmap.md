# Roadmap

## Completed through Stage 1.2

- versioned semantic canonical form and history-independent `spec_hash`;
- archive/snapshot v2 with explicit v1 migration and golden fixtures;
- deterministic permutation and archive round-trip property harnesses;
- migration protocol command and atomic destination handling;
- deterministic compact constraint facts and incremental `ShapeCompatible` discharge;
- event-level core semantics versions and archive/snapshot v3;
- central resource budgets across core/store/protocol/CLI/evaluator;
- fixed-seed solver soundness plus archive/protocol mutation corpora;
- statistical benchmark schema v2 with median/p95 and environment metadata.

Process locking remains deferred until concurrent workspace access is required. Incremental canonical subgraph caching should be added only if the statistical baseline identifies it as a material bottleneck.

## Completed Stage 2A: exact candidate foundation

- separate typed ImplIR and `impl_hash`;
- immutable CandidateForest/revision branches and `candidate_hash`;
- identity lowering plus compiler-owned exact rewrites;
- compositional `EquivalentToSpec` certificates;
- correctness/confidence EvidenceIR and deterministic differential validation;
- candidate semantics v1 and archive/snapshot v4.

## Stage 2B: speculative candidate space

Introduce agent-proposed rewrites, explicit proof debt and guarded/refuted states only with a clear trust boundary. Approximate refinement, e-graph/equality-space exploration, search policy and ranking remain separate opt-in work; none should weaken Stage 2A exact certificates.

## Stage 3: MemoryIR

Add logical-to-physical bufferization, layouts, address spaces, alias obligations and guarded in-place reuse without exposing raw pointers.

## Stage 4: ScheduleIR and simulator

Represent tile/split/fuse/bind/vectorize choices separately from algorithm and memory. Build CPU/GPU legality and resource simulators plus measurement records.

## Stage 5: first GPU backend

Select one target family, lower a deliberately small kernel subset, emit reproducibility manifests, and compare generated artifacts on target hardware.

## Stage 6: agent evaluation

Compare free, menu and hybrid policies using accepted actions per token, rejection rate, repair cycles, context size, semantic correctness and measured kernel performance.
