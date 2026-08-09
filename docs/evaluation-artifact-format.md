# Evaluation artifact format

`agentir.evaluation.archive` version 1 is separate from workspace archive v9. It stores an `EvaluationManifest`, exact corpus, policy descriptors, compiler build hash, optional source workspace hashes, ordered runs/episodes/transcripts/outcomes, raw aggregates, optional hardware anchors, and `archive_hash`.

The archive uses domain `agentir.evaluation.archive.v1\0`. Loading bounds bytes before parse and verifies envelope, corpus, policy, observation, episode, evaluation, and aggregate hashes; unique IDs; contiguous steps; observation/decision pairing; and aggregate identity. Replay of compiler outcomes is an explicit offline operation.

The format excludes API keys, environment secrets, hidden chain-of-thought, arbitrary provider payloads, mutable model sessions, native caches, and machine-specific benchmark output.

This v1 codec is immutable. New saves use evaluation archive v2; see [evaluation-archive-v2.md](evaluation-archive-v2.md).
