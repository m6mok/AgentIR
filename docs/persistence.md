# Persistence, semantics versions, migration and replay

The current writer emits archive/snapshot v11. V1–v10 remain immutable legacy inputs and cross explicit migration edges. The v8→v9 edge adds empty BackendStore, ArtifactStore and MeasurementStore; v9→v10 adds an empty CpuArtifactStore; v10→v11 adds an empty CpuMeasurementStore. None recalculates legacy IDs, events, or semantic hashes or invents execution history.

V7 snapshots persist memory plans, memory-local allocator/evidence, and semantics-v1 events with candidate/equality dependency cursors. Publication occurs only after legacy replay, MemoryIR event replay, anchor revalidation, structural verification, exact `memory_hash` recomputation, and allocator/head/store equality. New saves never write v6.

AgentIR persistence is split across two layers. `agentir-core` owns versioned snapshots, compiler-semantics-tagged events and deterministic replay but performs no I/O. `agentir-store` owns bounded file reads, exact archive codecs, integrity checks, the migration registry and atomic filesystem replacement.

## Independent version axes

Archive format, workspace snapshot schema, event compiler semantics and semantic canonicalization are separate contracts:

| Contract | Legacy | Current | Purpose |
| --- | --- | --- | --- |
| archive format | v1–v10 | v11 | exact on-disk envelope and `archive_hash` |
| snapshot schema | v1–v10 | v11 | resumable workspace representation |
| core semantics | v1 | v2 | transaction inference, obligation proposition and discharge |
| semantic canonical form | — | v1 | history-independent `spec_hash` |
| candidate semantics | v1, v2 | v3 | exact, speculative and equality-linked candidate events |
| candidate canonical/hash | v1, v2 | v3 | per-revision exact candidate identity |
| equality semantics/canonical | — | v1/v1 | equality event replay and exact state identity |
| ImplIR semantics/canonical | — | v1/v1 | verifier/evaluator behavior and `impl_hash` codec |
| memory semantics/event/canonical | — | v1/v1/v1 | MemoryIR verification, replay and exact state identity |

Archive v1 was published by commit `97c821a`. V2 added cached `spec_hash`; v3 added versioned SpecIR events; v4 added CandidateForest; v5 added speculative state; v6 added EqualityStore; v7 added MemoryPlanStore; v8 added target and schedule stores; v9 added backend, artifact and WebGPU measurement stores; v10 added CPU artifacts; v11 adds bounded CPU measurements. V1–v10 source codecs are immutable and new saves always write v11.

Migration tags every v1/v2 event with `LEGACY_CORE_SEMANTICS_VERSION = 1`. New transactions and forks use `CORE_SEMANTICS_VERSION = 2`. A restored draft can therefore append v2 events without rewriting old history. Replay dispatches each event independently, so legacy obligation propositions and `content_hash` values remain unchanged.

## Load pipeline

Nothing is published until all stages succeed:

1. read at most the hard archive-byte cap;
2. inspect only `format` and `format_version`;
3. deserialize the exact v1–v11 codec;
4. recompute that source version's `archive_hash`;
5. check revision/event/action counts against hard safety caps;
6. apply the explicit v1 → v2 → v3 → v4 → v5 → v6 → v7 → v8 → v9 → v10 → v11 chain (or suffix/no-op);
7. require snapshot schema v11;
8. replay every event with its declared core semantics version;
9. reproduce persistent IDs, parents, event hashes, `content_hash` and status summaries;
10. recompute every frozen revision's `spec_hash` and semantic codec version;
11. interleave candidate events under semantics v1/v2/v3 with equality events under semantics v1 using explicit dependency cursors;
12. verify every normalized `proposal_hash`, ImplIR and per-revision candidate hash v1/v2/v3;
13. verify proof frontier, ordered debt/statuses, translation certificates and EvidenceIR;
14. verify guard/fallback anchors, hashes and acyclic bounded fallback graph;
15. rebuild every equality node/edge/worklist/explanation, verify equality hashes and linkage evidence;
16. replay memory events at their explicit candidate/equality dependency cursors;
17. rebuild every MemoryIR plan, buffer/access/lifetime/alias fact, reuse/guard certificate and `memory_hash`;
18. verify CPU artifact packages/events, then CPU measurement hashes, anchors, allocator and dependency cursors without execution or clock reads;
19. verify all persistent stores and return only the complete workspace.

Unknown archive, snapshot or event-semantics versions are rejected. Serde defaults are not a migration mechanism. Source checksum verification always precedes migration, and count budgets precede event application.

## Save and migrate guarantees

`workspace.save` builds archive v11 in memory, checks its encoded size, writes a unique same-directory temporary file, flushes and `sync_all`s it, then atomically renames it. A failed write removes the temporary file and leaves the prior destination untouched where same-directory rename is atomic.

Archive v10 adds the deterministic CPU artifact store and event history. Archives v1–v9 remain immutable legacy inputs; the only new migration edge is pure v9→v10, which adds an empty CPU store and invents no package, execution, timing, or proof history. Replay verifies CPU package hashes, certificates, anchors and event order without executing bytecode.

Archive v11 adds the independent CPU measurement store and event history. Archive v10 is immutable; v10→v11 adds only an empty store and invents no fingerprint, timing, sample, output, or execution history. Native replay verifies all measurement identities and CPU-artifact dependency cursors with zero executions and zero clock reads.

`workspace.migrate_archive` fully verifies and replays the source before checking/writing the destination. The source is never edited. Existing or identical destinations require `overwrite: true`; a failure creates no partial destination. `MigrationReport` records the verified source version/hash, target v11, every migration step and the new hash when written.

V4 to v5 first verifies the exact v4 envelope through immutable v4 structs. It preserves SpecIR revisions/events/hashes and every candidate ID, state, evidence reference, candidate-hash-v1 byte contract and candidate-semantics-v1 event. It adds empty proposal/debt/guard stores and a zero proposal allocator; it never manufactures proposals or recalculates legacy hashes.

V5 to v6 likewise verifies the immutable v5 codec and adds an empty EqualityStore. It preserves all v1/v2 candidate bytes, IDs, hashes and events. Equality history can exist only in native v6 state and candidate/equality event dependencies are then replayed exactly.

V6 to v7 verifies the immutable v6 codec and complete Stage 1/2 replay before adding an empty deterministic MemoryPlanStore. It preserves all legacy SpecIR, candidate and equality bytes, IDs, hashes and events. Memory history can exist only in native v7 state.

This remains local atomic replacement, not a concurrent database: there is no process lock, compare-and-swap generation, compression, encryption or directory `fsync`.

## Fixtures

V1/v2/v3/v4/v5/v6 fixtures are immutable compatibility inputs. V6 fixtures cover empty/root/partial/saturated/merged/discharged/materialized and mixed candidate-semantics histories. V7 fixtures add fresh, reused, guarded, sealed and equality-materialized MemoryIR histories plus corruption at each memory integrity boundary. See `crates/agentir-store/tests/fixtures/README.md` for pinned hashes and reproducible version-specific generators.
# Archive v8

Archive/snapshot v8 is the immutable Stage 4 codec. Its v7→v8 edge adds empty TargetManifest and SchedulePlan stores. V8 records target and schedule events with candidate/equality/memory/target dependency cursors and verifies manifest hashes, schedule anchors, structural certificates, resource estimates, allocator state, event order and `schedule_hash`.

# Archive v9

V8 is now an immutable Stage 4 input. The explicit v8→v9 migration verifies its envelope and complete Stage 1–4 replay, then adds empty backend, artifact and measurement stores. Native v9 replay also verifies BackendIR hashes/certificates, exact WGSL package bytes and ABIs, artifact certificates, compiler build provenance, measurement hashes and event dependency order before publication.

# Archive v10 and v11

V10 is the immutable Stage 8A codec with compiler-published CPU packages. V11 is the Stage 8B codec with the separate `CpuMeasurementStore`; its only new migration edge adds that store empty. CPU measurement replay verifies exact artifact/build/runtime/config/input/host/sample/aggregate/output identities and ordered CPU-artifact cursors without calling the interpreter or clock.
