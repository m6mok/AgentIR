use agentir_authoring::{
    GraphOpcode as O, GraphOperand as A, IncrementalOperand as I, IncrementalOperation,
    IncrementalSession, IncrementalTransaction, STAGED_SCHEMA, StagedOperand as S, StagedOperation,
    StagedProposal, TRANSACTION_SCHEMA, compile_incremental_batch, compile_staged,
    parse_incremental_batch, parse_proposal, parse_staged,
};

fn i_local(name: &str) -> I {
    I::Local { name: name.into() }
}
fn i_op(bind: &str, op: O, operands: Vec<I>) -> IncrementalOperation {
    IncrementalOperation {
        bind: bind.into(),
        op,
        operands,
    }
}
fn s_local(name: &str) -> S {
    S::StageLocal { name: name.into() }
}
fn s_op(bind: &str, op: O, operands: Vec<S>) -> StagedOperation {
    StagedOperation {
        bind: bind.into(),
        op,
        operands,
    }
}

fn design_builder() -> StagedProposal {
    StagedProposal {
        schema: STAGED_SCHEMA.into(),
        stages: 16,
        seed: A::Tensor { name: "x0".into() },
        body: vec![
            s_op(
                "$affine",
                O::Fma,
                vec![
                    S::ScalarCycle {
                        prefix: "a".into(),
                        count: 5,
                        stride: 1,
                        offset: 0,
                    },
                    S::StatePrev,
                    S::TensorCycle {
                        prefix: "x".into(),
                        count: 10,
                        stride: 2,
                        offset: 1,
                    },
                ],
            ),
            s_op(
                "$masked",
                O::Mul,
                vec![
                    S::TensorCycle {
                        prefix: "x".into(),
                        count: 10,
                        stride: 3,
                        offset: 2,
                    },
                    s_local("$affine"),
                ],
            ),
            s_op(
                "$shifted",
                O::Add,
                vec![
                    S::TensorCycle {
                        prefix: "x".into(),
                        count: 10,
                        stride: 5,
                        offset: 3,
                    },
                    s_local("$affine"),
                ],
            ),
            s_op(
                "$mixed",
                O::Add,
                vec![s_local("$masked"), s_local("$shifted")],
            ),
            s_op(
                "$feedback",
                O::Fma,
                vec![
                    s_local("$mixed"),
                    S::ScalarCycle {
                        prefix: "a".into(),
                        count: 5,
                        stride: 1,
                        offset: 2,
                    },
                    S::StatePrev,
                ],
            ),
            s_op(
                "$state",
                O::Add,
                vec![
                    s_local("$feedback"),
                    S::StateLag {
                        stages: 3,
                        initial: vec![
                            A::Tensor { name: "x9".into() },
                            A::Tensor { name: "x8".into() },
                            A::Tensor { name: "x7".into() },
                            A::Tensor { name: "x6".into() },
                        ],
                    },
                ],
            ),
        ],
        state: "$state".into(),
    }
}

#[test]
fn staged_builder_expands_the_exact_design_without_manual_indices() {
    let graph = compile_staged(&design_builder()).expect("valid staged design");
    assert_eq!(graph.operations.len(), 96);
    assert_eq!(graph.r#yield, 95);
    assert_eq!(
        graph.operations[23].operands[1],
        A::Tensor { name: "x6".into() }
    );
    assert_eq!(graph.operations[29].operands[1], A::Local { operation: 11 });
    assert_eq!(graph.operations[95].operands[1], A::Local { operation: 77 });
}

#[test]
fn incremental_transactions_match_staged_expansion_and_stay_bounded() {
    let expected = compile_staged(&design_builder()).unwrap();
    let scalars = (0..5).map(|i| format!("a{i}"));
    let tensors = (0..10).map(|i| format!("x{i}"));
    let mut session = IncrementalSession::new(scalars, tensors);
    for stage in 0..16 {
        let base = stage * 6;
        let state = if stage == 0 {
            I::Tensor { name: "x0".into() }
        } else {
            i_local(&format!("$state{}", stage - 1))
        };
        let lag = if stage < 4 {
            I::Tensor {
                name: format!("x{}", 9 - stage),
            }
        } else {
            i_local(&format!("$state{}", stage - 3))
        };
        let tx = IncrementalTransaction {
            schema: TRANSACTION_SCHEMA.into(),
            base_operations: base,
            operations: vec![
                i_op(
                    &format!("$affine{stage}"),
                    O::Fma,
                    vec![
                        I::Scalar {
                            name: format!("a{}", stage % 5),
                        },
                        state.clone(),
                        I::Tensor {
                            name: format!("x{}", (2 * stage + 1) % 10),
                        },
                    ],
                ),
                i_op(
                    &format!("$masked{stage}"),
                    O::Mul,
                    vec![
                        I::Tensor {
                            name: format!("x{}", (3 * stage + 2) % 10),
                        },
                        i_local(&format!("$affine{stage}")),
                    ],
                ),
                i_op(
                    &format!("$shifted{stage}"),
                    O::Add,
                    vec![
                        I::Tensor {
                            name: format!("x{}", (5 * stage + 3) % 10),
                        },
                        i_local(&format!("$affine{stage}")),
                    ],
                ),
                i_op(
                    &format!("$mixed{stage}"),
                    O::Add,
                    vec![
                        i_local(&format!("$masked{stage}")),
                        i_local(&format!("$shifted{stage}")),
                    ],
                ),
                i_op(
                    &format!("$feedback{stage}"),
                    O::Fma,
                    vec![
                        i_local(&format!("$mixed{stage}")),
                        I::Scalar {
                            name: format!("a{}", (stage + 2) % 5),
                        },
                        state,
                    ],
                ),
                i_op(
                    &format!("$state{stage}"),
                    O::Add,
                    vec![i_local(&format!("$feedback{stage}")), lag],
                ),
            ],
        };
        session.apply(&tx).expect("six-op stage transaction");
    }
    assert_eq!(session.finish("$state15").unwrap(), expected);
}

#[test]
fn rejected_incremental_transaction_is_atomic_and_stale_base_is_local() {
    let mut session = IncrementalSession::new(vec!["a0".into()], vec!["x0".into()]);
    let first = IncrementalTransaction {
        schema: TRANSACTION_SCHEMA.into(),
        base_operations: 0,
        operations: vec![i_op(
            "$ok",
            O::Mul,
            vec![
                I::Scalar { name: "a0".into() },
                I::Tensor { name: "x0".into() },
            ],
        )],
    };
    session.apply(&first).unwrap();
    let bad = IncrementalTransaction {
        schema: TRANSACTION_SCHEMA.into(),
        base_operations: 1,
        operations: vec![i_op(
            "$new",
            O::Add,
            vec![i_local("$missing"), i_local("$ok")],
        )],
    };
    let error = session.apply(&bad).expect_err("unknown binding rejected");
    assert_eq!(error.path, "$.operations[0].operands[0]");
    assert_eq!(session.operation_count(), 1);
    assert!(session.finish("$new").is_err());
    let stale = IncrementalTransaction {
        schema: TRANSACTION_SCHEMA.into(),
        base_operations: 0,
        operations: vec![i_op("$new", O::Add, vec![i_local("$ok"), i_local("$ok")])],
    };
    let error = session.apply(&stale).expect_err("stale base rejected");
    assert_eq!(error.path, "$.base_operations");
    assert_eq!(session.operation_count(), 1);
}

#[test]
fn staged_builder_rejects_invalid_lag_before_expansion() {
    let mut source = design_builder();
    if let S::StateLag { initial, .. } = &mut source.body[5].operands[1] {
        initial.pop();
        initial.pop();
    }
    let error = compile_staged(&source).expect_err("incomplete prehistory rejected");
    assert_eq!(error.path, "$.body[5].operands[1]");
}

fn compile_fixture_batch(text: &str) -> agentir_authoring::GraphProposal {
    let batch = parse_incremental_batch(text).expect("strict incremental batch");
    compile_incremental_batch(
        &batch,
        (0..5).map(|index| format!("a{index}")),
        (0..10).map(|index| format!("x{index}")),
    )
    .expect("accepted batch")
}

#[test]
fn luna_medium_abc_separates_raw_incremental_and_builder_failure_modes() {
    let expected = compile_staged(&design_builder()).expect("canonical design");

    let raw_text = include_str!("fixtures/luna_medium_single_design96.json");
    let raw = parse_proposal(raw_text).expect("raw graph passes strict schema");
    assert_eq!(raw.operations[..29], expected.operations[..29]);
    assert_eq!(raw.operations[29].operands[1], A::Local { operation: 23 });
    assert_eq!(
        expected.operations[29].operands[1],
        A::Local { operation: 11 }
    );

    let incremental_text = include_str!("fixtures/luna_medium_incremental_design96.json");
    let incremental = compile_fixture_batch(incremental_text);
    assert_eq!(incremental, expected);

    let builder_text = include_str!("fixtures/luna_medium_builder_design96.json");
    let builder = compile_staged(&parse_staged(builder_text).expect("strict staged builder"))
        .expect("valid staged builder");
    assert_eq!(builder.operations[..23], expected.operations[..23]);
    assert_eq!(
        builder.operations[23].operands[1],
        A::Local { operation: 5 }
    );
    assert_eq!(
        expected.operations[23].operands[1],
        A::Tensor { name: "x6".into() }
    );
    let repaired_builder_text = include_str!("fixtures/luna_medium_builder_design96_repaired.json");
    let repaired_builder = compile_staged(
        &parse_staged(repaired_builder_text).expect("strict repaired staged builder"),
    )
    .expect("valid repaired staged builder");
    assert_eq!(repaired_builder, expected);

    let incremental_authored: usize = parse_incremental_batch(incremental_text)
        .unwrap()
        .transactions
        .iter()
        .map(|transaction| transaction.operations.len())
        .sum();
    let builder_authored = parse_staged(builder_text).unwrap().body.len();
    assert_eq!(
        (raw.operations.len(), incremental_authored, builder_authored),
        (96, 96, 6)
    );
    assert!(builder_text.len() < incremental_text.len());
    assert!(incremental_text.len() < raw_text.len());
}
