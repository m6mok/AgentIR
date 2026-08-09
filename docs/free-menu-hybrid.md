# Free, menu, and hybrid

- `free` exposes the production request schema and visible references, but no ready-made continuation choices. Submitted schema-valid actions return to the normal verifier and atomic transaction path.
- `menu` exposes a bounded ordered compiler-generated continuation frame. The only valid decision names one returned `choice_id`; arbitrary actions and escape are rejected before compiler execution.
- `hybrid` exposes the same primary menu plus one bounded typed-action escape. An escaped action is not trusted and traverses the identical production decoder/verifier/transaction path.

Continuation order is deterministic and covered by `observation_hash` and `episode_hash`. No Stage 6A menu is an optimizer ranking or automatic selection mechanism.
