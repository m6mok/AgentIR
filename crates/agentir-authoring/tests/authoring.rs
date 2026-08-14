use agentir_authoring::{
    AuthoringErrorCode, AuthoringGateway, ExecutionMode, GraphOpcode, GraphOperand, GraphOperation,
    GraphProposal, parse_proposal, parse_task,
};
use serde_json::json;

#[test]
fn default_instruction_names_schema_ambiguities_explicitly() {
    let instruction = agentir_authoring::DEFAULT_MODEL_INSTRUCTION;
    assert!(instruction.contains(r#""schema":"agentir.elementwise_graph.v1""#));
    assert!(instruction.contains("key op, never opcode"));
    assert!(instruction.contains("only supported op values are add, mul, and fma"));
    assert!(instruction.contains("Do not use shorthand operand objects"));
    assert!(instruction.contains("yield is a zero-based integer"));
    assert!(instruction.contains("Never replace fma with mul plus add"));
}

#[test]
fn unsupported_agent_opcode_is_rejected_instead_of_published() {
    let error = parse_proposal(
        r#"{"schema":"agentir.elementwise_graph.v1","operations":[{"op":"div","operands":[{"kind":"tensor","name":"numerator"},{"kind":"tensor","name":"denominator"}]}],"yield":0}"#,
    )
    .expect_err("unsupported opcode cannot enter the gateway");
    assert_eq!(error.code, AuthoringErrorCode::SchemaRejected);
    assert_eq!(error.path, "$.operations[0].op");
    assert_eq!(
        error.repair_hint.as_deref(),
        Some(agentir_authoring::DEFAULT_MODEL_INSTRUCTION)
    );
}

#[test]
fn proposal_operation_count_remains_bounded_at_128() {
    let operation = json!({
        "op":"mul",
        "operands":[
            {"kind":"tensor","name":"x"},
            {"kind":"tensor","name":"x"}
        ]
    });
    let accepted = json!({
        "schema":agentir_authoring::GRAPH_SCHEMA,
        "operations":vec![operation.clone();128],
        "yield":127
    });
    parse_proposal(&accepted.to_string()).expect("128 operations fit the local stress cap");

    let rejected = json!({
        "schema":agentir_authoring::GRAPH_SCHEMA,
        "operations":vec![operation;129],
        "yield":128
    });
    let error = parse_proposal(&rejected.to_string()).expect_err("129 operations exceed the cap");
    assert_eq!(error.code, AuthoringErrorCode::SchemaRejected);
    assert_eq!(error.path, "$.operations");
}

#[test]
fn observed_agent_schema_shortcuts_are_rejected_at_exact_paths() {
    let cases = [
        (
            r#"{"type":"agentir.elementwise_graph.v1","operations":[],"yield":{"operation":0}}"#,
            "$.type",
        ),
        (
            r#"{"schema":"agentir.elementwise_graph.v1","operations":[{"opcode":"fma","operands":[]}],"yield":0}"#,
            "$.operations[0].opcode",
        ),
        (
            r#"{"schema":"agentir.elementwise_graph.v1","operations":[{"op":"mul","operands":[{"scalar":"a"},{"tensor":"x"}]}],"yield":0}"#,
            "$.operations[0].operands[0].kind",
        ),
        (
            r#"{"schema":"agentir.elementwise_graph.v1","operations":[{"op":"mul","operands":[{"kind":"scalar","name":"a"},{"kind":"tensor","name":"x"}]}],"yield":{"operation":0}}"#,
            "$.yield",
        ),
    ];
    for (proposal, expected_path) in cases {
        let error = parse_proposal(proposal).expect_err("shortcut is not part of the wire schema");
        assert_eq!(error.code, AuthoringErrorCode::SchemaRejected);
        assert_eq!(error.path, expected_path);
        assert_eq!(
            error.repair_hint.as_deref(),
            Some(agentir_authoring::DEFAULT_MODEL_INSTRUCTION)
        );
    }
}

fn fixture() -> (
    agentir_authoring::AuthoringTask,
    agentir_authoring::GraphProposal,
) {
    (
        parse_task(include_str!(
            "../../../examples/authoring_task_two_term.json"
        ))
        .expect("valid task"),
        parse_proposal(include_str!(
            "../../../examples/authoring_proposal_two_term.json"
        ))
        .expect("valid proposal"),
    )
}

#[test]
fn graph_only_call_owns_envelope_ids_and_hashes() {
    let (task, proposal) = fixture();
    let result = AuthoringGateway::new()
        .publish(&task, &proposal, ExecutionMode::Portable)
        .expect("authoring succeeds");
    assert_eq!(result.outputs["out"], json!([32.0, 64.0, 96.0, 128.0]));
    assert_eq!(result.workspace, "w1");
    assert_eq!(result.revision, "r1");
    assert!(!result.spec_hash.is_empty());
    assert!(!result.cpu_artifact_hash.is_empty());
    assert_eq!(result.model_visible_calls, 1);
    assert_eq!(result.internal_agentir_requests, 10);
    assert!(!result.native_checked);
    assert!(
        result
            .transcript
            .iter()
            .all(|entry| entry.response["ok"] == true)
    );
}

#[test]
fn agent_trial_fma_graph_publishes_without_semantic_substitution() {
    let task = parse_task(
        r#"{
            "schema":"agentir.elementwise_authoring_task.v1",
            "task_id":"agent-trial-fma",
            "dimension":"N",
            "scalars":["scale"],
            "tensors":["signal","offset"],
            "inputs":{
                "scale":2.0,
                "signal":[1.0,2.0,3.0,4.0],
                "offset":[10.0,20.0,30.0,40.0]
            },
            "intent":{
                "schema":"agentir.elementwise_graph.v1",
                "operations":[{"op":"fma","operands":[
                    {"kind":"scalar","name":"scale"},
                    {"kind":"tensor","name":"signal"},
                    {"kind":"tensor","name":"offset"}
                ]}],
                "yield":0
            }
        }"#,
    )
    .expect("valid server task");
    let agent_proposal = parse_proposal(
        r#"{"schema":"agentir.elementwise_graph.v1","operations":[{"op":"fma","operands":[{"kind":"scalar","name":"scale"},{"kind":"tensor","name":"signal"},{"kind":"tensor","name":"offset"}]}],"yield":0}"#,
    )
    .expect("agent proposal follows exact schema");
    let result = AuthoringGateway::new()
        .publish(&task, &agent_proposal, ExecutionMode::Portable)
        .expect("exact FMA graph publishes");
    assert_eq!(result.outputs["out"], json!([12.0, 24.0, 36.0, 48.0]));
}

#[test]
fn agent_trial_dependency_graph_publishes_with_integer_local_references() {
    let task = parse_task(
        r#"{
            "schema":"agentir.elementwise_authoring_task.v1",
            "task_id":"agent-trial-dependency",
            "dimension":"N",
            "scalars":["p","q"],
            "tensors":["u","v","w"],
            "inputs":{
                "p":2.0,
                "q":3.0,
                "u":[1.0,2.0,3.0,4.0],
                "v":[10.0,20.0,30.0,40.0],
                "w":[2.0,2.0,2.0,2.0]
            },
            "intent":{
                "schema":"agentir.elementwise_graph.v1",
                "operations":[
                    {"op":"mul","operands":[{"kind":"scalar","name":"p"},{"kind":"tensor","name":"u"}]},
                    {"op":"mul","operands":[{"kind":"scalar","name":"q"},{"kind":"tensor","name":"v"}]},
                    {"op":"add","operands":[{"kind":"local","operation":0},{"kind":"local","operation":1}]},
                    {"op":"mul","operands":[{"kind":"local","operation":2},{"kind":"tensor","name":"w"}]}
                ],
                "yield":3
            }
        }"#,
    )
    .expect("valid server task");
    let agent_proposal = parse_proposal(
        r#"{"schema":"agentir.elementwise_graph.v1","operations":[{"op":"mul","operands":[{"kind":"scalar","name":"p"},{"kind":"tensor","name":"u"}]},{"op":"mul","operands":[{"kind":"scalar","name":"q"},{"kind":"tensor","name":"v"}]},{"op":"add","operands":[{"kind":"local","operation":0},{"kind":"local","operation":1}]},{"op":"mul","operands":[{"kind":"local","operation":2},{"kind":"tensor","name":"w"}]}],"yield":3}"#,
    )
    .expect("agent proposal follows exact schema");
    let result = AuthoringGateway::new()
        .publish(&task, &agent_proposal, ExecutionMode::Portable)
        .expect("dependency graph publishes");
    assert_eq!(result.outputs["out"], json!([64.0, 128.0, 192.0, 256.0]));
}

#[test]
fn wrong_intent_is_rejected_before_compiler_ids_are_consumed() {
    let (task, mut wrong) = fixture();
    wrong.operations[0].operands.swap(0, 1);
    let mut gateway = AuthoringGateway::new();
    let error = gateway
        .publish(&task, &wrong, ExecutionMode::Portable)
        .expect_err("wrong ordered intent is rejected");
    assert_eq!(error.code, AuthoringErrorCode::IntentRejected);
    assert_eq!(error.path, "$.operations[0].operands[0]");
    assert_eq!(error.expected, json!({"kind":"scalar","name":"a"}));
    assert_eq!(error.actual, json!({"kind":"tensor","name":"x"}));

    let accepted = gateway
        .publish(&task, &task.intent, ExecutionMode::Portable)
        .expect("valid proposal still receives first workspace ID");
    assert_eq!(accepted.workspace, "w1");
}

#[test]
fn intent_diagnostic_is_local_and_does_not_leak_the_complete_oracle() {
    let (task, mut wrong) = fixture();
    wrong.operations[0].operands.swap(0, 1);
    let error = AuthoringGateway::new()
        .publish(&task, &wrong, ExecutionMode::Portable)
        .expect_err("wrong intent remains unpublished");
    let diagnostic = serde_json::to_value(&error).expect("diagnostic JSON");
    assert_eq!(error.path, "$.operations[0].operands[0]");
    assert!(diagnostic.get("graph").is_none());
    assert!(diagnostic.get("operations").is_none());
    assert_ne!(error.expected, serde_json::to_value(&task.intent).unwrap());
}

#[test]
fn bad_local_reference_has_an_exact_path() {
    let (task, mut proposal) = fixture();
    proposal.operations[2].operands[0] = GraphOperand::Local { operation: 2 };
    let error = AuthoringGateway::new()
        .publish(&task, &proposal, ExecutionMode::Portable)
        .expect_err("forward local is rejected");
    assert_eq!(error.code, AuthoringErrorCode::ValidationRejected);
    assert_eq!(error.path, "$.operations[2].operands[0]");
}

#[test]
fn fma_cannot_be_substituted_with_mul_add() {
    let mut task = fixture().0;
    task.scalars = vec!["scale".to_owned()];
    task.tensors = vec!["signal".to_owned(), "offset".to_owned()];
    task.inputs = serde_json::from_value(json!({
        "scale": 1.000_000_119_209_289_6,
        "signal": [1.000_000_119_209_289_6],
        "offset": [-1.000_000_238_418_579]
    }))
    .expect("inputs");
    task.intent = GraphProposal {
        schema: agentir_authoring::GRAPH_SCHEMA.to_owned(),
        operations: vec![GraphOperation {
            op: GraphOpcode::Fma,
            operands: vec![
                GraphOperand::Scalar {
                    name: "scale".to_owned(),
                },
                GraphOperand::Tensor {
                    name: "signal".to_owned(),
                },
                GraphOperand::Tensor {
                    name: "offset".to_owned(),
                },
            ],
        }],
        r#yield: 0,
    };
    let substituted = GraphProposal {
        schema: agentir_authoring::GRAPH_SCHEMA.to_owned(),
        operations: vec![
            GraphOperation {
                op: GraphOpcode::Mul,
                operands: vec![
                    GraphOperand::Scalar {
                        name: "scale".to_owned(),
                    },
                    GraphOperand::Tensor {
                        name: "signal".to_owned(),
                    },
                ],
            },
            GraphOperation {
                op: GraphOpcode::Add,
                operands: vec![
                    GraphOperand::Local { operation: 0 },
                    GraphOperand::Tensor {
                        name: "offset".to_owned(),
                    },
                ],
            },
        ],
        r#yield: 1,
    };
    let error = AuthoringGateway::new()
        .publish(&task, &substituted, ExecutionMode::Portable)
        .expect_err("FMA substitution is rejected");
    assert_eq!(error.code, AuthoringErrorCode::IntentRejected);
}

#[test]
fn proposal_schema_denies_compiler_identity_fields() {
    let error = parse_proposal(
        r#"{
            "schema":"agentir.elementwise_graph.v1",
            "operations":[
                {"op":"mul","operands":[
                    {"kind":"tensor","name":"x"},
                    {"kind":"tensor","name":"x"}
                ]}
            ],
            "yield":0,
            "spec_hash":"guessed"
        }"#,
    )
    .expect_err("unknown compiler identity field is denied");
    assert_eq!(error.code, AuthoringErrorCode::SchemaRejected);
    assert_eq!(error.path, "$.spec_hash");
}

#[test]
fn compiler_artifacts_are_deterministic_across_fresh_sessions() {
    let (task, proposal) = fixture();
    let first = AuthoringGateway::new()
        .publish(&task, &proposal, ExecutionMode::Portable)
        .expect("first");
    let second = AuthoringGateway::new()
        .publish(&task, &proposal, ExecutionMode::Portable)
        .expect("second");
    assert_eq!(first.spec_hash, second.spec_hash);
    assert_eq!(first.cpu_artifact_hash, second.cpu_artifact_hash);
    assert_eq!(first.outputs, second.outputs);
}
