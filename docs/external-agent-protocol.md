# External agent protocol

The `agentir-eval` CLI is a stateful one-line-in/one-line-out JSONL transport. It supports:

```text
evaluation.corpus.list       evaluation.task.query
evaluation.run.start         evaluation.run.status
evaluation.run.cancel        evaluation.episode.query
evaluation.episode.next      evaluation.episode.submit
evaluation.episode.finish    evaluation.transcript.query
evaluation.aggregate         evaluation.compare
evaluation.archive.save      evaluation.archive.load
evaluation.replay
```

`evaluation.episode.submit` accepts run/episode/step IDs, the exact observation hash, either a menu choice or typed action, optional token provenance, and an optional external correlation ID. Unknown fields are rejected. It never accepts success, metrics, rejection class, compiler outcome, correctness certificate, harness hashes, or hidden state.

Example scripted start:

```json
{"command":"evaluation.run.start","request_id":"run","policy":"free_reference_v1","tasks":["saxpy-end-to-end-large"],"seeds":[0],"scripted":true}
```

External runs set `scripted:false`, choose `kind`, call `episode.next`, and submit against the returned hash.
