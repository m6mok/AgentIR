# ScheduleIR

ScheduleIR is a separate typed graph anchored to exactly one `spec_hash`, `impl_hash`, `memory_hash`, and `target_hash`. Persistent plans and revisions use `sp*` and `sr*`; nodes, axes, operations, obligations, and evidence use schedule-local compiler IDs. Client transactions name an explicit base revision and all three expected physical/scheduling hashes.

Every schedule contains nodes in deterministic MemoryIR operation order, iteration domains, typed axes and transforms, dependency edges, restricted fusion groups, bindings, vector/unroll choices, compiler-owned legality facts, and an analytical resource estimate. The serial root executes every operation and logical coordinate exactly once and preserves reduction order.

`schedule_hash` uses the domain `agentir.schedule.exact.v1\0`. It covers the immutable anchors, exact ScheduleIR state, proof references, status, and resource estimate. It excludes interactive limits, timestamps, benchmark data, and runtime inputs. Apply, fork, and seal are immutable events; a rejected operation consumes no schedule-local ID and moves no head.

Stage 5 explicitly selects a proved revision for lowering. Kernel grouping, ABI, dispatch formulas, WGSL, device execution, and measurements live downstream in BackendIR/artifact/runtime stores; lowering leaves the selected `schedule_hash` byte-for-byte unchanged.
