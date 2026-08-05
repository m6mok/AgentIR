# Persistence, semantics versions, migration and replay

The current writer emits archive/snapshot v7. V1–v6 remain immutable legacy inputs; v6 is decoded through `LegacyWorkspaceSnapshotV6`, its exact envelope checksum and complete Stage 1/2 replay are verified, then explicit v6 → v7 migration adds an empty deterministic `MemoryPlanStore`. Legacy IDs, events and hashes are not recalculated.

V7 snapshots persist memory plans, memory-local allocator/evidence, and semantics-v1 events with candidate/equality dependency cursors. Publication occurs only after legacy replay, MemoryIR event replay, anchor revalidation, structural verification, exact `memory_hash` recomputation, and allocator/head/store equality. New saves never write v6.

AgentIR persistence is split across two layers. `agentir-core` owns versioned snapshots, compiler-semantics-tagged events and deterministic replay but performs no I/O. `agentir-store` owns bounded file reads, exact archive codecs, integrity checks, the migration registry and atomic filesystem replacement.

## Independent version axes

Archive format, workspace snapshot schema, event compiler semantics and semantic canonicalization are separate contracts:

| Contract | Legacy | Current | Purpose |
| --- | --- | --- | --- |
| archive format | v1, v2, v3, v4, v5 | v6 | exact on-disk envelope and `archive_hash` |
| snapshot schema | v1, v2, v3, v4, v5 | v6 | resumable workspace representation |
| core semantics | v1 | v2 | transaction inference, obligation proposition and discharge |
| semantic canonical form | — | v1 | history-independent `spec_hash` |
| candidate semantics | v1, v2 | v3 | exact, speculative and equality-linked candidate events |
| candidate canonical/hash | v1, v2 | v3 | per-revision exact candidate identity |
| equality semantics/canonical | — | v1/v1 | equality event replay and exact state identity |
| ImplIR semantics/canonical | — | v1/v1 | verifier/evaluator behavior and `impl_hash` codec |

Archive v1 was published by commit `97c821a`. V2 added cached `spec_hash`; v3 added `VersionedWorkspaceEvent`; v4 added CandidateForest and candidate events; v5 added proposals, proof debt, translation records and guards; v6 added EqualityStore, equality events and candidate v3 linkage. All six source codecs are immutable. V7 adds MemoryPlanStore and memory events; new saves always write v7.

Migration tags every v1/v2 event with `LEGACY_CORE_SEMANTICS_VERSION = 1`. New transactions and forks use `CORE_SEMANTICS_VERSION = 2`. A restored draft can therefore append v2 events without rewriting old history. Replay dispatches each event independently, so legacy obligation propositions and `content_hash` values remain unchanged.

## Load pipeline

Nothing is published until all stages succeed:

1. read at most the hard archive-byte cap;
2. inspect only `format` and `format_version`;
3. deserialize the exact v1, v2, v3, v4, v5 or v6 codec;
4. recompute that source version's `archive_hash`;
5. check revision/event/action counts against hard safety caps;
6. apply the explicit v1 → v2 → v3 → v4 → v5 → v6 chain (or suffix/no-op);
7. require snapshot schema v6;
8. replay every event with its declared core semantics version;
9. reproduce persistent IDs, parents, event hashes, `content_hash` and status summaries;
10. recompute every frozen revision's `spec_hash` and semantic codec version;
11. interleave candidate events under semantics v1/v2/v3 with equality events under semantics v1 using explicit dependency cursors;
12. verify every normalized `proposal_hash`, ImplIR and per-revision candidate hash v1/v2/v3;
13. verify proof frontier, ordered debt/statuses, translation certificates and EvidenceIR;
14. verify guard/fallback anchors, hashes and acyclic bounded fallback graph;
15. rebuild every equality node/edge/worklist/explanation, verify equality hashes and linkage evidence;
16. verify CandidateForest and EqualityStore consistency and return only the complete workspace.

Unknown archive, snapshot or event-semantics versions are rejected. Serde defaults are not a migration mechanism. Source checksum verification always precedes migration, and count budgets precede event application.

## Save and migrate guarantees

`workspace.save` builds archive v7 in memory, checks its encoded size, writes a unique same-directory temporary file, flushes and `sync_all`s it, then atomically renames it. A failed write removes the temporary file and leaves the prior destination untouched where same-directory rename is atomic.

`workspace.migrate_archive` fully verifies and replays the source before checking/writing the destination. The source is never edited. Existing or identical destinations require `overwrite: true`; a failure creates no partial destination. `MigrationReport` records the verified source version/hash, target v6, every migration step and the new hash when written.

V4 to v5 first verifies the exact v4 envelope through immutable v4 structs. It preserves SpecIR revisions/events/hashes and every candidate ID, state, evidence reference, candidate-hash-v1 byte contract and candidate-semantics-v1 event. It adds empty proposal/debt/guard stores and a zero proposal allocator; it never manufactures proposals or recalculates legacy hashes.

V5 to v6 likewise verifies the immutable v5 codec and adds an empty EqualityStore. It preserves all v1/v2 candidate bytes, IDs, hashes and events. Equality history can exist only in native v6 state and candidate/equality event dependencies are then replayed exactly.

This remains local atomic replacement, not a concurrent database: there is no process lock, compare-and-swap generation, compression, encryption or directory `fsync`.

## Fixtures

V1/v2/v3/v4/v5 fixtures are immutable compatibility inputs. V6 fixtures cover empty/root/partial/saturated/merged/discharged/materialized and mixed candidate-semantics histories. Valid-envelope corruption fixtures damage equality anchors, nodes, edges, rules, side conditions, worklist/status, hashes, evidence or event ordering; every case fails before publication. See `crates/agentir-store/tests/fixtures/README.md` for pinned hashes and reproducible version-specific generators.
