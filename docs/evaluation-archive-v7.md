# Evaluation archive v7

Evaluation archive v7 retains verified Stage 7D recovery provenance in addition
to all v6 acquisition data:

- recovery journals and their work counters;
- immutable prepared slots and publication snapshots;
- reconciliation results, including ambiguous and abandoned outcomes;
- retry authorizations;
- recovery checkpoints and zero-device replay statuses.

V6→v7 first verifies the complete v6 archive, changes only the evaluation
manifest/version/hash domain, and records `NoRecoveryHistory`. It invents no
journal, snapshot, attempt, reconciliation, retry, measurement, or work record.
The deterministic load chain is v1→v2→v3→v4→v5→v6→v7.

New evaluation archives use `agentir.evaluation.archive.v7\0`. Evaluation and
workspace archive versioning remain independent: workspace archive v9 and
`HardwareMeasurementRecord` v1 are unchanged. Archive verification resolves
every referenced production measurement and replays recovery without an
executor or device access.
