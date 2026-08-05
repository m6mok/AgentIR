use agentir_protocol::Engine;
use serde_json::{Value, json};

#[allow(clippy::needless_pass_by_value)]
fn send(engine: &mut Engine, request: Value) -> Value {
    let response: Value = serde_json::from_str(&engine.process_line(&request.to_string())).unwrap();
    assert_eq!(response["ok"], true, "{response}");
    response["result"].clone()
}

#[allow(clippy::needless_pass_by_value)]
fn send_error(engine: &mut Engine, request: Value) -> Value {
    let response: Value = serde_json::from_str(&engine.process_line(&request.to_string())).unwrap();
    assert_eq!(response["ok"], false, "{response}");
    response
}

fn setup(engine: &mut Engine) -> Value {
    send(
        engine,
        json!({"command":"workspace.open","request_id":"open","workspace":"w1"}),
    );
    send(
        engine,
        json!({
            "command":"spec.apply",
            "request_id":"build",
            "workspace":"w1",
            "base_revision":"r0",
            "actions":[
                {"kind":"create_constant","bind":"$a","type":"i32","value":2},
                {"kind":"create_constant","bind":"$b","type":"i32","value":3},
                {"kind":"create_constant","bind":"$c","type":"i32","value":4},
                {"kind":"create_constant","bind":"$d","type":"i32","value":5},
                {"kind":"create_op","bind":"$left","opcode":"add","operands":["$a","$b"]},
                {"kind":"create_op","bind":"$right","opcode":"mul","operands":["$c","$d"]},
                {"kind":"create_op","bind":"$total","opcode":"add","operands":["$left","$right"]},
                {"kind":"set_output","name":"out","value":"$total"}
            ]
        }),
    );
    send(
        engine,
        json!({
            "command":"spec.freeze",
            "request_id":"freeze",
            "workspace":"w1",
            "base_revision":"r1"
        }),
    );
    send(
        engine,
        json!({
            "command":"candidate.create",
            "request_id":"candidate",
            "workspace":"w1",
            "spec_revision":"r2"
        }),
    )
}

fn create_and_saturate(engine: &mut Engine) -> (Value, Value) {
    let root = send(
        engine,
        json!({
            "command":"equality.create",
            "request_id":"equality-create",
            "workspace":"w1",
            "candidate":"c1",
            "candidate_revision":"cr1"
        }),
    );
    let saturated = send(
        engine,
        json!({
            "command":"equality.saturate",
            "request_id":"saturate",
            "workspace":"w1",
            "equality_space":"eqs1",
            "base_equality_revision":"er1",
            "expected_equality_hash":root["equality_hash"],
            "fuel":100
        }),
    );
    (root, saturated)
}

#[test]
fn equality_jsonl_saturates_explains_evaluates_and_materializes() {
    let mut engine = Engine::new();
    setup(&mut engine);
    let (root, saturated) = create_and_saturate(&mut engine);
    assert_eq!(saturated["status"], "fixed_point");
    assert_eq!(saturated["node_count"], 5);
    assert_eq!(saturated["edge_count"], 5);

    let explanation = send(
        &mut engine,
        json!({
            "command":"equality.explain",
            "request_id":"explain",
            "workspace":"w1",
            "equality_space":"eqs1",
            "equality_revision":"er2",
            "node":"en5"
        }),
    );
    assert_eq!(explanation["edges"].as_array().unwrap().len(), 3);
    let evaluated = send(
        &mut engine,
        json!({
            "command":"equality.evaluate",
            "request_id":"evaluate",
            "workspace":"w1",
            "equality_space":"eqs1",
            "equality_revision":"er2",
            "node":"en5",
            "inputs":{}
        }),
    );
    assert_eq!(evaluated["outputs"]["out"], 25);
    let stale_materialization = send_error(
        &mut engine,
        json!({
            "command":"equality.materialize",
            "request_id":"stale-materialize",
            "workspace":"w1",
            "equality_space":"eqs1",
            "equality_revision":"er2",
            "expected_equality_hash":"stale",
            "node":"en5"
        }),
    );
    assert_eq!(
        stale_materialization["error"]["code"],
        "EQUALITY_HASH_MISMATCH"
    );
    let materialized = send(
        &mut engine,
        json!({
            "command":"equality.materialize",
            "request_id":"materialize",
            "workspace":"w1",
            "equality_space":"eqs1",
            "equality_revision":"er2",
            "expected_equality_hash":saturated["equality_hash"],
            "node":"en5"
        }),
    );
    assert_eq!(materialized["candidate"], "c2");

    let stale = send_error(
        &mut engine,
        json!({
            "command":"equality.expand",
            "request_id":"stale",
            "workspace":"w1",
            "equality_space":"eqs1",
            "base_equality_revision":"er1",
            "expected_equality_hash":root["equality_hash"],
            "fuel":1
        }),
    );
    assert_eq!(stale["error"]["code"], "STALE_EQUALITY_BASE");
}

#[test]
fn speculative_debt_is_discharged_only_by_core_built_equality_path() {
    let mut engine = Engine::new();
    let identity = setup(&mut engine);
    let (_root, saturated) = create_and_saturate(&mut engine);
    let proposal = send(
        &mut engine,
        json!({
            "command":"candidate.propose",
            "request_id":"propose",
            "workspace":"w1",
            "candidate":"c1",
            "base_candidate_revision":"cr1",
            "target":"iop7",
            "replacement":{
                "inputs":[
                    {"bind":"$left","value":"iv3"},
                    {"bind":"$right","value":"iv6"}
                ],
                "operations":[{
                    "bind":"$constant",
                    "opcode":"constant",
                    "operands":[],
                    "constant":{"type":"i32","value":25}
                }],
                "result":{"value":"$constant"}
            },
            "expected_before_impl_hash":identity["impl_hash"],
            "allow_speculative":true
        }),
    );
    assert_eq!(proposal["state"], "speculative");
    let evaluated = send(
        &mut engine,
        json!({
            "command":"equality.evaluate",
            "request_id":"evaluate-before-proof",
            "workspace":"w1",
            "equality_space":"eqs1",
            "equality_revision":"er2",
            "node":"en5",
            "inputs":{}
        }),
    );
    assert_eq!(evaluated["outputs"]["out"], 25);
    let still_open = send(
        &mut engine,
        json!({
            "command":"candidate.check",
            "request_id":"still-open",
            "workspace":"w1",
            "candidate":"c1",
            "candidate_revision":"cr2"
        }),
    );
    assert_eq!(still_open["sealable"], false);

    let mismatch = send_error(
        &mut engine,
        json!({
            "command":"candidate.equality_check",
            "request_id":"wrong-target",
            "workspace":"w1",
            "candidate":"c1",
            "base_candidate_revision":"cr2",
            "proposal":"p1",
            "equality_space":"eqs1",
            "equality_revision":"er2",
            "expected_equality_hash":saturated["equality_hash"],
            "target_node":"en2"
        }),
    );
    assert_eq!(mismatch["error"]["code"], "EQUALITY_PROOF_INVALID");
    let unchanged = send(
        &mut engine,
        json!({
            "command":"candidate.query",
            "request_id":"unchanged",
            "workspace":"w1",
            "candidate":"c1"
        }),
    );
    assert_eq!(unchanged["head"], "cr2");

    let discharged = send(
        &mut engine,
        json!({
            "command":"candidate.equality_check",
            "request_id":"discharge",
            "workspace":"w1",
            "candidate":"c1",
            "base_candidate_revision":"cr2",
            "proposal":"p1",
            "equality_space":"eqs1",
            "equality_revision":"er2",
            "expected_equality_hash":saturated["equality_hash"],
            "target_node":"en5"
        }),
    );
    assert_eq!(discharged["candidate_revision"], "cr3");
    let sealed = send(
        &mut engine,
        json!({
            "command":"candidate.seal",
            "request_id":"seal",
            "workspace":"w1",
            "candidate":"c1",
            "base_candidate_revision":"cr3"
        }),
    );
    assert_eq!(sealed["state"], "sealed");
}
