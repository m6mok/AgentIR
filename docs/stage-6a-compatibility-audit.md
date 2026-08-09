# Stage 6A compatibility audit

The Stage 6A implementation started from clean commit `240d03a8dc3103fbe18c7fbc1e2218fedab6839e` (`feat: complete Stage 5 WGSL backend`). The audit confirmed workspace archive v9 as current output, immutable v1–v8 migration inputs, event/compiler semantics axes, benchmark schema v2, deterministic continuation ordering, and centralized Stage 1–5 limits.

No file under `crates/agentir-core` or `crates/agentir-store/tests/fixtures` changed. Existing domains for SpecIR, ImplIR, proposal/candidate/equality, MemoryIR, TargetManifest/ScheduleIR, BackendIR/build/artifact/device/measurement, content, and workspace archive remain untouched. Stage 6A domains exist only in `agentir-policy-eval`.

Before editing, SHA-256 was recorded for every filename matching `*-v8.json` or `*-v9.json` (53 files). The sorted `shasum -a 256` manifest itself hashes to:

```text
aaf955a718b3af4c659c20ab8c4b4d8ac7237d44976dc08eb792c1c6cd8563d6
```

The same command after implementation is byte-identical (`cmp` success). Workspace tests additionally re-verified all pinned legacy fixtures, migrations, corruptions, allocator continuations, hashes, certificates, and event replay.

The only package-boundary compatibility adjustment is naming: the existing CPU oracle directory is package `agentir-reference-eval` with unchanged Rust library crate name `agentir_eval`; the new executable package owns the requested `agentir-eval` CLI name. Existing Rust imports and behavior remain unchanged.
