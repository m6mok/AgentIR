use agentir_authoring::{
    AuthoringErrorCode, AuthoringFrame, AuthoringFrameBlueprint, AuthoringGateway, ExecutionMode,
    FRAMED_STAGED_SCHEMA, FrameOpcodeMenu, FrameRole, FrameSlot, FramedOperationChoice,
    FramedStagedProposal, GraphOpcode, GraphOperand, PublicAuthoringDeclarations,
    PublicProblemShape, RecommendationError, SurfaceRecommendation, build_authoring_frame,
    compile_framed_staged, parse_framed_staged, parse_task, recommend_surface,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn declarations() -> PublicAuthoringDeclarations {
    PublicAuthoringDeclarations {
        task_id: "two-term-v2".to_owned(),
        scalars: vec!["a".to_owned(), "b".to_owned()],
        tensors: vec!["x".to_owned(), "y".to_owned()],
    }
}

fn exact_menu(op: GraphOpcode, operands: &[&str]) -> FrameOpcodeMenu {
    FrameOpcodeMenu {
        op,
        operand_roles: operands
            .iter()
            .map(|role| vec![(*role).to_owned()])
            .collect(),
    }
}

fn two_term_blueprint() -> AuthoringFrameBlueprint {
    AuthoringFrameBlueprint {
        stages: 1,
        seed_role: "tensor_x".to_owned(),
        roles: BTreeMap::from([
            (
                "scalar_a".to_owned(),
                FrameRole::Scalar {
                    name: "a".to_owned(),
                },
            ),
            (
                "scalar_b".to_owned(),
                FrameRole::Scalar {
                    name: "b".to_owned(),
                },
            ),
            (
                "tensor_x".to_owned(),
                FrameRole::Tensor {
                    name: "x".to_owned(),
                },
            ),
            (
                "tensor_y".to_owned(),
                FrameRole::Tensor {
                    name: "y".to_owned(),
                },
            ),
            (
                "ax".to_owned(),
                FrameRole::StageLocal {
                    slot: "slot_ax".to_owned(),
                },
            ),
            (
                "by".to_owned(),
                FrameRole::StageLocal {
                    slot: "slot_by".to_owned(),
                },
            ),
        ]),
        slots: vec![
            FrameSlot {
                id: "slot_ax".to_owned(),
                menus: vec![exact_menu(GraphOpcode::Mul, &["scalar_a", "tensor_x"])],
            },
            FrameSlot {
                id: "slot_by".to_owned(),
                menus: vec![exact_menu(GraphOpcode::Mul, &["scalar_b", "tensor_y"])],
            },
            FrameSlot {
                id: "slot_out".to_owned(),
                menus: vec![
                    exact_menu(GraphOpcode::Add, &["ax", "by"]),
                    exact_menu(GraphOpcode::Fma, &["scalar_a", "tensor_x", "tensor_y"]),
                ],
            },
        ],
        state_candidates: vec!["slot_out".to_owned()],
    }
}

fn two_term_frame() -> AuthoringFrame {
    build_authoring_frame(&declarations(), &two_term_blueprint()).expect("valid public frame")
}

fn two_term_response(frame: &AuthoringFrame) -> FramedStagedProposal {
    FramedStagedProposal {
        schema: FRAMED_STAGED_SCHEMA.to_owned(),
        task_id: frame.task_id.clone(),
        frame_hash: frame.frame_hash.clone(),
        choices: vec![
            FramedOperationChoice {
                slot: "slot_ax".to_owned(),
                op: GraphOpcode::Mul,
                operands: vec!["scalar_a".to_owned(), "tensor_x".to_owned()],
            },
            FramedOperationChoice {
                slot: "slot_by".to_owned(),
                op: GraphOpcode::Mul,
                operands: vec!["scalar_b".to_owned(), "tensor_y".to_owned()],
            },
            FramedOperationChoice {
                slot: "slot_out".to_owned(),
                op: GraphOpcode::Add,
                operands: vec!["ax".to_owned(), "by".to_owned()],
            },
        ],
        state: "slot_out".to_owned(),
    }
}

#[test]
fn frame_is_deterministic_public_and_task_specific() {
    let first = two_term_frame();
    let second = two_term_frame();
    assert_eq!(first, second);
    assert_eq!(first.frame_hash.len(), 64);
    assert!(!serde_json::to_string(&first).unwrap().contains("inputs"));
    assert!(!serde_json::to_string(&first).unwrap().contains("intent"));

    let schema = first.response_json_schema();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"]["task_id"]["const"], first.task_id);
    assert_eq!(
        schema["properties"]["frame_hash"]["const"],
        first.frame_hash
    );
    assert_eq!(
        schema["properties"]["choices"]["prefixItems"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn framed_response_lowers_to_v1_graph_and_ordinary_gateway() {
    let frame = two_term_frame();
    let response = two_term_response(&frame);
    let graph = compile_framed_staged(&frame, &response).expect("deterministic lowering");
    let task = parse_task(
        &include_str!("../../../examples/authoring_task_two_term.json")
            .replace("two-term-v1", "two-term-v2"),
    )
    .expect("server task");
    assert_eq!(graph, task.intent);
    let result = AuthoringGateway::new()
        .publish_framed_staged(&task, &frame, &response, ExecutionMode::Portable)
        .expect("shared gateway publication");
    assert_eq!(result.outputs["out"], json!([32.0, 64.0, 96.0, 128.0]));
    assert_eq!(result.workspace, "w1");
}

#[test]
fn stale_frame_duplicate_slot_unknown_role_and_schema_errors_are_local() {
    let frame = two_term_frame();
    let mut stale = two_term_response(&frame);
    stale.frame_hash.replace_range(0..1, "0");
    if stale.frame_hash == frame.frame_hash {
        stale.frame_hash.replace_range(0..1, "1");
    }
    let error = compile_framed_staged(&frame, &stale).expect_err("stale frame rejected");
    assert_eq!(error.path, "$.frame_hash");

    let mut duplicate = two_term_response(&frame);
    duplicate.choices[1].slot = duplicate.choices[0].slot.clone();
    let error = compile_framed_staged(&frame, &duplicate).expect_err("duplicate slot rejected");
    assert_eq!(error.path, "$.choices[1].slot");

    let mut unknown = two_term_response(&frame);
    unknown.choices[0].operands[0] = "unknown_role".to_owned();
    let error = compile_framed_staged(&frame, &unknown).expect_err("unknown role rejected");
    assert_eq!(error.path, "$.choices[0].operands[0]");

    let wrong_type = serde_json::to_value(two_term_response(&frame)).unwrap();
    let mut wrong_type = wrong_type;
    wrong_type["choices"][0]["operands"] = json!("scalar_a");
    let error = parse_framed_staged(&wrong_type.to_string()).expect_err("wrong type rejected");
    assert_eq!(error.path, "$.choices[0].operands");

    let mut extra = serde_json::to_value(two_term_response(&frame)).unwrap();
    extra["compiler_id"] = json!("forbidden");
    let error = parse_framed_staged(&extra.to_string()).expect_err("unknown field rejected");
    assert_eq!(error.code, AuthoringErrorCode::SchemaRejected);
    assert_eq!(error.path, "$.compiler_id");
}

#[test]
fn rejection_is_atomic_and_exact_intent_still_controls_acceptance() {
    let frame = two_term_frame();
    let mut wrong = two_term_response(&frame);
    wrong.choices[2] = FramedOperationChoice {
        slot: "slot_out".to_owned(),
        op: GraphOpcode::Fma,
        operands: vec![
            "scalar_a".to_owned(),
            "tensor_x".to_owned(),
            "tensor_y".to_owned(),
        ],
    };
    let task = parse_task(
        &include_str!("../../../examples/authoring_task_two_term.json")
            .replace("two-term-v1", "two-term-v2"),
    )
    .unwrap();
    let mut gateway = AuthoringGateway::new();
    let error = gateway
        .publish_framed_staged(&task, &frame, &wrong, ExecutionMode::Portable)
        .expect_err("different exact structure rejected even if one input could agree");
    assert_eq!(error.code, AuthoringErrorCode::IntentRejected);
    let accepted = gateway
        .publish_framed_staged(
            &task,
            &frame,
            &two_term_response(&frame),
            ExecutionMode::Portable,
        )
        .expect("first workspace allocation remains available");
    assert_eq!(accepted.workspace, "w1");
}

fn recurrence_blueprint(
    stages: usize,
    body: usize,
    lag_warmup: Option<(usize, usize)>,
) -> AuthoringFrameBlueprint {
    let mut roles = BTreeMap::from([
        (
            "seed".to_owned(),
            FrameRole::Tensor {
                name: "x0".to_owned(),
            },
        ),
        ("previous".to_owned(), FrameRole::StatePrev),
        (
            "input_cycle".to_owned(),
            FrameRole::TensorCycle {
                prefix: "x".to_owned(),
                count: 10,
                stride: 3,
                offset: 1,
            },
        ),
        (
            "scalar_cycle".to_owned(),
            FrameRole::ScalarCycle {
                prefix: "a".to_owned(),
                count: 5,
                stride: 2,
                offset: 0,
            },
        ),
    ]);
    if let Some((lag, warmup)) = lag_warmup {
        roles.insert(
            "lagged".to_owned(),
            FrameRole::StateLag {
                stages: lag,
                initial: (0..warmup)
                    .map(|index| GraphOperand::Tensor {
                        name: format!("x{}", 9 - index),
                    })
                    .collect(),
            },
        );
    }
    let mut slots = Vec::new();
    for index in 0..body {
        let id = format!("slot{index}");
        let left = if index == 0 {
            if lag_warmup.is_some() {
                "lagged".to_owned()
            } else {
                "previous".to_owned()
            }
        } else {
            let role = format!("local{index}");
            roles.insert(
                role.clone(),
                FrameRole::StageLocal {
                    slot: format!("slot{}", index - 1),
                },
            );
            role
        };
        slots.push(FrameSlot {
            id,
            menus: vec![exact_menu(GraphOpcode::Add, &[&left, "input_cycle"])],
        });
    }
    AuthoringFrameBlueprint {
        stages,
        seed_role: "seed".to_owned(),
        roles,
        slots,
        state_candidates: vec![format!("slot{}", body.saturating_sub(1))],
    }
}

fn recurrence_declarations() -> PublicAuthoringDeclarations {
    PublicAuthoringDeclarations {
        task_id: "recurrence".to_owned(),
        scalars: (0..5).map(|index| format!("a{index}")).collect(),
        tensors: (0..10).map(|index| format!("x{index}")).collect(),
    }
}

fn response_from_single_menu(frame: &AuthoringFrame) -> FramedStagedProposal {
    FramedStagedProposal {
        schema: FRAMED_STAGED_SCHEMA.to_owned(),
        task_id: frame.task_id.clone(),
        frame_hash: frame.frame_hash.clone(),
        choices: frame
            .slots
            .iter()
            .map(|slot| FramedOperationChoice {
                slot: slot.id.clone(),
                op: slot.menus[0].op,
                operands: slot.menus[0]
                    .operand_roles
                    .iter()
                    .map(|roles| roles[0].clone())
                    .collect(),
            })
            .collect(),
        state: frame.state_candidates[0].clone(),
    }
}

#[test]
fn zero_one_and_exact_128_boundaries_are_checked_before_expansion() {
    let declarations = recurrence_declarations();
    let zero = recurrence_blueprint(0, 1, None);
    assert!(build_authoring_frame(&declarations, &zero).is_err());

    let one = build_authoring_frame(&declarations, &recurrence_blueprint(1, 1, None)).unwrap();
    assert_eq!(
        compile_framed_staged(&one, &response_from_single_menu(&one))
            .unwrap()
            .operations
            .len(),
        1
    );

    let maximum = build_authoring_frame(&declarations, &recurrence_blueprint(16, 8, None)).unwrap();
    assert_eq!(
        compile_framed_staged(&maximum, &response_from_single_menu(&maximum))
            .unwrap()
            .operations
            .len(),
        128
    );
    assert!(build_authoring_frame(&declarations, &recurrence_blueprint(17, 8, None)).is_err());
}

#[test]
fn state_prev_lag_warmup_and_cycles_have_exact_independent_expansion() {
    let declarations = recurrence_declarations();
    let short = recurrence_blueprint(8, 1, Some((3, 2)));
    assert!(build_authoring_frame(&declarations, &short).is_err());

    for warmup in [3, 4] {
        let frame = build_authoring_frame(
            &declarations,
            &recurrence_blueprint(8, 1, Some((3, warmup))),
        )
        .unwrap();
        let graph = compile_framed_staged(&frame, &response_from_single_menu(&frame)).unwrap();
        for stage in 0..8 {
            let expected_left = if stage < warmup {
                GraphOperand::Tensor {
                    name: format!("x{}", 9 - stage),
                }
            } else {
                GraphOperand::Local {
                    operation: stage - 3,
                }
            };
            assert_eq!(graph.operations[stage].operands[0], expected_left);
            assert_eq!(
                graph.operations[stage].operands[1],
                GraphOperand::Tensor {
                    name: format!("x{}", (stage * 3 + 1) % 10),
                }
            );
        }
    }
}

#[test]
fn fixed_seed_frame_corpus_matches_an_independent_oracle() {
    let declarations = recurrence_declarations();
    let mut seed = 20_260_813_u64;
    for _ in 0..32 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let stages = 1 + usize::try_from((seed >> 8) % 32).unwrap();
        let frame =
            build_authoring_frame(&declarations, &recurrence_blueprint(stages, 1, None)).unwrap();
        let graph = compile_framed_staged(&frame, &response_from_single_menu(&frame)).unwrap();
        assert_eq!(graph.operations.len(), stages);
        for stage in 0..stages {
            let expected_state = if stage == 0 {
                GraphOperand::Tensor {
                    name: "x0".to_owned(),
                }
            } else {
                GraphOperand::Local {
                    operation: stage - 1,
                }
            };
            assert_eq!(graph.operations[stage].operands[0], expected_state);
        }
    }
}

#[test]
fn recommendation_is_public_deterministic_and_rejects_ambiguity() {
    let declarations = recurrence_declarations();
    assert_eq!(
        recommend_surface(
            &declarations,
            &PublicProblemShape::Dag {
                operations: 12,
                auditable: true,
                exact_order_prescribed: true,
            }
        )
        .unwrap(),
        SurfaceRecommendation::Graph
    );
    assert_eq!(
        recommend_surface(
            &declarations,
            &PublicProblemShape::Dag {
                operations: 80,
                auditable: false,
                exact_order_prescribed: true,
            }
        )
        .unwrap(),
        SurfaceRecommendation::IncrementalBatch
    );
    assert!(matches!(
        recommend_surface(
            &declarations,
            &PublicProblemShape::RegularRecurrence {
                blueprint: recurrence_blueprint(8, 1, None),
                exact_order_prescribed: true,
            }
        )
        .unwrap(),
        SurfaceRecommendation::FramedStaged(_)
    ));
    assert_eq!(
        recommend_surface(
            &declarations,
            &PublicProblemShape::Dag {
                operations: 4,
                auditable: true,
                exact_order_prescribed: false,
            }
        )
        .unwrap_err(),
        RecommendationError::AmbiguousIntent
    );
}

#[test]
fn generic_schema_and_serde_bytes_are_deterministic() {
    let generic: Value =
        serde_json::from_str(agentir_authoring::FRAMED_STAGED_JSON_SCHEMA).unwrap();
    assert_eq!(generic["additionalProperties"], false);
    let frame = two_term_frame();
    let first = two_term_response(&frame);
    let reparsed = parse_framed_staged(&serde_json::to_string(&first).unwrap()).unwrap();
    assert_eq!(first, reparsed);
    assert_eq!(
        serde_json::to_vec(&compile_framed_staged(&frame, &first).unwrap()).unwrap(),
        serde_json::to_vec(&compile_framed_staged(&frame, &reparsed).unwrap()).unwrap()
    );
}
