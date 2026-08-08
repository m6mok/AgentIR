# Artifact package

An artifact package contains exact WGSL module bytes, entry points, the complete runtime binding ABI, dispatch and guard plans, output mappings, immutable Stage 1–5 anchors, compiler build identity, offline validation report and an `ArtifactEquivalentToBackend` certificate.

`artifact_hash` uses `agentir.artifact.wgsl.package.v1\0`. It changes when WGSL, the manifest, ABI or compiler build contract changes. It is independent of device discovery, execution results, measurements and resource policy. Packages are immutable after publication and archive replay rechecks their hashes, module/entry-point consistency, ABI against BackendIR and certificate.
