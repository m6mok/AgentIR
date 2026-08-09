# Stage 7D scope

Stage 7D is the evaluation-only durable recovery layer for a single Stage 7C
measurement-acquisition slot. It records an exact prepared attempt before
hardware is authorized, classifies a crash as indeterminate, observes the
production measurement store after restart, and either reconciles one exact
compatible publication or requires an explicit operator decision.

## Included in v1

- one single-writer recovery journal for one canonical slot;
- an exact server-owned pre-publication ID/hash snapshot;
- explicit prepare, execute, reconcile, retry authorization and abandon;
- safe fault injection before benchmark, after benchmark, after publication,
  and after the evaluation checkpoint;
- zero-device checkpoint restore, status, archive verification and replay;
- evaluation archive v7 and pure v6→v7 migration;
- deterministic synthetic crash/restart readiness evidence.

## Explicit non-claims

Stage 7D does not prove exactly-once hardware execution. A benchmark may have
run before a process or device failure became visible. It proves only that an
indeterminate attempt is not silently rerun, that at most one measurement is
accepted for the Stage 7C slot, and that an already-published compatible record
can be recovered without another benchmark.

Measurements remain non-correctness observations. Recovery cannot advance a
compiler proof, legalize an artifact, change search/ranking, or establish
performance superiority, portability, statistical significance, or global
optimality.

Concurrent writers, remote workers, distributed transactions, multi-device
pooling, automatic retry, live tuning, prediction, training, and new search or
ranking algorithms are outside v1.
