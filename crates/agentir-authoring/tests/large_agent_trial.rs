use agentir_authoring::{
    AuthoringGateway, AuthoringTask, ExecutionMode, GRAPH_SCHEMA, GraphOpcode, GraphOperand,
    GraphOperation, GraphProposal, TASK_SCHEMA, parse_proposal,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn operand(encoded: &str) -> GraphOperand {
    let (kind, value) = encoded.split_once(':').expect("test operand encoding");
    match kind {
        "s" => GraphOperand::Scalar {
            name: value.to_owned(),
        },
        "t" => GraphOperand::Tensor {
            name: value.to_owned(),
        },
        "l" => GraphOperand::Local {
            operation: value.parse().expect("local index"),
        },
        _ => panic!("unknown test operand kind"),
    }
}

fn expected_large_graph() -> GraphProposal {
    let specification: &[(&str, &[&str])] = &[
        ("mul", &["s:alpha", "t:x0"]),
        ("fma", &["t:x1", "s:beta", "t:x2"]),
        ("add", &["l:0", "l:1"]),
        ("mul", &["t:x3", "l:2"]),
        ("fma", &["s:gamma", "t:x4", "l:3"]),
        ("add", &["t:x5", "l:4"]),
        ("mul", &["l:2", "l:5"]),
        ("fma", &["l:6", "s:delta", "t:x6"]),
        ("add", &["l:7", "l:0"]),
        ("mul", &["s:epsilon", "l:8"]),
        ("add", &["l:9", "t:x7"]),
        ("fma", &["t:x8", "s:zeta", "l:10"]),
        ("mul", &["l:11", "l:3"]),
        ("add", &["l:12", "l:1"]),
        ("mul", &["t:x9", "l:13"]),
        ("add", &["l:14", "l:6"]),
        ("fma", &["s:alpha", "l:15", "t:x0"]),
        ("mul", &["l:16", "s:beta"]),
        ("add", &["l:17", "l:8"]),
        ("mul", &["l:18", "l:5"]),
        ("fma", &["l:19", "s:gamma", "l:2"]),
        ("add", &["t:x4", "l:20"]),
        ("mul", &["s:delta", "l:21"]),
        ("add", &["l:22", "l:11"]),
        ("fma", &["l:23", "s:epsilon", "t:x5"]),
        ("mul", &["l:24", "l:7"]),
        ("add", &["l:25", "l:16"]),
        ("mul", &["s:zeta", "l:26"]),
        ("add", &["l:27", "t:x9"]),
        ("fma", &["l:28", "s:alpha", "l:13"]),
        ("mul", &["l:29", "l:0"]),
        ("add", &["l:30", "l:24"]),
    ];
    GraphProposal {
        schema: GRAPH_SCHEMA.to_owned(),
        operations: specification
            .iter()
            .map(|(opcode, operands)| GraphOperation {
                op: match *opcode {
                    "add" => GraphOpcode::Add,
                    "mul" => GraphOpcode::Mul,
                    "fma" => GraphOpcode::Fma,
                    _ => panic!("unknown test opcode"),
                },
                operands: operands.iter().map(|item| operand(item)).collect(),
            })
            .collect(),
        r#yield: 31,
    }
}

#[test]
fn high_capability_agent_authors_and_executes_exact_32_operation_dag() {
    let agent_output = parse_proposal(include_str!("fixtures/large_good_agent_output.json"))
        .expect("agent output follows strict schema");
    let expected = expected_large_graph();
    assert_eq!(agent_output, expected);

    let mut inputs = BTreeMap::new();
    for (name, value) in [
        ("alpha", 0.5),
        ("beta", 0.6),
        ("gamma", 0.7),
        ("delta", 0.8),
        ("epsilon", 0.9),
        ("zeta", 0.4),
    ] {
        inputs.insert(name.to_owned(), json!(value));
    }
    for (index, name) in ["x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7", "x8", "x9"]
        .into_iter()
        .enumerate()
    {
        let first = 0.1 + (index as f64 * 0.05);
        inputs.insert(name.to_owned(), json!([first, first + 0.03]));
    }
    let task = AuthoringTask {
        schema: TASK_SCHEMA.to_owned(),
        task_id: "large-good-agent-32-op".to_owned(),
        dimension: "N".to_owned(),
        scalars: ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        tensors: ["x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7", "x8", "x9"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        inputs,
        intent: expected,
    };
    let result = AuthoringGateway::new()
        .publish(&task, &agent_output, ExecutionMode::Portable)
        .expect("large exact graph publishes and executes");
    let outputs = result.outputs["out"].as_array().expect("tensor output");
    assert_eq!(outputs.len(), 2);
    assert!(outputs.iter().all(Value::is_number));
    assert_eq!(result.model_visible_calls, 1);
    assert_eq!(result.internal_agentir_requests, 10);
}
