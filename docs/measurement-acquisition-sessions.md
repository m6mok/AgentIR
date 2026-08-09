# Measurement acquisition sessions

A session is published only after server-owned artifact checks and device/runtime preflight. Slots retain plan/round/index/artifact/config/target/build/device/runtime anchors. A successful slot exists only after one complete compiler-assigned measurement ID/hash is atomically published; failure retains no partial record or numeric sentinel.

Advance stages the session and measurement store together and commits only at a full slot boundary. Checkpoints freeze the completed canonical prefix and exact measurement anchors. Resume verifies checkpoint/plan/workspace/artifact/record/order/device/build/runtime/config state before hardware work. Cancellation is cooperative between slots. The in-memory wrapper makes record publication and session progress one atomic assignment; independent filesystem exactly-once recovery is not claimed, and an unprovable external crash must be classified `IndeterminateAfterCrash`, never silently rerun.
