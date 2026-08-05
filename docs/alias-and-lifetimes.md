# Alias and logical lifetimes

Alias and lifetime facts are compiler-owned. Alias relations are `must_alias`, `no_alias`, `may_alias`, and `partial_overlap`; provenance distinguishes type, region construction, lifetime, external contract, runtime guard, and unverified claim. An `unverified_claim` is audit metadata only and cannot authorize reuse.

Lifetimes use canonical logical MemoryIR operation order: first bind/definition point, ordered use points, last use, output escape, external lifetime, and deallocation eligibility. This deliberately makes no ScheduleIR or concurrency claim. Output escape keeps a result live; external storage is never plan-released.

Read-only queries expose buffer records and alias facts without mutation. All stride, extent, alignment, allocation-size, and total-allocation arithmetic is checked or saturating at the resource boundary. A missing proof yields a stable diagnostic and a fresh-plan repair, not an unsafe assumption.
