# Iteration domains

The core derives iteration domains from verified ImplIR value shapes and MemoryIR operation mappings. Clients cannot declare domains or axes. Each logical tensor dimension receives a typed data-parallel axis; reduction dimensions remain explicit reduction axes. Scalar operations have an empty domain.

A root axis records its operation, dimension, role, exact static or symbolic extent, serial baseline, and transform ancestry. Split and tile replace an active parent with ordered outer/inner axes. Static divisible extents use exact coverage; static non-divisible and symbolic extents attach a compiler-owned remainder domain. Traversal and canonicalization use compiler ID order and ordered node axes.

Verification rebuilds domains from immutable anchors, checks that active leaves cover the root domain exactly once, and rejects missing coverage, duplicate execution, invalid ancestry, or reordered reduction axes.
