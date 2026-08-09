# Stage 7B readiness

## Audit

- Ownership: cohort/objective/recommendation/search-wrapper/archive-v5 code exists only in `agentir-policy-eval`; workspace v9 and measurement-record v1 are unchanged.
- Trust: JSONL accepts only existing measurement IDs/hashes from a bound production workspace and rejects client timing/device data.
- Eligibility: exact same target/build/device/runtime/config/distribution/dimensions/status/count is mandatory; missing data is typed unavailable.
- Determinism: canonical hash order, lower-median integer aggregation, checked ppm comparison, artifact-hash ties, frozen Stage 7A ordering, and zero hardware calls during replay.
- Legacy: evaluation v1–v4 verify as immutable inputs and migrate explicitly through v4→v5 with no invented measured history.
- Study: scripted/learned beam widths 1/2/4, median/p95 objectives, three proved/offline-valid artifacts of one `spec_hash`, normal/indifference cohorts, structured rejection, two byte-identical runs, and a structured real-device skip.

## Remaining limits and Stage 7C blockers

Stage 7B is menu-only, sequential, terminal-only, and summary-record based. It has no continuation-native engine snapshot, concurrent/new algorithm, live acquisition loop, raw-sample significance analysis, energy record, predictor/interpolation, cross-device model, training during search, or live publication. Those require separately versioned Stage 7C-or-later contracts.

## Verdict

Ready for the narrow offline contract when the full repository gate and two-run comparator pass. Synthetic study records prove deterministic contract behavior only. In the absence of the optional same-device acquisition study there is no real hardware-performance evidence. This verdict makes no correctness, global-optimality, statistical-significance, portability, or full-Stage-7 completion claim.
