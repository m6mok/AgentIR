# Schedule legality

Legality is compiler-owned and positive: acceptance means the core reconstructed and verified the exact transformed schedule. Lack of a supported proof is rejection, not permission based on testing.

- Split and tile factors are positive and preserve complete, non-duplicated coverage. Non-divisible or symbolic extents receive a compiler-owned exact remainder.
- Fusion is limited to a single-user, dependent pointwise producer/consumer pair with identical domains.
- Reduction axes remain serial, preserving their fixed order.
- Bindings are unique and checked against the target hierarchy.
- Vectorization requires a supported scalar type and width plus compatible innermost stride and alignment for every accessed buffer.
- Unrolling is bounded and applies only to non-reduction axes.
- The rebuilt dependency graph and MemoryIR alias/lifetime facts must remain valid.

`schedule.legality_query` stages the requested action and returns its failed stable side condition without publication. `schedule.apply` performs the same checks atomically and also validates target resources.
