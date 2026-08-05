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
