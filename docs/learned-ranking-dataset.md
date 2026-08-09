# Learned-ranking dataset

`RankingDataset` is an immutable ordered manifest plus `RankingExample` records. Each example keeps exact source episode and choice-set hashes, a policy-visible `RankingInput`, a separate historical `RankingLabel`, a stable semantic group identity, and an independent example hash.

Inputs contain only feature schema/version/hash, ordered exact choice IDs, visible feature vectors, compiler order, complete/bounded frame status, interaction mode, escape rights, and an allowlisted subset of already-visible task metadata. Labels may retain historical selection, acceptance, progress, proof-frontier delta, remaining decisions, repair, completion, semantic criterion, deterministic context/token cost, and explicitly artifact/device-anchored performance observation. Labels are not proof and are never passed to inference.

The leakage validator rejects forbidden or non-visible metadata, future label versions, label-derived fields, duplicated exact examples, malformed choice ordering, schema mismatch, missing required labels, corrupt hashes, and semantic group overlap across a split. Splitting is fixed-seed and assigns whole semantic groups to train, validation, test, or excluded; individual rows are never randomly split.

Generation is bounded, panic-free, offline, and deterministic. Raw datasets belong under `target/`, not in source control.
