use agentir_authoring::{
    AuthoringGateway, AuthoringTask, ExecutionMode, GRAPH_SCHEMA, GraphOpcode, GraphOperand,
    GraphOperation, GraphProposal, TASK_SCHEMA, parse_proposal,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn scalar(prefix: &str, index: usize) -> GraphOperand {
    GraphOperand::Scalar {
        name: format!("{prefix}{index}"),
    }
}

fn tensor(prefix: &str, index: usize) -> GraphOperand {
    GraphOperand::Tensor {
        name: format!("{prefix}{index}"),
    }
}

const fn local(operation: usize) -> GraphOperand {
    GraphOperand::Local { operation }
}

fn operation(op: GraphOpcode, operands: Vec<GraphOperand>) -> GraphOperation {
    GraphOperation { op, operands }
}

fn canonical_design() -> GraphProposal {
    let mut operations = Vec::with_capacity(96);
    for stage in 0..16 {
        let base = stage * 6;
        let state = if stage == 0 {
            GraphOperand::Tensor {
                name: "seed".to_owned(),
            }
        } else {
            local(base - 1)
        };
        operations.extend([
            operation(
                GraphOpcode::Fma,
                vec![
                    scalar("g", stage % 5),
                    state.clone(),
                    tensor("bias", ((2 * stage) + 1) % 4),
                ],
            ),
            operation(
                GraphOpcode::Mul,
                vec![tensor("mask", ((3 * stage) + 2) % 5), local(base)],
            ),
            operation(
                GraphOpcode::Add,
                vec![tensor("source", ((5 * stage) + 3) % 7), local(base)],
            ),
            operation(GraphOpcode::Add, vec![local(base + 1), local(base + 2)]),
            operation(
                GraphOpcode::Fma,
                vec![local(base + 3), scalar("h", (stage + 2) % 4), state],
            ),
            operation(
                GraphOpcode::Add,
                vec![
                    local(base + 4),
                    if stage < 4 {
                        tensor("anchor", stage)
                    } else {
                        local(((stage - 4) * 6) + 5)
                    },
                ],
            ),
        ]);
    }
    GraphProposal {
        schema: GRAPH_SCHEMA.to_owned(),
        operations,
        r#yield: 95,
    }
}

fn task(intent: GraphProposal) -> AuthoringTask {
    let mut inputs = BTreeMap::new();
    for index in 0..5 {
        inputs.insert(format!("g{index}"), json!(0.02 + f64::from(index) * 0.002));
    }
    for index in 0..4 {
        inputs.insert(format!("h{index}"), json!(0.03 + f64::from(index) * 0.002));
    }
    inputs.insert("seed".to_owned(), json!([0.01, 0.011]));
    for (prefix, count) in [("bias", 4), ("mask", 5), ("source", 7), ("anchor", 4)] {
        for index in 0..count {
            let first = 0.01 + f64::from(index) * 0.001;
            inputs.insert(format!("{prefix}{index}"), json!([first, first + 0.0005]));
        }
    }
    AuthoringTask {
        schema: TASK_SCHEMA.to_owned(),
        task_id: "design-choice-96-low".to_owned(),
        dimension: "N".to_owned(),
        scalars: (0..5)
            .map(|index| format!("g{index}"))
            .chain((0..4).map(|index| format!("h{index}")))
            .collect(),
        tensors: std::iter::once("seed".to_owned())
            .chain(
                [("bias", 4), ("mask", 5), ("source", 7), ("anchor", 4)]
                    .into_iter()
                    .flat_map(|(prefix, count)| {
                        (0..count).map(move |index| format!("{prefix}{index}"))
                    }),
            )
            .collect(),
        inputs,
        intent,
    }
}

#[test]
fn low_reasoning_selects_and_executes_the_canonical_minimum_design() {
    // Each stage has six distinct required semantic results. Two affine roles can
    // each use one FMA; every other role needs one operation, so six is both a
    // constructive upper bound and a lower bound. Recomputing affine or expanding
    // either FMA exceeds it. The declared ready-role order then fixes the schedule.
    const MINIMUM_OPERATIONS_PER_STAGE: usize = 6;
    const STAGES: usize = 16;
    let expected = canonical_design();
    assert_eq!(
        expected.operations.len(),
        STAGES * MINIMUM_OPERATIONS_PER_STAGE
    );
    assert_eq!(
        expected
            .operations
            .iter()
            .filter(|operation| operation.op == GraphOpcode::Fma)
            .count(),
        32
    );

    let trials = [
        ("low-1", include_str!("fixtures/design96_low_1.json")),
        ("low-2", include_str!("fixtures/design96_low_2.json")),
        ("low-3", include_str!("fixtures/design96_low_3.json")),
        (
            "terra-low-repaired",
            include_str!("fixtures/terra_low_design96_repaired.json"),
        ),
    ];
    for (name, payload) in trials {
        let proposal = parse_proposal(payload)
            .unwrap_or_else(|error| panic!("{name} must satisfy strict schema: {error}"));
        assert_eq!(proposal, expected, "{name} selected a non-canonical design");
        let result = AuthoringGateway::new()
            .publish(&task(expected.clone()), &proposal, ExecutionMode::Portable)
            .unwrap_or_else(|error| panic!("{name} must publish and execute: {error}"));
        let outputs = result.outputs["out"].as_array().expect("tensor output");
        assert_eq!(outputs.len(), 2);
        assert!(outputs.iter().all(Value::is_number));
    }
}

#[test]
fn terra_low_design_first_attempt_is_rejected_before_publication() {
    let error = parse_proposal(include_str!("fixtures/terra_low_design96.json"))
        .expect_err("inputs/prior aliases are outside the strict wire contract");
    assert_eq!(
        error.code,
        agentir_authoring::AuthoringErrorCode::SchemaRejected
    );
    assert_eq!(error.path, "$.operations[0].inputs");
    assert_eq!(
        error.repair_hint.as_deref(),
        Some(agentir_authoring::DEFAULT_MODEL_INSTRUCTION)
    );
}
