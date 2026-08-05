# Persistence, semantics versions, migration and replay

AgentIR persistence is split across two layers. `agentir-core` owns versioned snapshots, compiler-semantics-tagged events and deterministic replay but performs no I/O. `agentir-store` owns bounded file reads, exact archive codecs, integrity checks, the migration registry and atomic filesystem replacement.

## Independent version axes

Archive format, workspace snapshot schema, event compiler semantics and semantic canonicalization are separate contracts:

| Contract | Legacy | Current | Purpose |
| --- | --- | --- | --- |
| archive format | v1, v2, v3 | v4 | exact on-disk envelope and `archive_hash` |
| snapshot schema | v1, v2, v3 | v4 | resumable workspace representation |
| core semantics | v1 | v2 | transaction inference, obligation proposition and discharge |
| semantic canonical form | — | v1 | history-independent `spec_hash` |
| candidate semantics | — | v1 | candidate transactions, IDs, evidence and replay |
| ImplIR semantics/canonical | — | v1/v1 | verifier/evaluator behavior and `impl_hash` codec |

Archive v1 was published by commit `97c821a`. V2 added cached `spec_hash`; v3 added `VersionedWorkspaceEvent`. All three codecs are immutable. V4 adds CandidateForest, its allocator, EvidenceIR and `VersionedCandidateEvent`; new saves always write v4.

Migration tags every v1/v2 event with `LEGACY_CORE_SEMANTICS_VERSION = 1`. New transactions and forks use `CORE_SEMANTICS_VERSION = 2`. A restored draft can therefore append v2 events without rewriting old history. Replay dispatches each event independently, so legacy obligation propositions and `content_hash` values remain unchanged.

## Load pipeline

Nothing is published until all stages succeed:

1. read at most the hard archive-byte cap;
2. inspect only `format` and `format_version`;
3. deserialize the exact v1, v2, v3 or v4 codec;
4. recompute that source version's `archive_hash`;
5. check revision/event/action counts against hard safety caps;
6. apply the explicit v1 → v2 → v3 → v4 chain (or suffix/no-op);
7. require snapshot schema v4;
8. replay every event with its declared core semantics version;
9. reproduce persistent IDs, parents, event hashes, `content_hash` and status summaries;
10. recompute every frozen revision's `spec_hash` and semantic codec version;
11. replay candidate events under candidate semantics v1;
12. verify every ImplIR, `impl_hash`, `candidate_hash`, SpecIR anchor, evidence record and compositional certificate chain;
13. verify CandidateForest consistency and return only the complete workspace.

Unknown archive, snapshot or event-semantics versions are rejected. Serde defaults are not a migration mechanism. Source checksum verification always precedes migration, and count budgets precede event application.

## Save and migrate guarantees

`workspace.save` builds archive v4 in memory, checks its encoded size, writes a unique same-directory temporary file, flushes and `sync_all`s it, then atomically renames it. A failed write removes the temporary file and leaves the prior destination untouched where same-directory rename is atomic.

`workspace.migrate_archive` fully verifies and replays the source before checking/writing the destination. The source is never edited. Existing or identical destinations require `overwrite: true`; a failure creates no partial destination. `MigrationReport` records the verified source version/hash, target v4, every migration step and the new hash when written.

This remains local atomic replacement, not a concurrent database: there is no process lock, compare-and-swap generation, compression, encryption or directory `fsync`.

## Fixtures

V1/v2/v3 fixtures are immutable compatibility inputs. V4 fixtures cover empty/frozen/identity/rewritten-sealed/migrated candidate states. A pinned corrupted-v4 corpus keeps a valid source envelope while damaging candidate semantics, `impl_hash`, `candidate_hash`, the SpecIR anchor or the evidence chain; every case must fail before publication. See `crates/agentir-store/tests/fixtures/README.md` for hashes and provenance.
