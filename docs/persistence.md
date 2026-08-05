# Persistence, semantics versions, migration and replay

AgentIR persistence is split across two layers. `agentir-core` owns versioned snapshots, compiler-semantics-tagged events and deterministic replay but performs no I/O. `agentir-store` owns bounded file reads, exact archive codecs, integrity checks, the migration registry and atomic filesystem replacement.

## Independent version axes

Archive format, workspace snapshot schema, event compiler semantics and semantic canonicalization are separate contracts:

| Contract | Legacy | Current | Purpose |
| --- | --- | --- | --- |
| archive format | v1, v2 | v3 | exact on-disk envelope and `archive_hash` |
| snapshot schema | v1, v2 | v3 | resumable workspace representation |
| core semantics | v1 | v2 | transaction inference, obligation proposition and discharge |
| semantic canonical form | — | v1 | history-independent `spec_hash` |

Archive v1 was published by commit `97c821a`. Archive/snapshot v2 added cached `spec_hash` and `semantic_canonical_version`. Both codecs are immutable. Archive/snapshot v3 adds `VersionedWorkspaceEvent { semantics_version, event }`; new saves always write v3.

Migration tags every v1/v2 event with `LEGACY_CORE_SEMANTICS_VERSION = 1`. New transactions and forks use `CORE_SEMANTICS_VERSION = 2`. A restored draft can therefore append v2 events without rewriting old history. Replay dispatches each event independently, so legacy obligation propositions and `content_hash` values remain unchanged.

## Load pipeline

Nothing is published until all stages succeed:

1. read at most the hard archive-byte cap;
2. inspect only `format` and `format_version`;
3. deserialize the exact v1, v2 or v3 codec;
4. recompute that source version's `archive_hash`;
5. check revision/event/action counts against hard safety caps;
6. apply v1 → v2 → v3, v2 → v3, or the explicit v3 no-op;
7. require snapshot schema v3;
8. replay every event with its declared core semantics version;
9. reproduce persistent IDs, parents, event hashes, `content_hash` and status summaries;
10. recompute every frozen revision's `spec_hash` and semantic codec version;
11. return the workspace only after the entire graph is verified.

Unknown archive, snapshot or event-semantics versions are rejected. Serde defaults are not a migration mechanism. Source checksum verification always precedes migration, and count budgets precede event application.

## Save and migrate guarantees

`workspace.save` builds archive v3 in memory, checks its encoded size, writes a unique same-directory temporary file, flushes and `sync_all`s it, then atomically renames it. A failed write removes the temporary file and leaves the prior destination untouched where same-directory rename is atomic.

`workspace.migrate_archive` fully verifies and replays the source before checking/writing the destination. The source is never edited. Existing or identical destinations require `overwrite: true`; a failure creates no partial destination. `MigrationReport` records the verified source version/hash, target v3, every migration step and the new hash when written.

This remains local atomic replacement, not a concurrent database: there is no process lock, compare-and-swap generation, compression, encryption or directory `fsync`.

## Fixtures

`minimal-v1.json`, `saxpy-v1.json` and `minimal-v2.json` are immutable compatibility inputs. V3 fixtures cover a minimal workspace, SAXPY, mixed semantics, corrupted event semantics and a future v4 header. See `crates/agentir-store/tests/fixtures/README.md` for hashes and provenance.
