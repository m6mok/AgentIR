# Measurement acquisition replay

## Stage 7D recovery replay

Recovery replay verifies the journal, every immutable prepared slot, publication
snapshot, reconciliation result, retry authorization, Stage 7C session anchor,
and referenced production record. Its API deliberately has no executor. The
verified work projection records `replay_hardware_calls = 0`; any retained
non-zero value rejects. Archive-v7 verification uses the same zero-device path.

Replay rehashes the plan, checkpoint, trace, result and every referenced completed production-format record; checks canonical slot order, anchors, counts and duplicate absence; and reproduces the terminal result hash and work contract.

The replay API has no acquisition executor parameter. Benchmark, adapter, device, provider, network and training calls are exactly zero. Frozen timing observations are verified, not remeasured. Missing, corrupt, duplicate or stale records reject before replay publication.
