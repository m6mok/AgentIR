# Persistence and replay

AgentIR persistence is split across two layers. `agentir-core` owns the semantic snapshot and event replay but performs no I/O. `agentir-store` owns the versioned JSON archive, checksum and atomic filesystem replacement.

## Archive version 1

The top-level envelope contains:

```text
format = "agentir.workspace"
format_version = 1
compiler_version
snapshot
archive_hash = sha256(canonical archive body)
```

The snapshot contains its own schema version, workspace/head IDs, every immutable revision, compiler allocator state and ordered events. Transaction events retain complete ActionIR input plus expected compiler transaction ID, child revision ID and content hash. Fork events retain parent, child and hash.

Archives larger than 64 MiB are rejected by the Stage 1 local store.

## Save guarantees

`workspace.save` serializes to a uniquely named temporary file in the destination directory, flushes and `sync_all`s it, then renames it over the requested path. A failed write removes its temporary file and leaves the prior destination untouched where the host filesystem provides atomic same-directory rename semantics.

This is atomic replacement, not a concurrent database: there is no process lock, compare-and-swap generation or directory `fsync` in version 1.

## Load verification

Before an archive becomes visible to the protocol engine, loading performs all of these checks:

1. format discriminator and version are supported;
2. archive body SHA-256 matches `archive_hash`;
3. every transaction and explicit fork replays through the normal compiler core;
4. replay reproduces persistent IDs, revision parents, program state and hashes;
5. each archived program hash is independently recomputed;
6. each cached status summary is recomputed;
7. replayed head and persistent allocator counters match the snapshot.

Original timestamps and ephemeral continuation counter state are restored only after semantic replay succeeds.

## CLI lifecycle

Run one process to construct and save a workspace, then start a new process and call `workspace.load`. A successful result includes `metadata` and `replay`. `workspace.verify_archive` performs the same validation without adding the workspace to the process.

The archive contains full graphs and event history, so it may include model-generated names, constants and provenance. Treat it as project data and apply normal filesystem access controls.
