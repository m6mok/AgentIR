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

## Completed Stage 3: MemoryIR

- separate typed MemoryIR and immutable memory-plan revision DAGs;
- deterministic fresh bufferization with explicit layouts, strides, address spaces and ownership;
- compiler-owned alias and logical lifetime facts;
- structurally proved in-place reuse and restricted `NoOverlap` guarded reuse with lazy exact fallback;
- independent `memory_hash`, memory event semantics v1, reference evaluator/trace and continuations;
- archive/snapshot v7 with explicit immutable v6 migration and deterministic replay.

Completion mapping: SpecIR → Stage 1; ImplIR/CandidateForest → Stage 2A; proof debt/guarded candidates → Stage 2B; exact equality → Stage 2C; logical-to-physical bufferization → Stage 3.

## Completed Stage 4: ScheduleIR and simulator

- separate typed ScheduleIR with conservative serial roots and immutable revision DAGs;
- compiler-derived iteration domains, exact split/tile/remainder coverage, and restricted fusion;
- target hierarchy binding, vectorization, unrolling, and MemoryIR compatibility proofs;
- immutable compiler-owned TargetManifest and independent `target_hash`;
- deterministic analytical resource simulation and reference scheduled execution;
- independent `schedule_hash`, schedule event semantics v1, and archive/snapshot v8 replay.

Completion mapping: SpecIR → Stage 1; ImplIR/CandidateForest → Stage 2A; proof debt/guarded candidates → Stage 2B; exact equality → Stage 2C; MemoryIR → Stage 3; ScheduleIR/TargetManifest/resource simulator → Stage 4; backend lowering and executable artifacts → Stage 5.

## Completed Stage 5: WebGPU/WGSL backend

- immutable typed BackendIR anchored to proved ScheduleIR and `backend_hash`;
- exact serial/grid, bounded remainder, restricted fusion, vector/unroll metadata and guarded-memory lowering for the one-dimensional elementwise subset;
- deterministic WGSL packages with retained binding ABI, offline Naga validation and independent `artifact_hash`;
- optional wgpu execution, separate device fingerprints and bounded confidence-only measurements;
- backend/artifact/measurement event persistence in archive/snapshot v9 with explicit v8 migration.

Stage 5 deliberately contains no reduction lowering, shared memory/subgroups, arbitrary GPU IR, autotuning, cost model, ranking or best-artifact search.

## Completed Stage 6A: reproducible agent policy evaluation

- separate `agentir-policy-eval` crate and `agentir-eval` JSONL CLI;
- immutable twenty-category offline task corpus and five deterministic scripted baselines;
- distinct free/menu/hybrid surfaces with production verifier execution;
- explicit observations, decisions, compiler outcomes, rejection/repair accounting and compiler-owned success;
- independent evaluation hashes, deterministic model/device-free replay, raw aggregates and fairness checks;
- separate evaluation archive v1 and optional same-device performance anchors.

## Completed Stage 6B: reproducible policy ranking

- stable bounded compiler-generated choice sets and identities;
- versioned visible features, fixed-point scores and deterministic ties;
- scripted/external rankers, explicit production selection and model-free replay;
- ranking metrics/fairness and evaluation archive v2 with explicit v1 migration.

## Completed Stage 6C: offline learned ranking foundation

- resumable exact choice pagination, typed repair descriptors, and deterministic non-semantic work counters;
- immutable policy-visible datasets with group-wise train/validation/test/excluded splits and leakage validation;
- bounded restartable pairwise integer-linear training and fixed-point inference;
- explicit learned policy kind, ordinary Stage 6B score validation, and one production dispatch after selection;
- evaluation archive v3 with explicit v1→v2→v3 migration and exact inference replay;
- two byte-identical local study/archive runs and the pre-Stage-7 architecture/readiness audit.

## Stage 7A: reproducible bounded offline search

- independently hashed structural objective and deterministic search-plan contracts;
- menu-only `deterministic_beam_v1` over existing production choice sets;
- isolated branch reconstruction, total frontier ordering and explicit duplicate provenance;
- deterministic advance/checkpoint/resume/cancellation and exact replay;
- evaluation archive v4 with pure v3 migration and no invented search history;
- scripted/learned beam-width 1/2/4 reproducibility study.

Stage 7A is not full autotuning and makes no globally optimal, hardware-performance or correctness claim.

## Stage 7B: reproducible measurement-aware offline search

- frozen same-device/build/runtime/config measurement cohorts over production records;
- separate integer median/p95 objective with ppm indifference semantics;
- terminal-only measured recommendations after unchanged Stage 7A search;
- deterministic checkpoint/resume/replay without hardware work;
- evaluation archive v5 and byte-identical two-run offline study.

Stage 7B is offline selection, not live autotuning, performance proof, global optimization, or correctness evidence.

## Completed Stage 7C: reproducible resumable measurement acquisition

- explicit server-owned preflight and bounded sequential acquisition;
- canonical artifact-hash round robin with fixed records per artifact;
- atomic complete-slot publication, checkpoint/resume/cancel and typed failures;
- separate Stage 7B cohort handoff, zero-device replay and evaluation archive v6;

## Completed Stage 7D: durable acquisition recovery

- durable prepare-before-hardware records with exact production publication snapshots;
- explicit execution and safe crash-boundary injection;
- server-owned zero/one/multiple reconciliation with no automatic rerun;
- explicit retry authorization with a new attempt ID and explicit abandonment;
- zero-device restore/replay, Stage 7C/7B handoff and evaluation archive v7.

Deferred beyond Stage 7D: concurrent writers, remote/distributed workers,
multi-device pooling, automatic retry, live tuning, prediction/training, energy
objectives, statistical inference, and new search/ranking algorithms.

## Completed Stage 7

Stage 7E integrates existing Stage 7A–7D records into a checkpointable campaign with one dormant optional hardware boundary, zero-device replay, and evaluation archive v8. The deterministic offline study materializes four distinct production-replayed terminal artifacts before labelled synthetic acquisition. ADR-180 makes this complete offline gate authoritative for project readiness, so Stage 7 is closed without requiring a physical GPU. Its acceptance evidence includes a byte-identical synthetic orchestration study without performance claims.

## Completed Stage 8A: deterministic CPU execution

- separate immutable `cpu_scalar_v1` target profile without GPU capabilities;
- compiler-owned lowering from proved serial ScheduleIR to portable scalar f32 bytecode;
- content-addressed `cpu_artifact_hash` and CPU compiler-build hash contracts independent from WGSL artifacts;
- safe bounded interpreter with exact input/shape checking and deterministic work counters;
- CPU artifact protocol publication/query/check/execute commands with no client-supplied bytecode or certificates;
- workspace archive/snapshot v10 with pure v9→v10 empty-store migration and replay verification without execution;
- byte-identical two-run Stage 8A study matching the reference evaluator.

## Completed Stage 8B: bounded CPU measurement

- separate `agentir-runtime-cpu` boundary for real monotonic timing and unchanged Stage 8A interpreter execution;
- bounded warmups/iterations, checked projected work, ordered raw nanosecond samples, output consistency, and deterministic integer min/median/p95/max;
- independent configuration/input/host/output/measurement hashes and append-only `CpuMeasurementStore`;
- acquire/list/query/check protocol with exactly one execution/clock command and no client-supplied observations;
- workspace archive/snapshot v11 with pure v10→v11 empty-store migration and zero-execution replay.

## Completed Stage 8

- Stage 8A publishes structurally proved, deterministic `cpu_scalar_v1` packages from verified ScheduleIR and executes them with the safe bounded interpreter.
- Stage 8B measures only retained, structurally verified packages through the sole execution/monotonic-clock acquisition boundary; measurements remain non-correctness observations.
- Stage 8C closes the combined contract with exact SAXPY execution, isolated synthetic clock/execution doubles, artifact/hash stability, atomic rejection, zero-execution query/check/archive replay, corruption rejection, archive v11 round-trip, and pure v10→v11 migration evidence.
- ADR-183 makes this offline gate authoritative for Stage 8 completion without speed, significance, portability, ranking, recommendation, selection, publication, or global-optimality claims.

## Completed Stage 9: isolated native CPU execution and offline closure

Stage 9 reuses an unchanged, compiler-published Stage 8A package as the sole
input to a pinned Cranelift JIT. Native lowering and the one audited call bridge
run in a fresh worker process, while core, persistence and the protocol process
retain their existing safe structural boundaries. Machine code remains
ephemeral; Stage 9 adds no native artifact publication, persistent store,
archive migration, proof relation, ranking or performance authority. See
[the Stage 9 scope](stage-9-scope.md) and ADR-184.

Stage 9A pins Cranelift 0.116.1, preserves exact add/mul/FMA semantics, verifies generated IR and confines the sole AgentIR-owned unsafe call to the worker bridge. Stage 9B implements production `cpu_native.execute` through a safe parent runtime, a server-selected hidden CLI worker mode, one fresh process and one native call. Parent and worker independently validate the unchanged package, while the parent additionally enforces bounded work, timeout/reaping, exact response framing, output validation and the runtime/execution observation identities. There is no interpreter fallback, retry, persisted state or performance claim.

Stage 9C closes the combined contract through `cargo test -p agentir-native-cpu-worker --test stage9_closure`. The gate composes the production compiler/package chain, real worker, unchanged portable interpreter, fixed-seed bitwise enum corpus, malformed/crash/timeout/forgery atomicity, zero-native structural/archive paths, reaping control flow, legacy archive pins and dependency/unsafe audits. It adds no production semantics or authority.

The Stage 9 contract is closed offline and the real-worker compatibility smoke passes on macOS/aarch64. Linux/x86_64 portability remains unconfirmed until the same closure command passes on that target; the offline closure result does not imply it.

AOT publication, SIMD/threading, reductions, broader dtype/rank support, host ABI embedding and CPU/GPU performance ranking remain future work.

Physical GPU qualification, concurrent/remote acquisition, multi-device pooling, new search algorithms, prediction, raw-sample significance, energy records, training during acquisition and live publication are outside the active strategy. Reintroducing a mandatory hardware gate requires a new ADR.
