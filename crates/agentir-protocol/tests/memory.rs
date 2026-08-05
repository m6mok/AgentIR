use agentir_protocol::Engine;
use serde_json::{Value, json};

#[allow(clippy::needless_pass_by_value)]
fn request(engine: &mut Engine, value: Value) -> Value {
    let response = engine.process_line(&value.to_string());
    let parsed: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(parsed["ok"], true, "{response}");
    parsed["result"].clone()
}

fn prepared_engine() -> (Engine, Value) {
    let mut engine = Engine::new();
    request(
        &mut engine,
        json!({"command":"workspace.open","request_id":"open","workspace":"w1"}),
    );
    request(
        &mut engine,
        json!({
            "command":"spec.apply","request_id":"build","workspace":"w1","base_revision":"r0",
            "actions":[
                {"kind":"define_dimension","bind":"$N","name":"N","constraints":["N >= 0"]},
                {"kind":"create_parameter","bind":"$x","name":"x","type":"tensor<f32,[$N]>"},
                {"kind":"create_op","bind":"$temporary","opcode":"map","operands":["$x"],"region":{"arguments":[{"name":"element","type":"f32"}],"captures":[],"operations":[],"yield_value":"element"}},
                {"kind":"create_op","bind":"$out","opcode":"map","operands":["$temporary"],"region":{"arguments":[{"name":"element","type":"f32"}],"captures":[],"operations":[],"yield_value":"element"}},
                {"kind":"set_output","name":"out","value":"$out"}
            ]
        }),
    );
    request(
        &mut engine,
        json!({"command":"spec.freeze","request_id":"freeze","workspace":"w1","base_revision":"r1"}),
    );
    request(
        &mut engine,
        json!({"command":"candidate.create","request_id":"candidate","workspace":"w1","spec_revision":"r2"}),
    );
    let memory = request(
        &mut engine,
        json!({"command":"memory.create","request_id":"memory","workspace":"w1","candidate":"c1","candidate_revision":"cr1"}),
    );
    (engine, memory)
}

#[test]
fn memory_jsonl_reuses_evaluates_queries_and_rejects_stale_hashes() {
    let (mut engine, memory) = prepared_engine();
    let query = &memory["query"];
    let continuation = request(
        &mut engine,
        json!({"command":"memory.continuation","request_id":"continue","workspace":"w1","memory_plan":"mp1","memory_revision":"mr1"}),
    );
    assert_eq!(
        continuation["reuse_choices"][1]["statically_applicable"],
        true
    );
    let reused = request(
        &mut engine,
        json!({
            "command":"memory.apply","request_id":"reuse","workspace":"w1","memory_plan":"mp1",
            "base_memory_revision":"mr1","expected_memory_hash":query["memory_hash"],
            "expected_impl_hash":query["impl_hash"],
            "actions":[{"kind":"request_in_place_reuse","input":"iv2","result":"iv3"}]
        }),
    );
    assert_eq!(reused["query"]["reused_buffer_count"], 1);
    let evaluation = request(
        &mut engine,
        json!({"command":"memory.evaluate","request_id":"evaluate","workspace":"w1","memory_plan":"mp1","memory_revision":"mr2","inputs":{"x":[1.0,2.0,3.0]}}),
    );
    assert_eq!(
        evaluation["evaluation"]["outputs"]["out"],
        json!([1.0, 2.0, 3.0])
    );
    assert!(
        evaluation["trace"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "reuse")
    );

    let stale = engine.process_line(
        &json!({
            "command":"memory.apply","request_id":"stale","workspace":"w1","memory_plan":"mp1",
            "base_memory_revision":"mr1","expected_memory_hash":query["memory_hash"],
            "expected_impl_hash":query["impl_hash"],
            "actions":[{"kind":"set_alignment","buffer":"buf2","alignment":8}]
        })
        .to_string(),
    );
    let stale: Value = serde_json::from_str(&stale).unwrap();
    assert_eq!(stale["ok"], false);
    assert_eq!(stale["error"]["code"], "STALE_MEMORY_BASE");
}

#[test]
fn guarded_memory_evaluation_takes_only_the_selected_physical_branch() {
    let (mut engine, memory) = prepared_engine();
    let query = &memory["query"];
    request(
        &mut engine,
        json!({
            "command":"memory.apply","request_id":"guard","workspace":"w1","memory_plan":"mp1",
            "base_memory_revision":"mr1","expected_memory_hash":query["memory_hash"],
            "expected_impl_hash":query["impl_hash"],
            "actions":[{"kind":"request_guarded_reuse","input":"iv2","result":"iv3","guard_against":"buf1"}]
        }),
    );
    let primary = request(
        &mut engine,
        json!({"command":"memory.evaluate","request_id":"primary","workspace":"w1","memory_plan":"mp1","memory_revision":"mr2","inputs":{"x":[4.0,5.0]},"guard_outcomes":{"mg1":true}}),
    );
    let fallback = request(
        &mut engine,
        json!({"command":"memory.evaluate","request_id":"fallback","workspace":"w1","memory_plan":"mp1","memory_revision":"mr2","inputs":{"x":[4.0,5.0]},"guard_outcomes":{"mg1":false}}),
    );
    assert_eq!(primary["evaluation"], fallback["evaluation"]);
    assert!(
        primary["trace"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "guarded_reuse")
    );
    assert!(
        !primary["trace"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "fallback_allocate")
    );
    assert!(
        fallback["trace"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "fallback_allocate")
    );
    assert!(
        !fallback["trace"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["kind"] == "guarded_reuse")
    );
}
