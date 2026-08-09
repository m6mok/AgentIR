# Measured search replay

Replay first performs ordinary Stage 7A replay in fresh isolated production engines. It then rehashes retained production measurement records, rechecks cohort equality constraints and validation policy, extracts compiler-owned terminal artifact hashes from retained production outcomes, repeats integer aggregation/indifference/ties, and verifies the recommendation hash.

Replay performs no benchmark, GPU, adapter, provider, network, or training call. Corrupt/missing/stale/cross-device records reject before publication. Failed resume or replay consumes no compiler IDs and mutates neither caller workspace nor retained search state. Learned resume requires the archive-retained model anchor exactly as Stage 7A does.
