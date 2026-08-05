# JSONL protocol

The `agentir` CLI accepts one JSON object per stdin line and emits exactly one response line. Command names are coarse-grained; opcodes are data inside transactions, not separate tools.

## Response envelope

Success:

```json
{"ok":true,"request_id":"q1","result":{},"diagnostics":[]}
```

Failure:

```json
{"ok":false,"request_id":"q1","error":{"code":"TYPE_MISMATCH","message":"operand types differ","expected":"f32","actual":"bool"},"diagnostics":[]}
```

Clients should branch on stable `code`, not message text.

## References

- `$name` — temporary binding visible only later in the same transaction;
- `vN`, `hN`, `dN` — persistent compiler-assigned IDs;
- `@vN` — persistent value in short-reference form;
- `@name` — current parameter or output name.
- `@N` — one-based index in the revision's deterministic live-value table.

Temporary bindings never survive commit. A successful response maps them to persistent IDs.
Inside a type string, `[$N]` is accepted as codec sugar and immediately canonicalized to `[N]`; the named dimension must already have been declared by an earlier action.

## Build transaction

```json
{
  "command": "spec.apply",
  "request_id": "build",
  "workspace": "w1",
  "base_revision": "r0",
  "actions": [
    {"kind":"create_parameter","bind":"$x","name":"x","type":"f32"},
    {"kind":"create_constant","bind":"$one","type":"f32","value":1.0},
    {"kind":"create_op","bind":"$out","opcode":"add","operands":["$x","$one"]},
    {"kind":"set_output","name":"out","value":"$out"}
  ]
}
```

An optional `allow_branch: true` explicitly permits a transaction based on a non-head revision. Without it, the response is `BASE_REVISION_CONFLICT`.

## Regions

`map`, `zip_map` and `reduce` accept an inline `region` with typed `arguments`, an explicit `captures` list, local `operations`, and `yield_value`. Region op bindings also start with `$`, but use a namespace local to the region.

```json
{
  "arguments":[{"name":"xi","type":"f32"}],
  "captures":["$scale"],
  "operations":[
    {"bind":"$product","opcode":"mul","operands":["$scale","xi"]}
  ],
  "yield_value":"$product"
}
```

## Commands

- `workspace.open`: optional `workspace`; returns root `r0`.
- `workspace.save`: workspace and destination `path`; writes an atomic versioned archive.
- `workspace.load`: archive `path` and optional `replace`; verifies and replays before inserting.
- `workspace.verify_archive`: verifies checksum and replay without retaining the workspace.
- `spec.apply` and `transaction.apply`: workspace, base revision, actions and optional client transaction ID.
- `spec.check`: optional revision, default head.
- `spec.freeze`: base revision; commits `freeze_spec` as a new revision.
- `program.query`: optional revision and `view: summary | canonical`.
- `program.evaluate`: optional revision and exact parameter-name `inputs`.
- `revision.fork`: `base_revision`.
- `revision.diff`: `from` and `to`.
- `continuation.get`: revision, hole and `mode: free | menu | hybrid`.

The complete SAXPY command sequence is [examples/saxpy.jsonl](../examples/saxpy.jsonl).

## Save and resume

Save the current workspace:

```json
{"command":"workspace.save","request_id":"save","workspace":"w1","path":"/tmp/saxpy.agentir.json"}
```

In a fresh CLI process, restore it:

```json
{"command":"workspace.load","request_id":"load","path":"/tmp/saxpy.agentir.json"}
```

The result contains archive metadata and a replay report. `replace` defaults to `false`, so an archive cannot silently overwrite an already open workspace with the same ID.
