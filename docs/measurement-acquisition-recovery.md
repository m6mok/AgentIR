# Measurement acquisition recovery

`MeasurementAcquisitionRecoveryJournal` v1 protects one canonical Stage 7C
slot. `prepare` verifies the session/catalog/store, records the current
production measurement IDs and hashes, freezes plan/session/workspace/root and
artifact/target/device/build/runtime/config anchors, and assigns an attempt ID.
It performs no benchmark or device call.

Only `execute` accepts an executor. It requires `Prepared` or
`RetryAuthorized`; every other state forbids execution. A crash boundary before
completion produces `IndeterminateAfterCrash` and never schedules an automatic
rerun. A publication and Stage 7C progress update are staged atomically where a
single store permits it. Across independent durable stores, uncertainty is
represented explicitly and resolved by reconciliation.

If reconciliation proves no publication exists, a separate
`authorize_retry` operation snapshots the store again and creates a new attempt
ID and trace event. The earlier attempt remains immutable. `abandon` is the only
way to stop an unresolved attempt without accepting or retrying it.

The independent domains are:

- `agentir.evaluation.measurement_acquisition_recovery_journal.v1\0`;
- `agentir.evaluation.measurement_acquisition_prepared_slot.v1\0`;
- `agentir.evaluation.measurement_acquisition_reconciliation.v1\0`.

Paths, request IDs, wall time, resource limits and work counters do not enter
these semantic identities.
