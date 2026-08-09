# External search protocol

The evaluation JSONL CLI exposes `evaluation.search.start`, `.advance`, `.status`, `.checkpoint`, `.resume`, `.cancel`, `.result`, and `.replay`. One physical line produces one structured response. Search commands are not core ActionIR.

Stage 7B adds `evaluation.measurement_cohort.create/query` and `evaluation.measured_search.start/advance/status/checkpoint/resume/cancel/result/replay`. Cohort creation accepts only measurement IDs/hashes from the production workspace bound to the evaluation protocol. `deny_unknown_fields` rejects latency summaries, device fingerprints, validation statuses, arbitrary artifact hashes, backend source, guards, or certificates supplied by the client. Measured commands retain exact cohort/objective anchors and report zero benchmark/device calls during search and replay.

Start names an exact task/corpus hash, existing scripted ranker, beam/depth/children/cadence, and receives compiler-owned root/objective/plan/ranker hashes. Later commands require exact objective and plan hashes. Start constructs the objective/plan server-side; clients never submit compiler choice IDs, source, guards, proofs or certificates. Advance is synchronously bounded in semantic units and starts no background thread.

Library search supports scripted and learned ranking. JSONL learned resume requires an archive-retained model; absent that anchor it returns a stable unsupported diagnostic. All search actions use isolated production engines and never mutate the evaluation protocol's ordinary live compiler sessions.
