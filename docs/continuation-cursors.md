# Continuation cursors

The v1 continuation envelope pages an already verified exact production choice set. It retains exact locator/revision/hash anchors, stable enumeration kind, configured total and work limits, returned count, complete/bounded semantics, exhausted status, opaque compiler-owned cursor, cursor version, continuation digest, deterministic work counters, and exact choices.

Choice IDs are assigned before pagination, so page size cannot affect identity. Concatenating pages is identical to the bounded one-shot ordering with no duplicates or omissions. The cursor contains a versioned canonical payload and independent digest; it contains no pointer, secret, timestamp, host, session, or machine state.

Corrupt/future cursors and cursor/anchor, workspace, run, layer, mutation, choice-set, or total-count mismatch reject before a page is published. Rejected compiler mutations leave the anchor valid; accepted anchor mutations make the prior cursor stale. Repeating the same resume is deterministic.
