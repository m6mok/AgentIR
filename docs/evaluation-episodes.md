# Evaluation episodes

An episode is an ordered state machine:

```text
ready → observation → decision → production compiler outcome
      → budget transition → next observation … → compiler-owned result
```

`episode.next` returns exactly one hashed observation. `episode.submit` must echo the run, episode, step, and observation hash. Stale/mismatched observations and policy violations fail before production state changes. A compiler rejection is recorded, but the production transaction path guarantees that its workspace/allocators remain unchanged.

Every `EpisodeStep` pairs one observation, decision, compiler outcome, context measurement, and optional correlation ID. Repair cycles begin at a rejection and close on the next accepted progress-producing action or episode end. The final `episode_hash` covers the corpus/task and policy anchors, seed, exact transcript, budget transitions, and compiler-derived result.

A ranked step additionally pairs one exact choice-set/schema anchor, `RankingTrace`, and `SelectionOutcome`. Ranking is read-only; only explicit selection invokes production mutation. Ranked transcripts use the explicit episode v2 hash domain while unranked Stage 6A episode hashes remain unchanged.
