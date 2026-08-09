# Measurement acquisition reconciliation

Reconciliation is a server-owned, zero-device observation of the production
measurement store. It verifies the journal and prepared-slot hashes, the exact
Stage 7C plan/session/slot order, workspace/root/device/build/runtime anchors,
and every record in the preparation snapshot. Only records appearing after
that boundary are candidates.

Each candidate is rehashed and must match artifact, target, compiler build,
device fingerprint, runtime, benchmark configuration, and validation policy.
Clients cannot nominate a record or supply timing, device, validation,
execution, outcome, or certificate metadata.

The deterministic classification is:

- zero compatible publications: `NoPublicationObserved`; no benchmark occurs;
- one compatible publication: first observed as
  `ExactlyOneCompatiblePublication`, then atomically attached as `Reconciled`;
- multiple compatible publications: `MultipleCompatiblePublications` and
  `Ambiguous`; no record is selected;
- incompatible publication or changed workspace/device/build/runtime anchor:
  a typed blocked result;
- corrupt/missing baseline or record: rejection with no caller-visible state or
  allocator mutation.

A complete Stage 7C slot cannot be reconciled twice. Reconciliation accepts no
executor and therefore cannot repeat hardware work.
