use agentir_protocol::Engine;
use serde_json::{Value, json};

fn request(engine: &mut Engine, value: &Value) -> Value {
    let line = engine.process_line(&value.to_string());
    serde_json::from_str(&line).expect("one JSON response")
}

fn frozen_constant_spec(engine: &mut Engine) {
    assert_eq!(
        request(
            engine,
            &json!({"command":"workspace.open","request_id":"open","workspace":"w1"})
        )["ok"],
        true
    );
    assert_eq!(
        request(
            engine,
            &json!({
                "command":"spec.apply",
                "request_id":"build",
                "workspace":"w1",
                "base_revision":"r0",
                "actions":[
                    {"kind":"create_constant","bind":"$x","type":"i32","value":2},
                    {"kind":"create_constant","bind":"$y","type":"i32","value":3},
                    {"kind":"create_op","bind":"$sum","opcode":"add","operands":["$x","$y"]},
                    {"kind":"set_output","name":"out","value":"$sum"}
                ]
            })
        )["ok"],
        true
    );
    assert_eq!(
        request(
            engine,
            &json!({
                "command":"spec.freeze",
                "request_id":"freeze",
                "workspace":"w1",
                "base_revision":"r1"
            })
        )["ok"],
        true
    );
}

#[test]
fn candidate_jsonl_identity_rewrite_validation_and_seal_are_structured() {
    let mut engine = Engine::new();
    frozen_constant_spec(&mut engine);
    let created = request(
        &mut engine,
        &json!({
            "command":"candidate.create",
            "request_id":"create",
            "workspace":"w1",
            "spec_revision":"r2"
        }),
    );
    assert_eq!(created["ok"], true, "{created}");
    assert_eq!(created["result"]["candidate"], "c1");
    assert_eq!(created["result"]["equivalence"]["status"], "proved");

    let continuation = request(
        &mut engine,
        &json!({
            "command":"candidate.continuation",
            "request_id":"continuation",
            "workspace":"w1",
            "candidate":"c1"
        }),
    );
    assert_eq!(continuation["result"]["matches"][0]["target"], "iop3");

    let unknown = request(
        &mut engine,
        &json!({
            "command":"candidate.apply",
            "request_id":"unknown-rule",
            "workspace":"w1",
            "candidate":"c1",
            "base_candidate_revision":"cr1",
            "actions":[{"kind":"apply_known_rewrite","rule":"invented","target":"iop3"}]
        }),
    );
    assert_eq!(unknown["ok"], false);
    assert_eq!(unknown["error"]["code"], "REWRITE_NOT_APPLICABLE");

    let rewritten = request(
        &mut engine,
        &json!({
            "command":"candidate.apply",
            "request_id":"rewrite",
            "workspace":"w1",
            "candidate":"c1",
            "base_candidate_revision":"cr1",
            "actions":[{
                "kind":"apply_known_rewrite",
                "rule":"fold_defined_scalar_constants",
                "target":"iop3",
                "expected_before_impl_hash":created["result"]["impl_hash"]
            }]
        }),
    );
    assert_eq!(rewritten["ok"], true, "{rewritten}");
    assert_eq!(rewritten["result"]["candidate_revision"], "cr2");

    let validated = request(
        &mut engine,
        &json!({
            "command":"candidate.validate",
            "request_id":"validate",
            "workspace":"w1",
            "candidate":"c1",
            "base_candidate_revision":"cr2",
            "seed":17,
            "cases":8
        }),
    );
    assert_eq!(validated["ok"], true, "{validated}");
    assert_eq!(validated["result"]["confidence_evidence"], 1);
    assert_eq!(validated["result"]["equivalence"]["status"], "proved");

    let sealed = request(
        &mut engine,
        &json!({
            "command":"candidate.seal",
            "request_id":"seal",
            "workspace":"w1",
            "candidate":"c1",
            "base_candidate_revision":"cr3"
        }),
    );
    assert_eq!(sealed["ok"], true, "{sealed}");
    assert_eq!(sealed["result"]["state"], "sealed");

    let malformed = engine.process_line(
        r#"{"command":"candidate.check","request_id":"strict","workspace":"w1","candidate":"c1","unknown":true}"#,
    );
    let malformed: Value = serde_json::from_str(&malformed).unwrap();
    assert_eq!(malformed["ok"], false);
    assert_eq!(malformed["error"]["code"], "INVALID_REQUEST");
}

#[test]
fn speculative_debt_refutation_and_guarded_fallback_are_end_to_end() {
    let mut engine = Engine::new();
    assert_eq!(
        request(
            &mut engine,
            &json!({"command":"workspace.open","request_id":"open","workspace":"w2"})
        )["ok"],
        true
    );
    assert_eq!(
        request(
            &mut engine,
            &json!({
                "command":"spec.apply","request_id":"build","workspace":"w2","base_revision":"r0",
                "actions":[
                    {"kind":"create_parameter","bind":"$x","name":"x","type":"i32"},
                    {"kind":"create_parameter","bind":"$y","name":"y","type":"i32"},
                    {"kind":"create_op","bind":"$sum","opcode":"add","operands":["$x","$y"]},
                    {"kind":"set_output","name":"out","value":"$sum"}
                ]
            })
        )["ok"],
        true
    );
    assert_eq!(
        request(
            &mut engine,
            &json!({"command":"spec.freeze","request_id":"freeze","workspace":"w2","base_revision":"r1"})
        )["ok"],
        true
    );
    let identity = request(
        &mut engine,
        &json!({"command":"candidate.create","request_id":"create","workspace":"w2","spec_revision":"r2"}),
    );
    let equivalent_but_unknown = request(
        &mut engine,
        &json!({
            "command":"candidate.propose","request_id":"proposal","workspace":"w2","candidate":"c1",
            "base_candidate_revision":"cr1","target":"iop3",
            "expected_before_impl_hash":identity["result"]["impl_hash"],"allow_speculative":true,
            "replacement":{
                "inputs":[{"bind":"$x","value":"iv1"},{"bind":"$y","value":"iv2"}],
                "operations":[
                    {"bind":"$sum","opcode":"add","operands":["$x","$y"]},
                    {"bind":"$zero","opcode":"constant","operands":[],"constant":{"type":"i32","value":0}},
                    {"bind":"$same","opcode":"add","operands":["$sum","$zero"]}
                ],
                "result":{"value":"$same"}
            }
        }),
    );
    assert_eq!(
        equivalent_but_unknown["ok"], true,
        "{equivalent_but_unknown}"
    );
    assert_eq!(equivalent_but_unknown["result"]["state"], "speculative");

    let passed = request(
        &mut engine,
        &json!({
            "command":"candidate.validate","request_id":"validate","workspace":"w2","candidate":"c1",
            "base_candidate_revision":"cr2","seed":17,"cases":12
        }),
    );
    assert_eq!(passed["ok"], true, "{passed}");
    assert_eq!(passed["result"]["equivalence"]["status"], "open");
    assert_eq!(passed["result"]["proof_debt"][0]["status"], "open");

    let unsupported = request(
        &mut engine,
        &json!({
            "command":"candidate.translation_check","request_id":"translation","workspace":"w2",
            "candidate":"c1","base_candidate_revision":"cr3","proposal":"p1"
        }),
    );
    assert_eq!(unsupported["ok"], true, "{unsupported}");
    assert_eq!(
        unsupported["result"]["diagnostic"],
        "TRANSLATION_UNSUPPORTED"
    );
    assert_eq!(
        unsupported["result"]["candidate"]["proof_debt"][0]["status"],
        "unsupported"
    );
    let blocked_seal = request(
        &mut engine,
        &json!({
            "command":"candidate.seal","request_id":"seal","workspace":"w2","candidate":"c1",
            "base_candidate_revision":"cr4"
        }),
    );
    assert_eq!(blocked_seal["error"]["code"], "CANDIDATE_HAS_PROOF_DEBT");

    let second = request(
        &mut engine,
        &json!({"command":"candidate.create","request_id":"create2","workspace":"w2","spec_revision":"r2"}),
    );
    let continuation = request(
        &mut engine,
        &json!({"command":"candidate.continuation","request_id":"cont","workspace":"w2","candidate":"c2"}),
    );
    let escape = &continuation["result"]["speculative_escape"];
    let incorrect = request(
        &mut engine,
        &json!({
            "command":"candidate.propose","request_id":"bad-proposal","workspace":"w2","candidate":"c2",
            "base_candidate_revision":second["result"]["candidate_revision"],
            "target":escape["target"],"expected_before_impl_hash":second["result"]["impl_hash"],
            "allow_speculative":true,
            "replacement":{
                "inputs":[
                    {"bind":"$x","value":escape["boundary_inputs"][0]},
                    {"bind":"$y","value":escape["boundary_inputs"][1]}
                ],
                "operations":[{"bind":"$wrong","opcode":"sub","operands":["$x","$y"]}],
                "result":{"value":"$wrong"}
            }
        }),
    );
    assert_eq!(incorrect["ok"], true, "{incorrect}");
    let refuted = request(
        &mut engine,
        &json!({
            "command":"candidate.validate","request_id":"refute","workspace":"w2","candidate":"c2",
            "base_candidate_revision":incorrect["result"]["candidate_revision"],"seed":1,"cases":32
        }),
    );
    assert_eq!(refuted["ok"], true, "{refuted}");
    assert_eq!(refuted["result"]["state"], "rejected");
    assert_eq!(refuted["result"]["proof_debt"][0]["status"], "refuted");
    let refuted_seal = request(
        &mut engine,
        &json!({
            "command":"candidate.seal","request_id":"refuted-seal","workspace":"w2","candidate":"c2",
            "base_candidate_revision":refuted["result"]["candidate_revision"]
        }),
    );
    assert_eq!(refuted_seal["error"]["code"], "OBLIGATION_REFUTED");

    assert_eq!(
        request(
            &mut engine,
            &json!({"command":"workspace.open","request_id":"guard-open","workspace":"guard"})
        )["ok"],
        true
    );
    request(
        &mut engine,
        &json!({
            "command":"spec.apply","request_id":"guard-build","workspace":"guard","base_revision":"r0",
            "actions":[
                {"kind":"create_parameter","bind":"$x","name":"x","type":"i32"},
                {"kind":"create_op","bind":"$div","opcode":"div","operands":["$x","$x"]},
                {"kind":"set_output","name":"out","value":"$div"}
            ]
        }),
    );
    request(
        &mut engine,
        &json!({"command":"spec.freeze","request_id":"guard-freeze","workspace":"guard","base_revision":"r1"}),
    );
    let guard_identity = request(
        &mut engine,
        &json!({"command":"candidate.create","request_id":"guard-create","workspace":"guard","spec_revision":"r2"}),
    );
    let guard_proposal = request(
        &mut engine,
        &json!({
            "command":"candidate.propose","request_id":"guard-proposal","workspace":"guard","candidate":"c1",
            "base_candidate_revision":"cr1","target":"iop2",
            "expected_before_impl_hash":guard_identity["result"]["impl_hash"],"allow_speculative":true,
            "replacement":{
                "inputs":[{"bind":"$lhs","value":"iv1"},{"bind":"$rhs","value":"iv1"}],
                "operations":[{"bind":"$one","opcode":"constant","operands":[],"constant":{"type":"i32","value":1}}],
                "result":{"value":"$one"}
            }
        }),
    );
    let guarded = request(
        &mut engine,
        &json!({
            "command":"candidate.translation_check","request_id":"guard-check","workspace":"guard",
            "candidate":"c1","base_candidate_revision":guard_proposal["result"]["candidate_revision"],"proposal":"p1"
        }),
    );
    assert_eq!(guarded["result"]["candidate"]["state"], "guarded");
    for value in [1_i32, -1, i32::MIN, i32::MAX, 7, -113] {
        let nonzero = request(
            &mut engine,
            &json!({"command":"candidate.evaluate","request_id":format!("nonzero-{value}"),"workspace":"guard","candidate":"c1","inputs":{"x":value}}),
        );
        assert_eq!(nonzero["result"]["outputs"]["out"], 1, "{nonzero}");
    }
    let zero = request(
        &mut engine,
        &json!({"command":"candidate.evaluate","request_id":"zero","workspace":"guard","candidate":"c1","inputs":{"x":0}}),
    );
    assert_eq!(zero["error"]["code"], "DIVISION_BY_ZERO");
    let guarded_validation = request(
        &mut engine,
        &json!({"command":"candidate.validate","request_id":"guard-validate","workspace":"guard","candidate":"c1","base_candidate_revision":"cr3","seed":29,"cases":16}),
    );
    assert_eq!(guarded_validation["ok"], true, "{guarded_validation}");
    assert_eq!(guarded_validation["result"]["state"], "guarded");
    let sealed_guard = request(
        &mut engine,
        &json!({"command":"candidate.seal","request_id":"guard-seal","workspace":"guard","candidate":"c1","base_candidate_revision":"cr4"}),
    );
    assert_eq!(sealed_guard["result"]["state"], "sealed");
}
