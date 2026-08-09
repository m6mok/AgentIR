# Glossary

- **Measurement cohort** — immutable same-device/build/runtime/config/input set of verified production measurement records, independently hashed for Stage 7B.
- **Measured objective** — terminal-only integer median/p95 minimization descriptor anchored to a cohort and the frozen Stage 7A structural objective.
- **Measured recommendation** — non-authoritative offline selection among eligible measured terminal artifacts; never a proof or global optimum.
- **Indifference band** — versioned checked-integer ppm interval in which noisy measurement summaries are equivalent under one descriptor and resolve without a faster-than claim.

- **SearchObjectiveDescriptor** — ordered structural evaluation objective anchored to one exact corpus/task/root.
- **SearchPlan** — deterministic Stage 7A algorithmic envelope, distinct from runtime safety limits and ranker/model identity.
- **SearchNode/SearchEdge** — evaluation-only trajectory provenance whose selected action and outcome are production verified.
- **SearchCheckpoint** — exact anchored frontier and next-work cursor for deterministic resume.
- **Recommended trajectory** — non-authoritative observed terminal or bounded-frontier selection, never a global optimum or compiler proof.

- **EvaluationChoiceSet** — exact ordered compiler-generated actions visible to one ranked observation.
- **RankingTrace** — policy preferences plus deterministic tie result and explicit selected choice; never compiler proof.
- **SelectionOutcome** — selected choice or bounded escape anchored to its production compiler outcome.
- **FeatureSchema** — ordered versioned definition of policy-visible, non-hidden ranking features.

- **Evaluation corpus**: immutable ordered Stage 6A task definitions identified by `corpus_hash`.
- **Policy descriptor**: versioned free/menu/hybrid visible surface and deterministic configuration identified by `policy_hash`.
- **Evaluation observation**: exact agent-visible task/compiler/budget state and bounded continuation, identified by `observation_hash`.
- **Evaluation episode**: ordered observation/decision/compiler-outcome transcript with compiler-derived result and `episode_hash`.
- **Evaluation archive**: separate `agentir.evaluation.archive` family, currently v4; never a workspace archive or correctness proof.
- **Repair cycle**: interval from one rejected decision through the first accepted progress-producing decision or episode completion.
- **Free policy**: schema-valid production action surface without a generated menu.
- **Menu policy**: compiler-generated choice-only surface with no arbitrary escape.
- **Hybrid policy**: compiler choices plus a bounded typed escape that returns to the production verifier.

- **BackendIR**: separate typed executable kernel graph anchored to one proved ScheduleIR revision.
- **BackendEquivalentToSchedule**: compiler-owned structural certificate for exact backend lowering.
- **Artifact package**: deterministic manifest, complete runtime ABI and exact WGSL module bytes.
- **ArtifactEquivalentToBackend**: compiler-owned structural emission certificate.
- **Device fingerprint**: separately hashed runtime-reported adapter and capability provenance.
- **Hardware measurement**: confidence-only bounded timing record; never a proof or ranking decision.

**MemoryIR** — separate typed graph that materializes reachable ImplIR tensor values into abstract buffer regions without changing computation semantics.

**Memory plan/revision** — independent `mp*` branch and immutable `mr*` physical-state revision anchored to one `spec_hash` and `impl_hash`.

**MemoryEquivalentToImpl** — compiler-proved relation that storage, accesses, reuse and guarded fallback preserve the anchored ImplIR interface, numeric contract and outputs.

**memory_hash** — exact history-sensitive identity of one typed MemoryIR revision, distinct from all SpecIR, candidate, equality and archive hashes.

- **ActionIR** — typed algebra of graph edits submitted by an agent.
- **Archive** — checksummed versioned workspace encoding; v1/v2/v3/v4/v5/v6 are immutable legacy inputs and v7 is current.
- **Archive hash** — version-specific integrity hash of a concrete archive body.
- **Canonical state** — deterministic serialized `Program` used for the history-sensitive `content_hash` and replay.
- **Compiler core** — transport-independent verifier and workspace state machine.
- **Compiler semantics version** — event-level selector for historical transaction inference and obligation behavior; independent of archive format.
- **Candidate semantics version** — independent selector for CandidateForest event replay; legacy exact history uses v1, Stage 2B proposal/validation uses v2 and equality-linked revisions use v3.
- **Candidate** — one persistent ImplIR branch anchored to an immutable frozen `spec_hash`.
- **Candidate hash** — per-revision v1/v2/v3 history-sensitive exact identity including IDs, proof state and evidence references.
- **CandidateForest** — independent collection of immutable candidate revision DAGs, EvidenceIR and candidate allocator/event state.
- **ConstraintFacts** — deterministic derived equality/static-binding model used to query and discharge shape relations.
- **ContinuationFrame** — parameteric description of legal next choices for a focused task.
- **Hole** — missing pure value with a persistent ID and required type/shape.
- **EvidenceIR** — deterministic correctness/confidence records with hashes, method, parameters, result and provenance.
- **Equality edge** — compiler-owned positive proof that one whole ImplIR program reaches another through one exact production rewrite.
- **Equality hash** — canonical identity of an equality anchor, hash-consed nodes, proof edges, worklist and status, independent of batching history.
- **Equality node** — one fully verified whole-program ImplIR member, hash-consed by `impl_hash`.
- **Equality space** — bounded persistent positive proof graph rooted at one fully proved unconditional candidate revision.
- **Guarded fallback** — candidate-level compiler guard selecting a conditional primary or immutable proved exact fallback lazily.
- **Impl hash** — history-independent identity of reachable typed ImplIR semantics.
- **ImplIR** — separate typed functional graph describing one implementation of frozen SpecIR.
- **MemoryIR physical boundary** — current Stage 3 layer; ScheduleIR and target lowering remain future work.
- **Obligation** — explicit proposition that is open, proved, refuted or unsupported.
- **Proof debt** — ordered persistent speculative obligations connecting consecutive implementation hashes.
- **Proof frontier** — last consecutive candidate prefix whose exact equivalence has compiler-owned proof; it may lag behind head.
- **Proposal hash** — domain-separated identity of an alpha-normalized replacement proposal before persistent ImplIR ID allocation.
- **Speculative proposal** — typed replacement fragment accepted with explicit opt-in but not treated as correctness evidence.
- **Persistent ID** — compiler-assigned identity such as `v4`, `h1` or `r2`.
- **Region** — closed pure block with typed arguments, explicit captures and a yield.
- **Revision** — immutable workspace snapshot with parent links and content hash.
- **Resource limit** — runtime workload policy excluded from all SpecIR/ImplIR/candidate semantic or exact hashes.
- **ScheduleIR** — future mapping of work to target hardware.
- **SpecIR** — functional graph describing what must be computed.
- **Semantic canonical form** — versioned, alpha-normalized output-reachable representation of frozen SpecIR.
- **Spec hash** — domain-separated SHA-256 identity of semantic canonical form, independent of compiler IDs and construction history.
- **Temporary binding** — `$name` usable only within one transaction.
- **Workspace** — SpecIR revision DAG plus independent CandidateForest, EqualityStore, MemoryPlanStore and compiler-owned allocators; it may be persisted only after complete replay verification.
- **Ranking dataset** — immutable Stage 6C visible ranking inputs plus separately stored historical labels, split by semantic group.
- **Learned model artifact** — deterministic fixed-point evaluation artifact anchored to dataset, split, configuration, schema and codec; it has no correctness authority.
- **Inference record** — exact input/model/policy/choice-set anchors and one fixed-point score per visible choice; work counters are observational.
- **Continuation cursor** — opaque versioned compiler-owned token for resuming an exact bounded enumeration at unchanged anchors.
- **Typed repair** — bounded compiler-owned descriptor anchored to an exact diagnostic/base; it still traverses the production verifier and does not promise acceptance.
- **Measurement acquisition plan** — immutable Stage 7C workspace/artifact/config contract with canonical artifact-hash round-robin order.
- **Recovery journal** — Stage 7D single-writer record that durably anchors one prepared Stage 7C slot and every explicit recovery decision.
- **Prepared slot** — immutable attempt record created before hardware authorization, including an exact production publication snapshot.
- **Reconciliation** — server-owned zero-device classification of compatible production measurements appearing after a prepared boundary.
- **Indeterminate hardware execution** — a state in which a benchmark may have run but exactly-once execution cannot be proved; automatic retry is forbidden.
- **Autotuning campaign** — Stage 7E evaluation-only composition of frozen search, acquisition, recovery, cohort, and recommendation records.
- **Campaign checkpoint** — exact zero-device restart snapshot carrying campaign and current Stage 7A–7D anchors.
- **Campaign result** — non-authoritative final recommendation record; it neither publishes an artifact nor advances correctness.
- **Stage 7 closure gate** — deterministic multi-artifact production replay, labelled synthetic lifecycle/recovery evidence, zero-device replay, archive verification and the full offline workspace gate; physical GPU execution is optional compatibility evidence.
- **Acquisition slot** — one complete benchmark/publication unit; checkpoint and cancellation occur only between slots.
- **Acquisition checkpoint** — independently hashed completed-prefix snapshot whose device/build/runtime/record anchors are revalidated before resume.
- **Acquisition result** — non-correctness terminal orchestration record; it is not a cohort, recommendation or performance proof.
# Stage 4 terms

- **TargetManifest** — immutable compiler-owned capability contract identified by `target_hash`.
- **ScheduleIR** — separate typed exact schedule graph anchored to MemoryIR and TargetManifest.
- **iteration domain** — compiler-derived logical coordinate space for one ImplIR operation.
- **compiler remainder** — exact compiler-owned tail domain for non-divisible or symbolic splitting.
- **resource simulator** — deterministic analytical target-capacity checker, not a performance model.
- **schedule_hash** — exact ScheduleIR state contract, independent from all prior hashes.
