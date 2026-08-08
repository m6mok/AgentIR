# Scheduled MemoryIR safety

ScheduleIR may reorder only where compiler-owned dependencies and MemoryIR facts permit it. It never edits buffers, layouts, aliases, lifetimes, reuse decisions, or guards; `memory_hash` therefore remains unchanged across schedule revisions.

The verifier maps every scheduled node back to one MemoryIR operation, rebuilds data dependencies, preserves access order where aliasing could matter, and rejects schedules that invalidate last-use, escape, or overlap facts. Vector access additionally checks exact stride and alignment. Fresh allocation remains the exact repair when reuse is unsafe.

For guarded MemoryIR, scheduled execution preserves the same compiler-owned `NoOverlap` predicate and evaluates only the selected exact branch. The false branch lazily uses the proved fresh allocation template. Schedule tests and traces are confidence evidence only; structural compiler certificates establish equivalence to MemoryIR.
