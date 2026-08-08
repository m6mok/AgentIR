# Deterministic schedule resource simulator

The Stage 4 simulator is an analytical legality oracle, not a performance model. Given exact ScheduleIR plus one TargetManifest, it deterministically computes grid/workgroup dimensions, threads and subgroups per workgroup, vector lanes, serial work, logical tiles and remainders, shared/private memory, live abstract memory, and global buffer references.

All arithmetic is checked or saturating and the work is centrally bounded. Any capacity violation rejects publication with a structured diagnostic. The result is recomputed during apply, check, replay, archive load, and scheduled evaluation; cached estimates must match exactly.

The simulator neither proves semantic equivalence by measurement nor predicts execution time. It contains no calibration database, device query, ranking, cost model, autotuner, or backend lowering.
