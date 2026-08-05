# Compact constraint solving

Stage 1.2 uses a deterministic, sound and deliberately incomplete fact engine. Its job is not to solve arbitrary integer arithmetic; it supplies cheap proof facts to type/shape inference and rechecks structured `ShapeCompatible` obligations whenever a new constraint is accepted.

## Supported facts

`ConstraintFacts` supports:

- equality of static extents and contradiction of different static extents;
- plain symbol equivalence such as `N == M`;
- symbol-to-static binding such as `N == 4`;
- transitive equivalence and static propagation;
- normalization of one-symbol affine terms `k*N+c` after symbol/static substitution;
- declared-symbol `NonNegative` facts;
- whole-shape equality in ordered dimension/rank order.

Symbols must already be declared. Shapes are parsed into the typed `DimExpr` model, so unsupported multi-symbol, nonlinear, divisibility or inequality expressions never enter the fact engine. A directly accepted affine equality proves that exact ordered relation (and its symmetric form); other affine consequences that compact normalization cannot derive remain `unknown`.

## Determinism

Symbol classes choose the lexicographically smallest representative. Collections that affect results or diagnostics are `BTreeMap`/`BTreeSet`; duplicate constraints are fact-level and current-program no-ops. Proof and contradiction evidence lists accepted facts in canonical order. Reordering facts or alpha-renaming all symbols preserves the proof classification.

## Soundness before completeness

The query result is exactly one of:

- `proved`: equality follows from normalized accepted facts;
- `contradiction`: normalized static values, ranks or equal-coefficient affine offsets conflict;
- `unknown`: the engine lacks sufficient compact evidence.

`unknown` is never interpreted as false and never closes proof debt. This is why Stage 1.2 can use a bounded brute-force oracle as a one-way property: every `proved` result must hold for every enumerated satisfying assignment, while the oracle may find true statements that the solver leaves unknown.

## Obligation lifecycle

A new current-semantics shape obligation stores the relation kind, left/right types and shapes, involved symbols, operation-or-hole context and action provenance. It does not rely on parsing a human message.

1. Inference queries current facts.
2. A proved relation is legal immediately and creates no shape obligation.
3. An unknown relation is conditional and creates an open structured obligation.
4. `add_constraint` stages the new fact, checks all existing facts and open structured obligations, then closes only relations now proved.
5. If the fact makes an accepted obligation contradictory, the entire transaction is rejected with `CONSTRAINT_CONTRADICTION`.
6. `spec.check` observes persistent discharged state; `spec.freeze` rechecks facts and still rejects any blocking open obligation.

Legacy semantics-v1 events retain their original unstructured proposition and non-discharging `AddConstraint` behavior so historical `content_hash` values replay exactly.

## Deferred reasoning

Stage 1.2 has no SMT integration, Presburger solver, general affine elimination, inequalities, divisibility, nonlinear expressions or optimization over constraints. Those features require a separate capability and proof contract rather than silently widening `ConstraintFacts`.
