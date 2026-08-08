# Stage 5 compatibility audit

Baseline commit: `aa5133bed57dde8cc89f77887661aff042148223`.

Stage 5 does not change any legacy content/spec/impl/proposal/candidate/equality/memory/target/schedule hash codec. The existing `generic_gpu_v1` target remains `9df67ea24cadc12612bf6448d6e89a63a93c44a46aa64ff1cdfb277d7e7ac2d5`. New contracts use distinct domains documented in [backend-ir.md](backend-ir.md) and [artifact-package.md](artifact-package.md).

Pinned committed SHA-256 bytes for the immutable v8 corpus:

| Fixture | SHA-256 |
| --- | --- |
| `minimal-v8.json` | `c90b655b840e83b53a60ddbb5ef2508ea2c69f7b0092858d0954ae14f8425f39` |
| `target-generic-v8.json` | `c1beda9426992539df80fbe4368a055289cc94813051f4b3211a3b4fb9377872` |
| `schedule-serial-v8.json` | `d6fc6e31fe9945d5b7c34078d1bb250e0fe8848d3f4d0eef46ac720e30916f91` |
| `schedule-split-v8.json` / `schedule-remainder-v8.json` | `3477ef246cf750bf70882be6969c0df89e0d1350cf60201fb87e801c44d62cdb` |
| `schedule-tiled-v8.json` | `9b400945fcdd7fbd0ac144a74592bee82a4279dac68c04b29bd566e38f1cacaa` |
| `schedule-fused-v8.json` | `354a03074c7ffe65b6f39bbb09ee76dd098d163179cb7d75ac2e82d86d92820e` |
| `schedule-forked-v8.json` | `8a78c59da8c862032c2c662acc6fee3f65280be10a630c7dfd34f4928a1112c4` |
| `schedule-sealed-v8.json` | `6668b1c9aacdc5bb59fe2bb84bde29628deb722c55760ec099b1a807edb76c90` |
| `schedule-guarded-v8.json` | `a845b82e8cff5fed8f0ebddfd7b048646bc81c0d0eedb4f6c494510e6bc7e668` |

Corruption fixtures remain immutable as well: schedule certificate `d7c87fc63f3ce898673ce19fc38f72a7b82aef3e3c840e090b30ebccb3a02c63`, coverage `f1e19e9653dc2b83ef3bdf40c4b13c9be5a1268a965b0c752886526d59fee95b`, dependency `e67199ea96738c74c77193e59d762e8b04be018214f76b97a28aef2e1ef5710a`, hash `4ebb7234ef42f3a57dd1fa2bbf3146da7a612d58751992610c308f52f347f3ec`, memory anchor `a2be1684fcd9a47407818f9e76b51f295a2c050b18018d95f55a37ca5505f493`, resource `d14933f8ade37882aff9ade555facb723d071ca75af81195047a52dff7158ea9`, target capability `da0d6f0b40fa541df2fa8395fcf49a77c8032cb694fa01101fe6f192e76d8b8d`, and target hash `1e47c15c7b367452a1aac3433e77282f2881b7f878cfcd4e43338063819e920d`. Tests load v8 through the explicit v8→v9 migration and verify the legacy envelope before publication.
