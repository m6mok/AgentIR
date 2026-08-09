# Typed repairs

`RepairDescriptor` is a bounded compiler-owned description of an ordinary production request. Stable codes cover stale base, invalid reference, type mismatch, shape mismatch, open obligation, unsupported rewrite, unsafe memory reuse, illegal schedule transform, resource limit, unsupported backend lowering, ranking/schema/model mismatch, and stale continuation cursor.

Every repair anchors the exact diagnostic code and base revisions/hashes, declares a small action bound, and has an independent descriptor hash. An anchor change invalidates it. Validation does not guarantee compiler acceptance: the action still traverses the normal production decoder, verifier, and atomic transaction path.

Repair payloads containing agent-supplied proof, certificate, alias/lifetime proof, or guard fields reject. Free diagnostic prose can accompany a repair but is not a typed repair contract.
