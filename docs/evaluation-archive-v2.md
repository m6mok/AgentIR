# Evaluation archive v2

New evaluation saves use `agentir.evaluation.archive` version 2 and domain `agentir.evaluation.archive.v2\0`. V2 adds feature schemas, ranking policy descriptors, exact choice sets, per-step ranking/selection records, and an explicit ranked/unranked status for every episode.

Evaluation archive v1 remains an immutable input. `migrate_archive_v1_to_v2` first verifies the exact v1 envelope and transcript, then changes only the envelope version, adds empty ranking stores, and marks every legacy episode `unranked`. It never invents scores, traces, selections, or outcomes. Loading performs the pure migration before ordinary replay.

Canonical JSON decoding uses exact IEEE-754 round-trip parsing. This is required because raw aggregate ratios participate in evaluation archive hashes; a one-ULP parser drift would otherwise make a freshly saved archive fail its own envelope verification. Metrics remain finite and never substitute for compiler proof.

V2 is an immutable legacy input. New saves use evaluation archive v4 and cross the explicit v2→v3→v4 migrations described in [evaluation-archive-v3.md](evaluation-archive-v3.md) and [evaluation-archive-v4.md](evaluation-archive-v4.md).
