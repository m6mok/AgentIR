# Measurement cohorts

`MeasurementCohort` v1 uses `agentir.evaluation.measurement_cohort.v1\0`. Canonical order is measurement-hash order and is independent of request order; duplicate hashes reject.

Every retained `HardwareMeasurementRecord` is rehashed with the production `agentir.measurement.hardware.v1\0` contract and must reference a retained offline-valid artifact. All records share exact target, compiler build, device fingerprint, runtime version, warmups, iterations, input distribution, and tensor dimensions. The cohort records one validation policy, an equal positive record count per artifact, and either `single_record_summary_v1` or `median_of_record_summaries_v1`.

`hardware_executed_v1` accepts only `offline_validated_and_device_executed`. `synthetic_fixture_v1` is reserved for explicitly labelled test/study records and is never performance evidence. A missing terminal record is `Unavailable` with a typed reason, never zero or an integer sentinel.
