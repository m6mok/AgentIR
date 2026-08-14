use agentir_authoring::{
    AuthoringErrorCode, AuthoringGateway, AuthoringPayload, AuthoringSurface, ExecutionMode,
    GRAPH_JSON_SCHEMA, GRAPH_SCHEMA, GraphOpcode, GraphOperand, IncrementalBatch,
    IncrementalOperand, IncrementalOperation, IncrementalSession, IncrementalTransaction,
    STAGED_JSON_SCHEMA, STAGED_SCHEMA, StagedOperand, StagedOperation, StagedProposal,
    TRANSACTION_JSON_SCHEMA, TRANSACTION_SCHEMA, compile_authoring_payload,
    compile_incremental_batch, compile_staged, parse_authoring_payload, parse_incremental_batch,
    parse_proposal, parse_staged, parse_task, parse_transaction,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn task() -> agentir_authoring::AuthoringTask {
    parse_task(include_str!(
        "../../../examples/authoring_task_two_term.json"
    ))
    .expect("valid task")
}

fn i_op(bind: &str, operands: Vec<IncrementalOperand>) -> IncrementalOperation {
    IncrementalOperation {
        bind: bind.to_owned(),
        op: GraphOpcode::Add,
        operands,
    }
}

fn i_local(name: &str) -> IncrementalOperand {
    IncrementalOperand::Local {
        name: name.to_owned(),
    }
}

fn simple_transaction(base_operations: usize, bind: &str) -> IncrementalTransaction {
    IncrementalTransaction {
        schema: TRANSACTION_SCHEMA.to_owned(),
        base_operations,
        operations: vec![IncrementalOperation {
            bind: bind.to_owned(),
            op: GraphOpcode::Mul,
            operands: vec![
                IncrementalOperand::Scalar {
                    name: "a".to_owned(),
                },
                IncrementalOperand::Tensor {
                    name: "x".to_owned(),
                },
            ],
        }],
    }
}

fn staged_one_op(stages: usize) -> StagedProposal {
    StagedProposal {
        schema: STAGED_SCHEMA.to_owned(),
        stages,
        seed: GraphOperand::Tensor {
            name: "x".to_owned(),
        },
        body: vec![StagedOperation {
            bind: "$state".to_owned(),
            op: GraphOpcode::Add,
            operands: vec![
                StagedOperand::StatePrev,
                StagedOperand::Tensor {
                    name: "y".to_owned(),
                },
            ],
        }],
        state: "$state".to_owned(),
    }
}

#[test]
fn public_dispatcher_lowers_all_three_bare_payloads_to_one_graph() {
    let task = task();
    let cases = [
        (
            AuthoringSurface::Graph,
            include_str!("../../../examples/authoring_proposal_two_term.json"),
        ),
        (
            AuthoringSurface::IncrementalBatch,
            include_str!("../../../examples/authoring_incremental_two_term.json"),
        ),
        (
            AuthoringSurface::Staged,
            include_str!("../../../examples/authoring_staged_two_term.json"),
        ),
    ];
    for (surface, text) in cases {
        let explicit = parse_authoring_payload(text, Some(surface)).expect("explicit parse");
        let detected = parse_authoring_payload(text, None).expect("schema dispatch");
        assert_eq!(explicit, detected);
        assert_eq!(explicit.surface(), surface);
        assert_eq!(
            compile_authoring_payload(&task, &explicit).expect("lowered graph"),
            task.intent
        );
        let result = AuthoringGateway::new()
            .publish_payload(&task, &explicit, ExecutionMode::Portable)
            .expect("ordinary publication path");
        assert_eq!(result.outputs["out"], json!([32.0, 64.0, 96.0, 128.0]));
        assert_eq!(result.workspace, "w1");
    }
}

#[test]
fn auto_detection_and_parsers_report_exact_schema_paths() {
    let malformed = parse_transaction("{not-json").expect_err("malformed JSON");
    assert_eq!(malformed.path, "$");
    assert_eq!(malformed.code, AuthoringErrorCode::SchemaRejected);

    let unknown = parse_transaction(
        r#"{"schema":"agentir.elementwise_transaction.v1","base_operations":0,"operations":[{"bind":"$x","op":"add","operands":[],"compiler_id":"v7"}]}"#,
    )
    .expect_err("unknown nested field");
    assert_eq!(unknown.path, "$.operations[0].compiler_id");

    let staged_unknown = parse_staged(
        r#"{"schema":"agentir.elementwise_staged.v1","stages":1,"seed":{"kind":"tensor","name":"x"},"body":[{"bind":"$x","op":"add","operands":[{"kind":"state_prev"},{"kind":"tensor","name":"y","type":"f32"}]}],"state":"$x"}"#,
    )
    .expect_err("unknown operand field");
    assert_eq!(staged_unknown.path, "$.body[0].operands[1].type");

    let wrong_surface = parse_authoring_payload(
        include_str!("../../../examples/authoring_staged_two_term.json"),
        Some(AuthoringSurface::Graph),
    )
    .expect_err("explicit dialect is authoritative");
    assert_eq!(wrong_surface.path, "$.schema");
    assert_eq!(wrong_surface.expected, json!(GRAPH_SCHEMA));

    let unknown_schema = parse_authoring_payload(r#"{"schema":"v1"}"#, None)
        .expect_err("unknown schema is not fallback-parsed");
    assert_eq!(unknown_schema.path, "$.schema");

    let wrong_transaction = parse_transaction(
        r#"{"schema":"agentir.elementwise_transaction.v2","base_operations":0,"operations":[]}"#,
    )
    .expect_err("wrong transaction version");
    assert_eq!(wrong_transaction.path, "$.schema");
    let wrong_staged = parse_staged(
        r#"{"schema":"agentir.elementwise_staged.v2","stages":1,"seed":{"kind":"tensor","name":"x"},"body":[],"state":"$x"}"#,
    )
    .expect_err("wrong staged version");
    assert_eq!(wrong_staged.path, "$.schema");
}

#[test]
fn empty_and_over_limit_wire_programs_are_rejected() {
    let empty_transaction = parse_transaction(
        r#"{"schema":"agentir.elementwise_transaction.v1","base_operations":0,"operations":[]}"#,
    )
    .expect_err("empty transaction");
    assert_eq!(empty_transaction.path, "$.operations");

    let operation = json!({
        "bind":"$x",
        "op":"add",
        "operands":[{"kind":"tensor","name":"x"},{"kind":"tensor","name":"y"}]
    });
    let over_transaction = json!({
        "schema":TRANSACTION_SCHEMA,
        "base_operations":0,
        "operations":vec![operation;9]
    });
    assert_eq!(
        parse_transaction(&over_transaction.to_string())
            .expect_err("nine operations")
            .path,
        "$.operations"
    );

    let empty_batch = parse_incremental_batch(
        r#"{"schema":"agentir.elementwise_incremental_batch.v1","transactions":[],"yield":"$x"}"#,
    )
    .expect_err("empty batch");
    assert_eq!(empty_batch.path, "$.transactions");

    let empty_body = parse_staged(
        r#"{"schema":"agentir.elementwise_staged.v1","stages":1,"seed":{"kind":"tensor","name":"x"},"body":[],"state":"$x"}"#,
    )
    .expect_err("empty body");
    assert_eq!(empty_body.path, "$.body");

    let body_op = json!({
        "bind":"$x",
        "op":"add",
        "operands":[{"kind":"state_prev"},{"kind":"tensor","name":"y"}]
    });
    let over_body = json!({
        "schema":STAGED_SCHEMA,
        "stages":1,
        "seed":{"kind":"tensor","name":"x"},
        "body":vec![body_op;9],
        "state":"$x"
    });
    assert_eq!(
        parse_staged(&over_body.to_string())
            .expect_err("nine body operations")
            .path,
        "$.body"
    );
}

#[test]
fn incremental_bindings_bases_and_transactions_are_atomic() {
    let mut session = IncrementalSession::new(vec!["a".to_owned()], vec!["x".to_owned()]);
    session
        .apply(&simple_transaction(0, "$first"))
        .expect("first transaction");
    let before = session.finish("$first").expect("graph before rejection");

    let bad = IncrementalTransaction {
        schema: TRANSACTION_SCHEMA.to_owned(),
        base_operations: 1,
        operations: vec![i_op("$new", vec![i_local("$future"), i_local("$first")])],
    };
    let error = session.apply(&bad).expect_err("forward/unknown binding");
    assert_eq!(error.path, "$.operations[0].operands[0]");
    assert_eq!(session.operation_count(), 1);
    assert_eq!(session.finish("$first").unwrap(), before);
    assert!(session.finish("$new").is_err());

    for binding in ["first", "$9bad", "$"] {
        let invalid = simple_transaction(1, binding);
        assert_eq!(
            session.apply(&invalid).expect_err("invalid binding").path,
            "$.operations[0].bind"
        );
    }
    let duplicate = simple_transaction(1, "$first");
    assert_eq!(
        session
            .apply(&duplicate)
            .expect_err("duplicate binding")
            .path,
        "$.operations[0].bind"
    );

    let sequences = [
        vec![simple_transaction(0, "$a"), simple_transaction(0, "$b")],
        vec![simple_transaction(0, "$a"), simple_transaction(2, "$b")],
        vec![simple_transaction(1, "$b"), simple_transaction(0, "$a")],
    ];
    for transactions in sequences {
        let batch = IncrementalBatch {
            schema: agentir_authoring::INCREMENTAL_BATCH_SCHEMA.to_owned(),
            transactions,
            r#yield: "$b".to_owned(),
        };
        let error = compile_incremental_batch(&batch, vec!["a".to_owned()], vec!["x".to_owned()])
            .expect_err("duplicate, gap, or reorder");
        assert!(error.path.ends_with(".base_operations"));
    }
}

#[test]
fn invalid_arity_and_missing_final_references_are_local() {
    let mut session = IncrementalSession::new(Vec::<String>::new(), vec!["x".to_owned()]);
    let arity = IncrementalTransaction {
        schema: TRANSACTION_SCHEMA.to_owned(),
        base_operations: 0,
        operations: vec![IncrementalOperation {
            bind: "$bad".to_owned(),
            op: GraphOpcode::Fma,
            operands: vec![IncrementalOperand::Tensor {
                name: "x".to_owned(),
            }],
        }],
    };
    assert_eq!(
        session.apply(&arity).expect_err("bad arity").path,
        "$.operations[0].operands"
    );

    let mut staged_arity = staged_one_op(1);
    staged_arity.body[0].op = GraphOpcode::Fma;
    assert_eq!(
        compile_staged(&staged_arity)
            .expect_err("bad staged arity")
            .path,
        "$.body[0].operands"
    );

    let missing_yield = parse_incremental_batch(
        r#"{"schema":"agentir.elementwise_incremental_batch.v1","transactions":[{"schema":"agentir.elementwise_transaction.v1","base_operations":0,"operations":[{"bind":"$x","op":"add","operands":[{"kind":"tensor","name":"x"},{"kind":"tensor","name":"x"}]}]}]}"#,
    )
    .expect_err("missing yield");
    assert_eq!(missing_yield.path, "$.yield");

    let batch = IncrementalBatch {
        schema: agentir_authoring::INCREMENTAL_BATCH_SCHEMA.to_owned(),
        transactions: vec![simple_transaction(0, "$x")],
        r#yield: "$missing".to_owned(),
    };
    assert_eq!(
        compile_incremental_batch(&batch, vec!["a".to_owned()], vec!["x".to_owned()])
            .expect_err("unknown yield")
            .path,
        "$.yield"
    );

    let missing_state = parse_staged(
        r#"{"schema":"agentir.elementwise_staged.v1","stages":1,"seed":{"kind":"tensor","name":"x"},"body":[{"bind":"$x","op":"add","operands":[{"kind":"state_prev"},{"kind":"tensor","name":"x"}]}]}"#,
    )
    .expect_err("missing state");
    assert_eq!(missing_state.path, "$.state");

    let mut staged = staged_one_op(1);
    staged.state = "$missing".to_owned();
    assert_eq!(
        compile_staged(&staged).expect_err("unknown state").path,
        "$.state"
    );
}

#[test]
fn staged_lag_cycle_and_reference_errors_are_precise() {
    let mut staged = staged_one_op(2);
    staged.body[0].operands[1] = StagedOperand::StateLag {
        stages: 0,
        initial: vec![],
    };
    assert_eq!(
        compile_staged(&staged).expect_err("zero lag").path,
        "$.body[0].operands[1].stages"
    );

    staged.body[0].operands[1] = StagedOperand::StateLag {
        stages: 2,
        initial: vec![GraphOperand::Tensor {
            name: "x".to_owned(),
        }],
    };
    assert_eq!(
        compile_staged(&staged).expect_err("short warmup").path,
        "$.body[0].operands[1]"
    );

    staged.body[0].operands[1] = StagedOperand::StateLag {
        stages: 1,
        initial: vec![GraphOperand::Local { operation: 0 }],
    };
    assert_eq!(
        compile_staged(&staged)
            .expect_err("local warmup is forbidden")
            .path,
        "$.body[0].operands[1].initial[0]"
    );

    staged.body[0].operands[1] = StagedOperand::TensorCycle {
        prefix: "x".to_owned(),
        count: 0,
        stride: 1,
        offset: 0,
    };
    assert_eq!(
        compile_staged(&staged).expect_err("zero cycle").path,
        "$.body[0].operands[1].count"
    );

    staged.body[0].operands[1] = StagedOperand::StageLocal {
        name: "$future".to_owned(),
    };
    assert_eq!(
        compile_staged(&staged).expect_err("forward local").path,
        "$.body[0].operands[1]"
    );
}

#[test]
fn staged_expansion_has_exact_bounds_and_no_integer_wraparound() {
    let accepted = compile_staged(&staged_one_op(128)).expect("exact boundary");
    assert_eq!(accepted.operations.len(), 128);
    assert_eq!(accepted.r#yield, 127);
    assert_eq!(
        compile_staged(&staged_one_op(129))
            .expect_err("129 rejected")
            .path,
        "$.stages"
    );

    let mut overflow = staged_one_op(usize::MAX);
    overflow.body.push(StagedOperation {
        bind: "$second".to_owned(),
        op: GraphOpcode::Add,
        operands: vec![
            StagedOperand::StageLocal {
                name: "$state".to_owned(),
            },
            StagedOperand::Tensor {
                name: "y".to_owned(),
            },
        ],
    });
    overflow.state = "$second".to_owned();
    let error = compile_staged(&overflow).expect_err("checked multiplication overflow");
    assert_eq!(error.path, "$.stages");
    assert_eq!(error.actual["expanded"], "overflow");

    let mut cycle = staged_one_op(2);
    cycle.body[0].operands[1] = StagedOperand::TensorCycle {
        prefix: "x".to_owned(),
        count: 3,
        stride: usize::MAX,
        offset: usize::MAX,
    };
    let graph = compile_staged(&cycle).expect("cycle arithmetic uses widened integers");
    assert_eq!(
        graph.operations[0].operands[1],
        GraphOperand::Tensor {
            name: format!("x{}", (usize::MAX as u128 % 3) as usize),
        }
    );
}

#[test]
fn incremental_batch_has_the_same_exact_128_operation_boundary() {
    let mut transactions = Vec::new();
    for transaction_index in 0..16 {
        let base = transaction_index * 8;
        let operations = (0..8)
            .map(|offset| IncrementalOperation {
                bind: format!("$v{}", base + offset),
                op: GraphOpcode::Mul,
                operands: vec![
                    IncrementalOperand::Scalar {
                        name: "a".to_owned(),
                    },
                    IncrementalOperand::Tensor {
                        name: "x".to_owned(),
                    },
                ],
            })
            .collect();
        transactions.push(IncrementalTransaction {
            schema: TRANSACTION_SCHEMA.to_owned(),
            base_operations: base,
            operations,
        });
    }
    let mut batch = IncrementalBatch {
        schema: agentir_authoring::INCREMENTAL_BATCH_SCHEMA.to_owned(),
        transactions,
        r#yield: "$v127".to_owned(),
    };
    assert_eq!(
        compile_incremental_batch(&batch, vec!["a".to_owned()], vec!["x".to_owned()])
            .expect("128 accepted")
            .operations
            .len(),
        128
    );
    batch.transactions.push(simple_transaction(128, "$v128"));
    batch.r#yield = "$v128".to_owned();
    let error = compile_incremental_batch(&batch, vec!["a".to_owned()], vec!["x".to_owned()])
        .expect_err("129 rejected");
    assert_eq!(error.path, "$.transactions[16].operations");
}

#[test]
fn compilation_and_serialization_are_deterministic() {
    let task = task();
    for text in [
        include_str!("../../../examples/authoring_proposal_two_term.json"),
        include_str!("../../../examples/authoring_incremental_two_term.json"),
        include_str!("../../../examples/authoring_staged_two_term.json"),
    ] {
        let first = parse_authoring_payload(text, None).unwrap();
        let second = parse_authoring_payload(text, None).unwrap();
        let first_graph = compile_authoring_payload(&task, &first).unwrap();
        let second_graph = compile_authoring_payload(&task, &second).unwrap();
        assert_eq!(first_graph, second_graph);
        assert_eq!(
            serde_json::to_vec(&first_graph).unwrap(),
            serde_json::to_vec(&second_graph).unwrap()
        );
    }
}

#[test]
fn rejected_complete_payload_opens_no_workspace_or_compiler_id() {
    let task = task();
    let mut bad = parse_incremental_batch(include_str!(
        "../../../examples/authoring_incremental_two_term.json"
    ))
    .unwrap();
    bad.transactions[1].base_operations = 1;
    let mut gateway = AuthoringGateway::new();
    let error = gateway
        .publish_payload(
            &task,
            &AuthoringPayload::IncrementalBatch(bad),
            ExecutionMode::Portable,
        )
        .expect_err("complete batch rejected before publication");
    assert_eq!(error.path, "$.transactions[1].base_operations");

    let graph = parse_proposal(include_str!(
        "../../../examples/authoring_proposal_two_term.json"
    ))
    .unwrap();
    let accepted = gateway
        .publish_payload(
            &task,
            &AuthoringPayload::Graph(graph),
            ExecutionMode::Portable,
        )
        .expect("first compiler allocation remains available");
    assert_eq!(accepted.workspace, "w1");
    assert_eq!(accepted.revision, "r1");
}

#[test]
fn public_schema_documents_match_root_serde_contracts_and_are_strict() {
    let graph = parse_proposal(include_str!(
        "../../../examples/authoring_proposal_two_term.json"
    ))
    .unwrap();
    let transaction = simple_transaction(0, "$x");
    let batch = parse_incremental_batch(include_str!(
        "../../../examples/authoring_incremental_two_term.json"
    ))
    .unwrap();
    let staged = parse_staged(include_str!(
        "../../../examples/authoring_staged_two_term.json"
    ))
    .unwrap();
    let contracts = [
        (
            GRAPH_JSON_SCHEMA,
            GRAPH_SCHEMA,
            serde_json::to_value(graph).unwrap(),
        ),
        (
            TRANSACTION_JSON_SCHEMA,
            TRANSACTION_SCHEMA,
            serde_json::to_value(transaction).unwrap(),
        ),
        (
            agentir_authoring::INCREMENTAL_BATCH_JSON_SCHEMA,
            agentir_authoring::INCREMENTAL_BATCH_SCHEMA,
            serde_json::to_value(batch).unwrap(),
        ),
        (
            STAGED_JSON_SCHEMA,
            STAGED_SCHEMA,
            serde_json::to_value(staged).unwrap(),
        ),
    ];
    for (schema_text, schema_id, serialized) in contracts {
        let schema: Value = serde_json::from_str(schema_text).expect("valid JSON Schema");
        assert_eq!(schema["properties"]["schema"]["const"], schema_id);
        assert_eq!(schema["additionalProperties"], false);
        let required = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<BTreeSet<_>>();
        let serialized_keys = serialized
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(required, serialized_keys);
        assert_schema_objects_are_strict(&schema);
    }

    for surface in [
        AuthoringSurface::Graph,
        AuthoringSurface::IncrementalBatch,
        AuthoringSurface::Staged,
    ] {
        assert!(!surface.json_schema().is_empty());
        assert!(surface.model_instruction().contains(surface.schema()));
        assert!(surface.model_instruction().contains('{'));
    }
}

fn assert_schema_objects_are_strict(value: &Value) {
    match value {
        Value::Object(object) => {
            if object.contains_key("properties") {
                assert_eq!(
                    object.get("additionalProperties"),
                    Some(&Value::Bool(false)),
                    "object schema must deny unknown fields: {value}"
                );
            }
            for child in object.values() {
                assert_schema_objects_are_strict(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                assert_schema_objects_are_strict(child);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}
