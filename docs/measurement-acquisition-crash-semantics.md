# Measurement acquisition crash semantics

| Boundary | What is known | Required next action |
|---|---|---|
| Before durable prepare | Hardware was not authorized | Prepare the canonical slot |
| After prepare, before benchmark | No automatic execution is allowed | Reconcile; retry only after zero publication and explicit authorization |
| After benchmark, before publication | Hardware may have run; no complete record is visible | Reconcile; never create a numeric failure sentinel |
| After publication, before evaluation checkpoint | A complete record may exist while Stage 7C still shows the slot pending | Reconcile zero/one/multiple new publications |
| After checkpoint | The Stage 7C slot is complete | Ordinary resume skips it; reconciliation cannot repeat it |

These boundaries deliberately avoid an exactly-once claim. The journal proves
authorization and observation history, not whether physical device execution
occurred exactly once. V1 assumes one workspace and one writer.
