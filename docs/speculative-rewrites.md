# Speculative rewrites

`candidate.propose` replaces one top-level, single-result ImplIR operation. The request names an explicit candidate base revision, stale-state `expected_before_impl_hash`, target operation, boundary inputs and an ordered fragment. New fragment bindings begin with `$`; the core alpha-normalizes them before allocating persistent `iop*` and `iv*` IDs.

The boundary must list the target operands exactly and in order. Fragment operands may reference only boundary bindings or earlier fragment results. Operations use the existing pure ImplIR subset, optional regions use the existing typed closed-region model, and exactly one yielded value must have the target result type. Proposals cannot alter parameters, output names, constraints or `NumericContract`.

The core classifies the applied result without trusting `claimed_rule`:

- `legal`: an exact identity or production known rewrite is recognized;
- `conditional`: only the restricted guarded self-division shape is recognized;
- `unknown`: well typed, but no proof is known;
- `unsupported`: well typed structure outside the validator profile;
- `illegal`: malformed, ill typed, stale or over budget and rejected atomically.

Conditional, unknown and unsupported proposals require `allow_speculative: true`. Acceptance creates provenance plus ordered proof debt; it never creates correctness evidence. Rejection leaves the candidate head and every allocator unchanged.

## Proposal identity

`proposal_hash` uses domain `agentir.proposal.semantic.v1\0`. Its canonical model includes the base `impl_hash`, target, ordered normalized boundary, alpha-normalized fragment, output type, `NumericContract` and codec version. It excludes candidate/revision/evidence IDs, later allocated ImplIR IDs, time, resource policy and JSON map insertion order.

It is not interchangeable with `impl_hash`: the latter identifies reachable implementation semantics after application. It is also not `candidate_hash`, which identifies exact candidate history and proof state.
