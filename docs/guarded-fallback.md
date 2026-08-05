# Guarded fallback

Stage 2B supports one conditional rule:

```text
i32 div(x, x) -> constant i32 1
guard: x != 0
fallback: exact proved parent candidate revision
```

The validator requires scalar `i32`, identical operand IDs and an exact constant-one primary result. It creates `I32NonZero` itself. The fallback is an immutable, fully proved `EquivalentToSpec` revision in the same CandidateForest with the same `spec_hash` and interface. The graph is cycle checked and recursion depth bounded. No agent-provided guard or general guard expression language exists.

Evaluation first binds inputs and computes only the dependency cone needed for `x != 0`. A true guard evaluates only the primary candidate. A false guard evaluates only the fallback; therefore `x = 0` preserves the original division-by-zero result instead of eagerly executing the constant primary and hiding the error. Guard-dependency errors follow the exact parent behavior.

This remains exact `EquivalentToSpec` because every input is routed either through a conditionally valid primary or through an already proved implementation. Differential tests cover both reachable branches but remain confidence evidence. A counterexample refutes the guarded obligation and blocks sealing and publication of corrupted archives.
