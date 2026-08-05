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

## Completed Stage 2B: bounded speculative candidate space

- typed agent proposals with alpha-normalized `proposal_hash` and explicit opt-in;
- persistent ordered debt plus a proof frontier distinct from candidate head;
- compiler-owned identity/known-rewrite translation validation and deterministic refutation;
- one exact lazy guarded self-division fallback;
- candidate hash/semantics v2 and mixed v1/v2 replay;
- archive/snapshot v5 and explicit exact v4 migration.

Approximate refinement, e-graphs, search policy and ranking remain separate future work; none may weaken exact certificates.

## Completed Stage 2: ImplIR, proof debt and exact equality space

Completion mapping from the 0.1 reference specification:

- CandidateForest and compiler-owned known rewrites → Stage 2A;
- reference equivalence chains and checking → Stage 2A/2B;
- speculative rewrites and persistent proof debt → Stage 2B;
- e-graph-like bounded exact equality space → Stage 2C.

### Stage 2C: exact equality space

- persistent whole-program equality nodes hash-consed by `impl_hash`;
- compiler-owned positive proof edges from the shared exact production rewrite engine;
- deterministic bounded expansion, saturation, continuation and canonical explanations;
- equality membership discharge for ordered proof debt and explicit candidate materialization;
- candidate hash/semantics v3 with mixed v1/v2/v3 replay;
- archive/snapshot v6 with equality events, immutable v5 migration and corruption fixtures.

Stage 2 is complete at the exact-only boundary. The equality space is not an e-graph, extractor, ranker or search policy, and it adds no approximate relation.

## Stage 3: MemoryIR

Add logical-to-physical bufferization, layouts, address spaces, alias obligations and guarded in-place reuse without exposing raw pointers.

## Stage 4: ScheduleIR and simulator

Represent tile/split/fuse/bind/vectorize choices separately from algorithm and memory. Build CPU/GPU legality and resource simulators plus measurement records.

## Stage 5: first GPU backend

Select one target family, lower a deliberately small kernel subset, emit reproducibility manifests, and compare generated artifacts on target hardware.

## Stage 6: agent evaluation

Compare free, menu and hybrid policies using accepted actions per token, rejection rate, repair cycles, context size, semantic correctness and measured kernel performance.
