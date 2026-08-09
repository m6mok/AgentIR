# External search protocol

The evaluation JSONL CLI exposes `evaluation.search.start`, `.advance`, `.status`, `.checkpoint`, `.resume`, `.cancel`, `.result`, and `.replay`. One physical line produces one structured response. Search commands are not core ActionIR.

Start names an exact task/corpus hash, existing scripted ranker, beam/depth/children/cadence, and receives compiler-owned root/objective/plan/ranker hashes. Later commands require exact objective and plan hashes. Start constructs the objective/plan server-side; clients never submit compiler choice IDs, source, guards, proofs or certificates. Advance is synchronously bounded in semantic units and starts no background thread.

Library search supports scripted and learned ranking. JSONL learned resume requires an archive-retained model; absent that anchor it returns a stable unsupported diagnostic. All search actions use isolated production engines and never mutate the evaluation protocol's ordinary live compiler sessions.
