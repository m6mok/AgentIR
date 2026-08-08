//! Dependency-light statistical cross-stage performance baseline.

use agentir_core::{
    Action, HoleId, RevisionId, Transaction, Workspace, WorkspaceId,
    actions::{RegionArgumentSpec, RegionSpec},
    candidate::{
        CandidateAction, CandidateAllocator, CandidateTransaction, FOLD_SCALAR_CONSTANTS_RULE,
        ProposalInput, ProposalOperation, ProposalResult, ProposedImplFragment, RelationKind,
        SpeculativeRewriteProposal, canonicalize_proposal_with_limit,
        normalize_speculative_proposal,
    },
    canonical::canonical_bytes,
    constraints::ConstraintFacts,
    continuation::InteractionMode,
    ids::{
        BufferId, CandidateId, CandidateRevisionId, EqualityNodeId, EqualityRevisionId,
        EqualitySpaceId, ImplOperationId, ImplValueId, MemoryGuardId, MemoryPlanId,
        MemoryRevisionId, ProposalId, ScheduleAxisId, ScheduleNodeId, SchedulePlanId,
        ScheduleRevisionId, TargetManifestId, TargetManifestRevisionId,
    },
    impl_ir::{canonicalize_impl_with_limit, identity_lower},
    ir::ConstantValue,
    memory::{MemoryAction, MemoryTransaction, canonical_memory_bytes_with_limit},
    resources::ResourceLimits,
    schedule::{ScheduleAction, ScheduleTransaction, canonical_schedule_bytes},
    schedule_ir::BindingLevel,
    semantic::canonicalize_spec,
    shapes::{ShapeConstraint, same_shape},
    target::{TargetProfile, canonical_target_bytes},
    types::Shape,
};
use agentir_store::{
    WorkspaceArchiveV1, WorkspaceArchiveV2, WorkspaceArchiveV3, WorkspaceArchiveV4,
    WorkspaceArchiveV5, WorkspaceArchiveV6, load_workspace_bytes, migrate_archive_v1_to_v2,
    migrate_archive_v2_to_v3, migrate_archive_v3_to_v4, migrate_archive_v4_to_v5,
    migrate_archive_v5_to_v6, migrate_archive_v6_to_v7,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, hint::black_box, process::Command, time::Instant};

const WARM_UP_ITERATIONS: usize = 3;
const MEASURED_ITERATIONS: usize = 15;

#[derive(Serialize)]
struct Measurement {
    warm_up_iterations: usize,
    measured_iterations: usize,
    min: u128,
    median: u128,
    p95: u128,
    max: u128,
    unit: &'static str,
    workload_size: Value,
}

#[derive(Serialize)]
struct Metadata {
    benchmark_schema_version: u32,
    agentir_crate_version: &'static str,
    git_revision: Option<String>,
    dirty_worktree: Option<bool>,
    target_architecture: &'static str,
    operating_system: &'static str,
    rust_compiler_version: Option<String>,
    build_mode: &'static str,
}

fn command_output(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program).args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn metadata() -> Metadata {
    Metadata {
        benchmark_schema_version: 2,
        agentir_crate_version: env!("CARGO_PKG_VERSION"),
        git_revision: command_output("git", &["rev-parse", "HEAD"]),
        dirty_worktree: Command::new("git")
            .args(["status", "--porcelain"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| !output.stdout.is_empty()),
        target_architecture: std::env::consts::ARCH,
        operating_system: std::env::consts::OS,
        rust_compiler_version: command_output("rustc", &["--version"]),
        build_mode: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    }
}

fn elapsed_ns<T>(operation: impl FnOnce() -> T) -> u128 {
    let started = Instant::now();
    black_box(operation());
    started.elapsed().as_nanos()
}

fn measure(mut sample: impl FnMut() -> u128, workload_size: Value) -> Measurement {
    for _ in 0..WARM_UP_ITERATIONS {
        black_box(sample());
    }
    let mut samples = (0..MEASURED_ITERATIONS)
        .map(|_| sample())
        .collect::<Vec<_>>();
    samples.sort_unstable();
    let p95_index = (samples.len() * 95).div_ceil(100).saturating_sub(1);
    Measurement {
        warm_up_iterations: WARM_UP_ITERATIONS,
        measured_iterations: MEASURED_ITERATIONS,
        min: samples[0],
        median: samples[samples.len() / 2],
        p95: samples[p95_index],
        max: samples[samples.len() - 1],
        unit: "nanoseconds",
        workload_size,
    }
}

fn chain_transaction(workspace: &Workspace, operation_count: usize) -> Transaction {
    let mut actions = vec![
        Action::CreateConstant {
            bind: "$acc0".to_owned(),
            ty: "f32".parse().unwrap(),
            value: json!(1.0),
        },
        Action::CreateConstant {
            bind: "$one".to_owned(),
            ty: "f32".parse().unwrap(),
            value: json!(1.0),
        },
    ];
    for index in 0..operation_count {
        actions.push(Action::CreateOp {
            bind: format!("$acc{}", index + 1),
            opcode: "add".to_owned(),
            operands: vec![format!("$acc{index}"), "$one".to_owned()],
            attributes: BTreeMap::new(),
            region: None,
        });
    }
    actions.push(Action::SetOutput {
        name: "out".to_owned(),
        value: format!("$acc{operation_count}"),
    });
    Transaction {
        workspace: workspace.id().clone(),
        base_revision: RevisionId::new("r0"),
        actions,
        client_transaction_id: None,
        allow_branch: false,
    }
}

fn transaction_apply(operation_count: usize) -> u128 {
    let mut workspace = Workspace::new(WorkspaceId::new("bench")).unwrap();
    let transaction = chain_transaction(&workspace, operation_count);
    elapsed_ns(|| {
        black_box(workspace.apply(&transaction).unwrap());
    })
}

fn frozen_chain(operation_count: usize) -> Workspace {
    let mut workspace =
        Workspace::new(WorkspaceId::new(format!("chain-{operation_count}"))).unwrap();
    let mut transaction = chain_transaction(&workspace, operation_count);
    transaction.actions[0] = Action::CreateParameter {
        bind: "$acc0".to_owned(),
        name: "input".to_owned(),
        ty: "f32".parse().unwrap(),
    };
    let built = workspace.apply(&transaction).unwrap();
    workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: built.revision,
            actions: vec![Action::FreezeSpec],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    workspace
}

fn partial_workspace() -> (Workspace, RevisionId) {
    let mut workspace = Workspace::new(WorkspaceId::new("partial")).unwrap();
    let revision = workspace
        .apply(&Transaction {
            workspace: workspace.id().clone(),
            base_revision: RevisionId::new("r0"),
            actions: vec![
                Action::CreateParameter {
                    bind: "$x".to_owned(),
                    name: "x".to_owned(),
                    ty: "f32".parse().unwrap(),
                },
                Action::CreateHole {
                    bind: "$hole".to_owned(),
                    expected_type: "f32".parse().unwrap(),
                    shape_constraints: Vec::new(),
                },
                Action::SetOutput {
                    name: "out".to_owned(),
                    value: "$hole".to_owned(),
                },
            ],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap()
        .revision;
    (workspace, revision)
}

fn equality(left: &str, right: &str) -> ShapeConstraint {
    ShapeConstraint::Equal {
        left: left.parse().unwrap(),
        right: right.parse().unwrap(),
    }
}

fn constraint_facts(count: usize) -> ConstraintFacts {
    let mut facts = ConstraintFacts::default();
    for index in 0..=count {
        facts.declare_symbol(&format!("S{index}"), true).unwrap();
    }
    for index in 0..count {
        facts
            .insert(&equality(
                &format!("[S{index}]"),
                &format!("[S{}]", index + 1),
            ))
            .unwrap();
    }
    facts
}

fn fact_insertion(count: usize) -> u128 {
    elapsed_ns(|| black_box(constraint_facts(count)))
}

fn transitive_discharge() -> u128 {
    let mut facts = ConstraintFacts::default();
    for symbol in ["N", "M", "K"] {
        facts.declare_symbol(symbol, true).unwrap();
    }
    facts.insert(&equality("[N]", "[M]")).unwrap();
    elapsed_ns(|| {
        facts.insert(&equality("[M]", "[K]")).unwrap();
        black_box(
            facts
                .query_shapes(&"[N]".parse().unwrap(), &"[K]".parse().unwrap())
                .unwrap(),
        );
    })
}

fn contradiction_detection() -> u128 {
    let mut facts = ConstraintFacts::default();
    facts.declare_symbol("N", true).unwrap();
    facts.insert(&equality("[N]", "[4]")).unwrap();
    elapsed_ns(|| {
        black_box(facts.insert(&equality("[N]", "[5]")).unwrap_err());
    })
}

fn resource_rejection() -> u128 {
    let limits = ResourceLimits {
        actions_per_transaction: 1,
        ..ResourceLimits::default()
    };
    let mut workspace = Workspace::with_limits(WorkspaceId::new("limited"), limits).unwrap();
    let transaction = Transaction {
        workspace: workspace.id().clone(),
        base_revision: RevisionId::new("r0"),
        actions: vec![
            Action::CreateConstant {
                bind: "$a".to_owned(),
                ty: "i32".parse().unwrap(),
                value: json!(1),
            },
            Action::CreateConstant {
                bind: "$b".to_owned(),
                ty: "i32".parse().unwrap(),
                value: json!(2),
            },
        ],
        client_transaction_id: None,
        allow_branch: false,
    };
    elapsed_ns(|| black_box(workspace.apply(&transaction).unwrap_err()))
}

fn identity_candidate(operation_count: usize) -> Workspace {
    let mut workspace = frozen_chain(operation_count);
    let revision = workspace.head().clone();
    workspace
        .candidate_create(&revision, RelationKind::EquivalentToSpec)
        .unwrap();
    workspace
}

fn tensor_memory_candidate(operation_count: usize) -> Workspace {
    let id = WorkspaceId::new(format!("memory-chain-{operation_count}"));
    let limits = ResourceLimits {
        memory_alias_facts: 1_000_000,
        ..ResourceLimits::default()
    };
    let mut workspace = Workspace::with_limits(id.clone(), limits).unwrap();
    let mut actions = vec![
        Action::DefineDimension {
            bind: Some("$N".to_owned()),
            name: "N".to_owned(),
            constraints: vec!["N >= 0".to_owned()],
        },
        Action::CreateParameter {
            bind: "$value0".to_owned(),
            name: "x".to_owned(),
            ty: "tensor<f32,[N]>".parse().unwrap(),
        },
    ];
    for index in 0..operation_count {
        actions.push(Action::CreateOp {
            bind: format!("$value{}", index + 1),
            opcode: "map".to_owned(),
            operands: vec![format!("$value{index}")],
            attributes: BTreeMap::new(),
            region: Some(RegionSpec {
                arguments: vec![RegionArgumentSpec {
                    name: "element".to_owned(),
                    ty: "f32".parse().unwrap(),
                }],
                captures: Vec::new(),
                operations: Vec::new(),
                yield_value: "element".to_owned(),
            }),
        });
    }
    actions.push(Action::SetOutput {
        name: "out".to_owned(),
        value: format!("$value{operation_count}"),
    });
    let built = workspace
        .apply(&Transaction {
            workspace: id.clone(),
            base_revision: RevisionId::new("r0"),
            actions,
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    workspace
        .apply(&Transaction {
            workspace: id,
            base_revision: built.revision,
            actions: vec![Action::FreezeSpec],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    let revision = workspace.head().clone();
    workspace
        .candidate_create(&revision, RelationKind::EquivalentToSpec)
        .unwrap();
    workspace
}

fn constant_match_candidate(match_count: usize) -> Workspace {
    let id = WorkspaceId::new(format!("constant-matches-{match_count}"));
    let mut workspace = Workspace::new(id.clone()).unwrap();
    let mut actions = vec![
        Action::CreateConstant {
            bind: "$left".to_owned(),
            ty: "i32".parse().unwrap(),
            value: json!(2),
        },
        Action::CreateConstant {
            bind: "$right".to_owned(),
            ty: "i32".parse().unwrap(),
            value: json!(3),
        },
    ];
    for index in 0..match_count {
        actions.push(Action::CreateOp {
            bind: format!("$sum{index}"),
            opcode: "add".to_owned(),
            operands: vec!["$left".to_owned(), "$right".to_owned()],
            attributes: BTreeMap::new(),
            region: None,
        });
        actions.push(Action::SetOutput {
            name: format!("out{index}"),
            value: format!("$sum{index}"),
        });
    }
    let built = workspace
        .apply(&Transaction {
            workspace: id.clone(),
            base_revision: RevisionId::new("r0"),
            actions,
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    workspace
        .apply(&Transaction {
            workspace: id,
            base_revision: built.revision,
            actions: vec![Action::FreezeSpec],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    let revision = workspace.head().clone();
    workspace
        .candidate_create(&revision, RelationKind::EquivalentToSpec)
        .unwrap();
    workspace
}

fn apply_one_constant_fold(workspace: &mut Workspace) {
    workspace
        .candidate_apply(&CandidateTransaction {
            candidate: CandidateId::new("c1"),
            base_revision: CandidateRevisionId::new("cr1"),
            actions: vec![CandidateAction::ApplyKnownRewrite {
                rule: FOLD_SCALAR_CONSTANTS_RULE.to_owned(),
                target: ImplOperationId::new("iop3"),
                expected_before_impl_hash: None,
            }],
        })
        .unwrap();
}

fn speculative_base(opcode: &str, self_operand: bool) -> Workspace {
    let id = WorkspaceId::new(format!("speculative-{opcode}-{self_operand}"));
    let mut workspace = Workspace::new(id.clone()).unwrap();
    let mut actions = vec![Action::CreateParameter {
        bind: "$x".to_owned(),
        name: "x".to_owned(),
        ty: "i32".parse().unwrap(),
    }];
    if !self_operand {
        actions.push(Action::CreateParameter {
            bind: "$y".to_owned(),
            name: "y".to_owned(),
            ty: "i32".parse().unwrap(),
        });
    }
    actions.extend([
        Action::CreateOp {
            bind: "$result".to_owned(),
            opcode: opcode.to_owned(),
            operands: if self_operand {
                vec!["$x".to_owned(), "$x".to_owned()]
            } else {
                vec!["$x".to_owned(), "$y".to_owned()]
            },
            attributes: BTreeMap::new(),
            region: None,
        },
        Action::SetOutput {
            name: "out".to_owned(),
            value: "$result".to_owned(),
        },
    ]);
    let built = workspace
        .apply(&Transaction {
            workspace: id.clone(),
            base_revision: RevisionId::new("r0"),
            actions,
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    workspace
        .apply(&Transaction {
            workspace: id,
            base_revision: built.revision,
            actions: vec![Action::FreezeSpec],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    workspace
}

fn chain_proposal(
    target: ImplOperationId,
    expected_before_impl_hash: agentir_core::impl_ir::ImplHash,
    operation_count: usize,
    first_opcode: &str,
) -> SpeculativeRewriteProposal {
    let mut operations = Vec::with_capacity(operation_count);
    for index in 0..operation_count {
        operations.push(ProposalOperation {
            bind: format!("$local{index}"),
            opcode: if index == 0 {
                first_opcode.to_owned()
            } else {
                "add".to_owned()
            },
            operands: vec![
                if index == 0 {
                    "$left".to_owned()
                } else {
                    format!("$local{}", index - 1)
                },
                "$right".to_owned(),
            ],
            attributes: BTreeMap::new(),
            constant: None,
            region: None,
        });
    }
    SpeculativeRewriteProposal {
        target,
        replacement: ProposedImplFragment {
            inputs: vec![
                ProposalInput {
                    bind: "$left".to_owned(),
                    value: ImplValueId::new("iv1"),
                },
                ProposalInput {
                    bind: "$right".to_owned(),
                    value: ImplValueId::new("iv2"),
                },
            ],
            operations,
            result: ProposalResult {
                value: format!("$local{}", operation_count - 1),
            },
        },
        expected_before_impl_hash,
        allow_speculative: true,
        claimed_rule: None,
    }
}

fn constant_one_proposal(
    target: &str,
    before: agentir_core::impl_ir::ImplHash,
    boundary: Vec<ProposalInput>,
) -> SpeculativeRewriteProposal {
    SpeculativeRewriteProposal {
        target: ImplOperationId::new(target),
        replacement: ProposedImplFragment {
            inputs: boundary,
            operations: vec![ProposalOperation {
                bind: "$one".to_owned(),
                opcode: "constant".to_owned(),
                operands: Vec::new(),
                attributes: BTreeMap::new(),
                constant: Some(ConstantValue::I32 { value: 1 }),
                region: None,
            }],
            result: ProposalResult {
                value: "$one".to_owned(),
            },
        },
        expected_before_impl_hash: before,
        allow_speculative: true,
        claimed_rule: None,
    }
}

fn open_speculative_candidate() -> Workspace {
    load_workspace_bytes(include_bytes!(
        "../../agentir-store/tests/fixtures/speculative-open-v5.json"
    ))
    .unwrap()
    .workspace
}

fn prepared_identity_validation() -> Workspace {
    let mut workspace = speculative_base("add", false);
    let before = workspace
        .candidate_revision(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
        .unwrap()
        .impl_hash
        .clone();
    let proposal = chain_proposal(ImplOperationId::new("iop3"), before, 1, "add");
    workspace
        .candidate_propose(
            &CandidateId::new("c1"),
            &CandidateRevisionId::new("cr1"),
            &proposal,
        )
        .unwrap();
    workspace
}

fn prepared_guarded_validation() -> Workspace {
    let mut workspace = speculative_base("div", true);
    let before = workspace
        .candidate_revision(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
        .unwrap()
        .impl_hash
        .clone();
    let proposal = constant_one_proposal(
        "iop2",
        before,
        vec![
            ProposalInput {
                bind: "$left".to_owned(),
                value: ImplValueId::new("iv1"),
            },
            ProposalInput {
                bind: "$right".to_owned(),
                value: ImplValueId::new("iv1"),
            },
        ],
    );
    workspace
        .candidate_propose(
            &CandidateId::new("c1"),
            &CandidateRevisionId::new("cr1"),
            &proposal,
        )
        .unwrap();
    workspace
}

fn prepared_known_validation() -> Workspace {
    let mut workspace = constant_match_candidate(1);
    let before = workspace
        .candidate_revision(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
        .unwrap()
        .impl_hash
        .clone();
    let mut proposal = constant_one_proposal(
        "iop3",
        before,
        vec![
            ProposalInput {
                bind: "$left".to_owned(),
                value: ImplValueId::new("iv1"),
            },
            ProposalInput {
                bind: "$right".to_owned(),
                value: ImplValueId::new("iv2"),
            },
        ],
    );
    proposal.replacement.operations[0].constant = Some(ConstantValue::I32 { value: 5 });
    proposal.allow_speculative = false;
    workspace
        .candidate_propose(
            &CandidateId::new("c1"),
            &CandidateRevisionId::new("cr1"),
            &proposal,
        )
        .unwrap();
    workspace
}

fn insert_proof_debt(step_count: usize) -> Workspace {
    let mut workspace = speculative_base("add", false);
    let mut revision = CandidateRevisionId::new("cr1");
    let mut before = workspace
        .candidate_revision(&CandidateId::new("c1"), &revision)
        .unwrap()
        .impl_hash
        .clone();
    for step in 0..step_count {
        let proposal = chain_proposal(
            ImplOperationId::new(format!("iop{}", step + 3)),
            before,
            1,
            if step % 2 == 0 { "sub" } else { "add" },
        );
        let report = workspace
            .candidate_propose(&CandidateId::new("c1"), &revision, &proposal)
            .unwrap();
        revision = report.candidate_revision;
        before = report.impl_hash;
    }
    workspace
}

fn constant_equality_chain(step_count: usize) -> Workspace {
    let id = WorkspaceId::new(format!("equality-chain-{step_count}"));
    let mut workspace = Workspace::new(id.clone()).unwrap();
    let mut actions = vec![
        Action::CreateConstant {
            bind: "$acc0".to_owned(),
            ty: "i32".parse().unwrap(),
            value: json!(1),
        },
        Action::CreateConstant {
            bind: "$one".to_owned(),
            ty: "i32".parse().unwrap(),
            value: json!(1),
        },
    ];
    for index in 0..step_count {
        actions.push(Action::CreateOp {
            bind: format!("$acc{}", index + 1),
            opcode: "add".to_owned(),
            operands: vec![format!("$acc{index}"), "$one".to_owned()],
            attributes: BTreeMap::new(),
            region: None,
        });
    }
    actions.push(Action::SetOutput {
        name: "out".to_owned(),
        value: format!("$acc{step_count}"),
    });
    let built = workspace
        .apply(&Transaction {
            workspace: id.clone(),
            base_revision: RevisionId::new("r0"),
            actions,
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    workspace
        .apply(&Transaction {
            workspace: id,
            base_revision: built.revision,
            actions: vec![Action::FreezeSpec],
            client_transaction_id: None,
            allow_branch: false,
        })
        .unwrap();
    let candidate = workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    workspace
        .equality_create(&candidate.candidate, &candidate.candidate_revision)
        .unwrap();
    workspace
}

fn saturated_equality_chain(step_count: usize) -> Workspace {
    let mut workspace = constant_equality_chain(step_count);
    let root = workspace
        .equality_query(
            &EqualitySpaceId::new("eqs1"),
            &EqualityRevisionId::new("er1"),
        )
        .unwrap();
    workspace
        .equality_saturate(
            &root.equality_space,
            &root.equality_revision,
            &root.equality_hash,
            u64::try_from(step_count).unwrap().saturating_add(1),
        )
        .unwrap();
    workspace
}

fn prepared_equality_discharge() -> Workspace {
    let mut workspace = load_workspace_bytes(include_bytes!(
        "../../agentir-store/tests/fixtures/equality-saturated-v6.json"
    ))
    .unwrap()
    .workspace;
    let identity = workspace
        .candidate_revision(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
        .unwrap()
        .clone();
    let operands = identity.impl_program.operations[&ImplOperationId::new("iop7")]
        .operands
        .clone();
    workspace
        .candidate_propose(
            &CandidateId::new("c1"),
            &CandidateRevisionId::new("cr1"),
            &SpeculativeRewriteProposal {
                target: ImplOperationId::new("iop7"),
                replacement: ProposedImplFragment {
                    inputs: vec![
                        ProposalInput {
                            bind: "$left".to_owned(),
                            value: operands[0].clone(),
                        },
                        ProposalInput {
                            bind: "$right".to_owned(),
                            value: operands[1].clone(),
                        },
                    ],
                    operations: vec![ProposalOperation {
                        bind: "$constant".to_owned(),
                        opcode: "constant".to_owned(),
                        operands: Vec::new(),
                        attributes: BTreeMap::new(),
                        constant: Some(ConstantValue::I32 { value: 25 }),
                        region: None,
                    }],
                    result: ProposalResult {
                        value: "$constant".to_owned(),
                    },
                },
                expected_before_impl_hash: identity.impl_hash,
                allow_speculative: true,
                claimed_rule: None,
            },
        )
        .unwrap();
    workspace
}

fn main() {
    let mut timings = BTreeMap::<String, Measurement>::new();
    for count in [1_usize, 10, 100] {
        timings.insert(
            format!("transaction_apply_{count}_operations"),
            measure(|| transaction_apply(count), json!({"operations": count})),
        );
    }

    let left: Shape = "[M,2*N+1]".parse().unwrap();
    let right = left.clone();
    timings.insert(
        "shape_query".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..100_000 {
                        black_box(same_shape(&left, &right));
                    }
                })
            },
            json!({"rank": 2, "queries": 100_000}),
        ),
    );

    let (mut partial, partial_revision) = partial_workspace();
    let partial_program = partial.revision(&partial_revision).unwrap().program.clone();
    timings.insert(
        "exact_state_canonical_serialization".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..10_000 {
                        black_box(canonical_bytes(&partial_program).unwrap());
                    }
                })
            },
            json!({"program_values": partial_program.values.len(), "serializations": 10_000}),
        ),
    );
    timings.insert(
        "continuation_generation".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..10_000 {
                        black_box(
                            partial
                                .continuation(
                                    &partial_revision,
                                    &HoleId::new("h1"),
                                    InteractionMode::Hybrid,
                                )
                                .unwrap(),
                        );
                    }
                })
            },
            json!({"live_values": partial_program.values.len(), "frames": 10_000}),
        ),
    );

    let semantic_workspaces = [10_usize, 100, 1_000]
        .into_iter()
        .map(|count| (count, frozen_chain(count)))
        .collect::<BTreeMap<_, _>>();
    let mut canonical_sizes = BTreeMap::<String, usize>::new();
    for (count, workspace) in &semantic_workspaces {
        let program = &workspace.revision(workspace.head()).unwrap().program;
        let canonical = canonicalize_spec(program).unwrap();
        canonical_sizes.insert(
            format!("semantic_canonical_{count}_operations"),
            canonical.bytes.len(),
        );
        timings.insert(
            format!("semantic_canonicalization_{count}_operations"),
            measure(
                || elapsed_ns(|| black_box(canonicalize_spec(program).unwrap())),
                json!({"reachable_operations": count}),
            ),
        );
    }
    let hash_workspace = semantic_workspaces.get(&100).unwrap();
    timings.insert(
        "repeated_spec_hash_query".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..1_000 {
                        black_box(
                            hash_workspace
                                .semantic_canonical(hash_workspace.head())
                                .unwrap()
                                .spec_hash,
                        );
                    }
                })
            },
            json!({"reachable_operations": 100, "queries": 1_000}),
        ),
    );

    let candidate_workspaces = [10_usize, 100, 1_000]
        .into_iter()
        .map(|count| (count, identity_candidate(count)))
        .collect::<BTreeMap<_, _>>();
    for (count, frozen) in &semantic_workspaces {
        let source = &frozen.revision(frozen.head()).unwrap().program;
        timings.insert(
            format!("identity_lowering_{count}_operations"),
            measure(
                || {
                    let mut allocator = CandidateAllocator::default();
                    elapsed_ns(|| black_box(identity_lower(source, &mut allocator).unwrap()))
                },
                json!({"operations": count}),
            ),
        );
        timings.insert(
            format!("candidate_create_{count}_operations"),
            measure(
                || {
                    let mut workspace = frozen.clone();
                    let revision = workspace.head().clone();
                    elapsed_ns(|| {
                        black_box(
                            workspace
                                .candidate_create(&revision, RelationKind::EquivalentToSpec)
                                .unwrap(),
                        )
                    })
                },
                json!({"operations": count}),
            ),
        );
        let candidate = candidate_workspaces.get(count).unwrap();
        let revision = candidate
            .candidate_revision(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
            .unwrap();
        let impl_canonical = canonicalize_impl_with_limit(
            &revision.impl_program,
            ResourceLimits::default().candidate_canonical_bytes,
        )
        .unwrap();
        canonical_sizes.insert(
            format!("impl_canonical_{count}_operations"),
            impl_canonical.bytes.len(),
        );
        timings.insert(
            format!("impl_canonicalization_{count}_operations"),
            measure(
                || {
                    elapsed_ns(|| {
                        black_box(
                            canonicalize_impl_with_limit(
                                &revision.impl_program,
                                ResourceLimits::default().candidate_canonical_bytes,
                            )
                            .unwrap(),
                        )
                    })
                },
                json!({"operations": count}),
            ),
        );
    }
    for count in [10_usize, 100, 1_000] {
        let candidate = candidate_workspaces.get(&count).unwrap();
        timings.insert(
            format!("equality_creation_{count}_operations"),
            measure(
                || {
                    let mut workspace = candidate.clone();
                    elapsed_ns(|| {
                        black_box(
                            workspace
                                .equality_create(
                                    &CandidateId::new("c1"),
                                    &CandidateRevisionId::new("cr1"),
                                )
                                .unwrap(),
                        )
                    })
                },
                json!({"operations": count}),
            ),
        );
    }

    let candidate_100 = candidate_workspaces.get(&100).unwrap();
    let candidate_100_revision = candidate_100
        .candidate_revision(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
        .unwrap();
    canonical_sizes.insert(
        "exact_candidate_state_100_operations".to_owned(),
        serde_json::to_vec(candidate_100_revision).unwrap().len(),
    );
    canonical_sizes.insert(
        "candidate_exact_v1_100_operations".to_owned(),
        serde_json::to_vec(candidate_100_revision).unwrap().len(),
    );
    timings.insert(
        "repeated_impl_hash_query".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..1_000 {
                        black_box(&candidate_100_revision.impl_hash);
                    }
                })
            },
            json!({"operations": 100, "queries": 1_000}),
        ),
    );
    timings.insert(
        "candidate_hash_query".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..1_000 {
                        black_box(&candidate_100_revision.candidate_hash);
                    }
                })
            },
            json!({"operations": 100, "queries": 1_000}),
        ),
    );
    timings.insert(
        "candidate_fork".to_owned(),
        measure(
            || {
                let mut workspace = candidate_100.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .candidate_fork(
                                &CandidateId::new("c1"),
                                &CandidateRevisionId::new("cr1"),
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"operations": 100}),
        ),
    );
    timings.insert(
        "equivalence_chain_verification".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    black_box(
                        candidate_100
                            .candidate_check(
                                &CandidateId::new("c1"),
                                &CandidateRevisionId::new("cr1"),
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"proof_edges": 1, "operations": 100}),
        ),
    );

    let fold_candidate = constant_match_candidate(1);
    timings.insert(
        "candidate_transaction_apply".to_owned(),
        measure(
            || {
                let mut workspace = fold_candidate.clone();
                elapsed_ns(|| apply_one_constant_fold(&mut workspace))
            },
            json!({"rewrite_actions": 1}),
        ),
    );
    timings.insert(
        "candidate_seal".to_owned(),
        measure(
            || {
                let mut workspace = fold_candidate.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .candidate_seal(
                                &CandidateId::new("c1"),
                                &CandidateRevisionId::new("cr1"),
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"proof_edges": 1}),
        ),
    );
    timings.insert(
        "differential_validation_small".to_owned(),
        measure(
            || {
                let spec = &fold_candidate
                    .revision(&RevisionId::new("r2"))
                    .unwrap()
                    .program;
                let implementation = &fold_candidate
                    .candidate_revision(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
                    .unwrap()
                    .impl_program;
                elapsed_ns(|| {
                    black_box(
                        agentir_eval::differential_validate(
                            spec,
                            implementation,
                            17,
                            16,
                            &ResourceLimits::default(),
                        )
                        .unwrap(),
                    )
                })
            },
            json!({"cases": 16}),
        ),
    );
    for count in [10_usize, 100, 1_000] {
        let workspace = constant_match_candidate(count);
        timings.insert(
            format!("constant_fold_{count}_match_scan"),
            measure(
                || {
                    elapsed_ns(|| {
                        let continuation = workspace
                            .candidate_continuation(
                                &CandidateId::new("c1"),
                                &CandidateRevisionId::new("cr1"),
                            )
                            .unwrap();
                        assert_eq!(continuation.matches.len(), count);
                        black_box(continuation)
                    })
                },
                json!({"matches": count}),
            ),
        );
        if count == 100 {
            timings.insert(
                "known_rewrite_applicability_scan".to_owned(),
                measure(
                    || {
                        elapsed_ns(|| {
                            black_box(
                                workspace
                                    .candidate_continuation(
                                        &CandidateId::new("c1"),
                                        &CandidateRevisionId::new("cr1"),
                                    )
                                    .unwrap(),
                            )
                        })
                    },
                    json!({"operations": 102, "matches": 100}),
                ),
            );
            timings.insert(
                "candidate_continuation_generation".to_owned(),
                measure(
                    || {
                        elapsed_ns(|| {
                            black_box(
                                workspace
                                    .candidate_continuation(
                                        &CandidateId::new("c1"),
                                        &CandidateRevisionId::new("cr1"),
                                    )
                                    .unwrap(),
                            )
                        })
                    },
                    json!({"matches": 100}),
                ),
            );
        }
    }

    let proposal_base = speculative_base("add", false);
    let proposal_revision = proposal_base
        .candidate_revision(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
        .unwrap();
    for count in [10_usize, 100, 1_000] {
        let proposal = chain_proposal(
            ImplOperationId::new("iop3"),
            proposal_revision.impl_hash.clone(),
            count,
            "sub",
        );
        timings.insert(
            format!("proposal_normalization_{count}_operations"),
            measure(
                || elapsed_ns(|| black_box(normalize_speculative_proposal(&proposal).unwrap())),
                json!({"fragment_operations": count}),
            ),
        );
        let canonical = canonicalize_proposal_with_limit(
            &proposal_revision.impl_program,
            &proposal,
            &ResourceLimits::default(),
        )
        .unwrap();
        canonical_sizes.insert(
            format!("proposal_canonical_{count}_operations"),
            canonical.bytes.len(),
        );
        timings.insert(
            format!("proposal_hash_{count}_operations"),
            measure(
                || {
                    elapsed_ns(|| {
                        black_box(
                            canonicalize_proposal_with_limit(
                                &proposal_revision.impl_program,
                                &proposal,
                                &ResourceLimits::default(),
                            )
                            .unwrap(),
                        )
                    })
                },
                json!({"fragment_operations": count, "canonical_bytes": canonical.bytes.len()}),
            ),
        );
    }
    let speculative_proposal = chain_proposal(
        ImplOperationId::new("iop3"),
        proposal_revision.impl_hash.clone(),
        1,
        "sub",
    );
    timings.insert(
        "speculative_transaction_apply".to_owned(),
        measure(
            || {
                let mut workspace = proposal_base.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .candidate_propose(
                                &CandidateId::new("c1"),
                                &CandidateRevisionId::new("cr1"),
                                &speculative_proposal,
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"fragment_operations": 1, "new_obligations": 1}),
        ),
    );
    for steps in [1_usize, 10, 100] {
        timings.insert(
            format!("proof_debt_insertion_{steps}_steps"),
            measure(
                || elapsed_ns(|| black_box(insert_proof_debt(steps))),
                json!({"speculative_steps": steps}),
            ),
        );
    }

    let open_speculative = open_speculative_candidate();
    let open_revision = open_speculative
        .candidate_revision(&CandidateId::new("c1"), &CandidateRevisionId::new("cr2"))
        .unwrap();
    canonical_sizes.insert(
        "candidate_exact_v2_speculative".to_owned(),
        serde_json::to_vec(open_revision).unwrap().len(),
    );
    canonical_sizes.insert(
        "proof_debt_speculative".to_owned(),
        serde_json::to_vec(&open_revision.proof_debt).unwrap().len(),
    );
    timings.insert(
        "proof_frontier_query".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    black_box(
                        open_speculative
                            .candidate_check(
                                &CandidateId::new("c1"),
                                &CandidateRevisionId::new("cr2"),
                            )
                            .unwrap()
                            .proof_frontier,
                    )
                })
            },
            json!({"proof_debt": 1}),
        ),
    );
    timings.insert(
        "candidate_hash_v2".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    black_box(
                        open_speculative
                            .candidate_check(
                                &CandidateId::new("c1"),
                                &CandidateRevisionId::new("cr2"),
                            )
                            .unwrap()
                            .candidate_hash,
                    )
                })
            },
            json!({"proof_debt": 1, "hash_version": 2}),
        ),
    );
    timings.insert(
        "candidate_continuation_speculative_escape".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    black_box(
                        open_speculative
                            .candidate_continuation(
                                &CandidateId::new("c1"),
                                &CandidateRevisionId::new("cr2"),
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"proof_debt": 1, "escape_schemas": 1}),
        ),
    );

    let known_validation = prepared_known_validation();
    timings.insert(
        "known_rewrite_recognition".to_owned(),
        measure(
            || {
                let mut workspace = known_validation.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .candidate_translation_check(
                                &CandidateId::new("c1"),
                                &CandidateRevisionId::new("cr2"),
                                &ProposalId::new("p1"),
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"production_rules": 1}),
        ),
    );
    timings.insert(
        "unsupported_translation_validation".to_owned(),
        measure(
            || {
                let mut workspace = open_speculative.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .candidate_translation_check(
                                &CandidateId::new("c1"),
                                &CandidateRevisionId::new("cr2"),
                                &ProposalId::new("p1"),
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"proof_debt": 1}),
        ),
    );
    let identity_validation = prepared_identity_validation();
    timings.insert(
        "canonical_identity_validation".to_owned(),
        measure(
            || {
                let mut workspace = identity_validation.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .candidate_translation_check(
                                &CandidateId::new("c1"),
                                &CandidateRevisionId::new("cr2"),
                                &ProposalId::new("p1"),
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"equal_impl_hashes": true}),
        ),
    );
    let guarded_validation = prepared_guarded_validation();
    timings.insert(
        "guarded_validation".to_owned(),
        measure(
            || {
                let mut workspace = guarded_validation.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .candidate_translation_check(
                                &CandidateId::new("c1"),
                                &CandidateRevisionId::new("cr2"),
                                &ProposalId::new("p1"),
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"guard_dependencies": 1, "fallback_depth": 1}),
        ),
    );

    let guarded_workspace = load_workspace_bytes(include_bytes!(
        "../../agentir-store/tests/fixtures/guarded-candidate-v5.json"
    ))
    .unwrap()
    .workspace;
    for (name, input, succeeds) in [
        ("guarded_evaluation_guard_true", 7, true),
        ("guarded_evaluation_guard_false_fallback", 0, false),
    ] {
        timings.insert(
            name.to_owned(),
            measure(
                || {
                    elapsed_ns(|| {
                        let result = agentir_eval::evaluate_candidate_with_limits(
                            guarded_workspace.candidate_forest(),
                            &CandidateId::new("c1"),
                            &CandidateRevisionId::new("cr3"),
                            &BTreeMap::from([("x".to_owned(), json!(input))]),
                            &ResourceLimits::default(),
                        );
                        assert_eq!(result.is_ok(), succeeds);
                        black_box(result)
                    })
                },
                json!({"guard_dependencies": 1, "guard_true": succeeds}),
            ),
        );
    }

    let speculative_spec = &open_speculative
        .revision(&RevisionId::new("r2"))
        .unwrap()
        .program;
    let refutation = agentir_eval::differential_validate_candidate(
        speculative_spec,
        open_speculative.candidate_forest(),
        &CandidateId::new("c1"),
        &CandidateRevisionId::new("cr2"),
        17,
        16,
        &ResourceLimits::default(),
    )
    .unwrap();
    assert!(!refutation.passed);
    timings.insert(
        "refutation_counterexample_publication".to_owned(),
        measure(
            || {
                let mut workspace = open_speculative.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .candidate_record_validation(
                                &CandidateId::new("c1"),
                                &CandidateRevisionId::new("cr2"),
                                refutation.clone(),
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"executed_cases": refutation.executed_cases}),
        ),
    );

    for count in [10_usize, 100, 1_000] {
        timings.insert(
            format!("constraint_fact_insertion_{count}"),
            measure(|| fact_insertion(count), json!({"facts": count})),
        );
        let facts = constraint_facts(count);
        let left: Shape = "[S0]".parse().unwrap();
        let right: Shape = format!("[S{count}]").parse().unwrap();
        timings.insert(
            format!("constraint_query_{count}"),
            measure(
                || elapsed_ns(|| black_box(facts.query_shapes(&left, &right).unwrap())),
                json!({"facts": count}),
            ),
        );
    }
    timings.insert(
        "transitive_equality_discharge".to_owned(),
        measure(transitive_discharge, json!({"facts": 2})),
    );
    timings.insert(
        "constraint_contradiction_detection".to_owned(),
        measure(contradiction_detection, json!({"accepted_facts": 1})),
    );
    timings.insert(
        "resource_rejection_fast_path".to_owned(),
        measure(
            resource_rejection,
            json!({"actions_attempted": 2, "limit": 1}),
        ),
    );

    let equality_root_bytes =
        include_bytes!("../../agentir-store/tests/fixtures/equality-root-v6.json");
    let equality_partial_bytes =
        include_bytes!("../../agentir-store/tests/fixtures/equality-partially-expanded-v6.json");
    let equality_saturated_bytes =
        include_bytes!("../../agentir-store/tests/fixtures/equality-saturated-v6.json");
    let equality_root = load_workspace_bytes(equality_root_bytes).unwrap().workspace;
    let equality_root_query = equality_root
        .equality_query(
            &EqualitySpaceId::new("eqs1"),
            &EqualityRevisionId::new("er1"),
        )
        .unwrap();
    timings.insert(
        "equality_one_step_expansion".to_owned(),
        measure(
            || {
                let mut workspace = equality_root.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .equality_expand(
                                &equality_root_query.equality_space,
                                &equality_root_query.equality_revision,
                                &equality_root_query.equality_hash,
                                1,
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"work_items": 1}),
        ),
    );
    for steps in [1_usize, 10, 100] {
        let root = constant_equality_chain(steps);
        let query = root
            .equality_query(
                &EqualitySpaceId::new("eqs1"),
                &EqualityRevisionId::new("er1"),
            )
            .unwrap();
        timings.insert(
            format!("equality_expansion_{steps}_steps"),
            measure(
                || {
                    let mut workspace = root.clone();
                    elapsed_ns(|| {
                        black_box(
                            workspace
                                .equality_saturate(
                                    &query.equality_space,
                                    &query.equality_revision,
                                    &query.equality_hash,
                                    u64::try_from(steps).unwrap(),
                                )
                                .unwrap(),
                        )
                    })
                },
                json!({"work_items": steps}),
            ),
        );
    }
    timings.insert(
        "equality_node_hash_cons_lookup".to_owned(),
        measure(
            || {
                let mut workspace = equality_root.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .equality_saturate(
                                &equality_root_query.equality_space,
                                &equality_root_query.equality_revision,
                                &equality_root_query.equality_hash,
                                100,
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"semantic_nodes": 5, "merged_results": 1}),
        ),
    );
    timings.insert(
        "equality_edge_deduplication".to_owned(),
        measure(
            || {
                let mut workspace = equality_root.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .equality_saturate(
                                &equality_root_query.equality_space,
                                &equality_root_query.equality_revision,
                                &equality_root_query.equality_hash,
                                100,
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"proof_edges": 5}),
        ),
    );
    timings.insert(
        "equality_saturation_to_fixed_point".to_owned(),
        measure(
            || {
                let mut workspace = equality_root.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .equality_saturate(
                                &equality_root_query.equality_space,
                                &equality_root_query.equality_revision,
                                &equality_root_query.equality_hash,
                                100,
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"semantic_nodes": 5, "proof_edges": 5}),
        ),
    );
    let equality_partial = load_workspace_bytes(equality_partial_bytes)
        .unwrap()
        .workspace;
    let equality_partial_query = equality_partial
        .equality_query(
            &EqualitySpaceId::new("eqs1"),
            &EqualityRevisionId::new("er2"),
        )
        .unwrap();
    timings.insert(
        "equality_resumed_saturation".to_owned(),
        measure(
            || {
                let mut workspace = equality_partial.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .equality_saturate(
                                &equality_partial_query.equality_space,
                                &equality_partial_query.equality_revision,
                                &equality_partial_query.equality_hash,
                                100,
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"already_processed": 1, "remaining_work": 2}),
        ),
    );
    let equality_saturated = load_workspace_bytes(equality_saturated_bytes)
        .unwrap()
        .workspace;
    timings.insert(
        "equality_hash_query".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..1_000 {
                        black_box(
                            equality_saturated
                                .equality_query(
                                    &EqualitySpaceId::new("eqs1"),
                                    &EqualityRevisionId::new("er2"),
                                )
                                .unwrap()
                                .equality_hash,
                        );
                    }
                })
            },
            json!({"queries": 1_000, "semantic_nodes": 5}),
        ),
    );

    for depth in [1_usize, 10, 100] {
        let workspace = saturated_equality_chain(depth);
        let target = EqualityNodeId::new(format!("en{}", depth + 1));
        timings.insert(
            format!("equality_proof_explanation_{depth}_edges"),
            measure(
                || {
                    elapsed_ns(|| {
                        black_box(
                            workspace
                                .equality_explain(
                                    &EqualitySpaceId::new("eqs1"),
                                    &EqualityRevisionId::new("er2"),
                                    &target,
                                )
                                .unwrap(),
                        )
                    })
                },
                json!({"proof_edges": depth}),
            ),
        );
    }

    let equality_discharge = prepared_equality_discharge();
    let equality_discharge_query = equality_discharge
        .equality_query(
            &EqualitySpaceId::new("eqs1"),
            &EqualityRevisionId::new("er2"),
        )
        .unwrap();
    timings.insert(
        "equality_backed_debt_discharge".to_owned(),
        measure(
            || {
                let mut workspace = equality_discharge.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .candidate_equality_check(
                                &CandidateId::new("c1"),
                                &CandidateRevisionId::new("cr2"),
                                &ProposalId::new("p1"),
                                &equality_discharge_query.equality_space,
                                &equality_discharge_query.equality_revision,
                                &equality_discharge_query.equality_hash,
                                &EqualityNodeId::new("en5"),
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"proof_edges": 3, "proof_debt": 1}),
        ),
    );
    timings.insert(
        "equality_materialization".to_owned(),
        measure(
            || {
                let mut workspace = equality_saturated.clone();
                let query = workspace
                    .equality_query(
                        &EqualitySpaceId::new("eqs1"),
                        &EqualityRevisionId::new("er2"),
                    )
                    .unwrap();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .equality_materialize(
                                &query.equality_space,
                                &query.equality_revision,
                                &query.equality_hash,
                                &EqualityNodeId::new("en5"),
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"proof_edges": 3}),
        ),
    );

    let equality_root_revision = equality_root
        .equality_store()
        .revision(
            &EqualitySpaceId::new("eqs1"),
            &EqualityRevisionId::new("er1"),
        )
        .unwrap();
    let equality_saturated_revision = equality_saturated
        .equality_store()
        .revision(
            &EqualitySpaceId::new("eqs1"),
            &EqualityRevisionId::new("er2"),
        )
        .unwrap();
    let equality_explanation = equality_saturated
        .equality_explain(
            &EqualitySpaceId::new("eqs1"),
            &EqualityRevisionId::new("er2"),
            &EqualityNodeId::new("en5"),
        )
        .unwrap();
    canonical_sizes.insert(
        "equality_root".to_owned(),
        serde_json::to_vec(equality_root_revision).unwrap().len(),
    );
    canonical_sizes.insert(
        "equality_nodes".to_owned(),
        serde_json::to_vec(&equality_saturated_revision.nodes)
            .unwrap()
            .len(),
    );
    canonical_sizes.insert(
        "equality_edges".to_owned(),
        serde_json::to_vec(&equality_saturated_revision.edges)
            .unwrap()
            .len(),
    );
    canonical_sizes.insert(
        "equality_worklist".to_owned(),
        serde_json::to_vec(
            &equality_partial
                .equality_store()
                .revision(
                    &EqualitySpaceId::new("eqs1"),
                    &EqualityRevisionId::new("er2"),
                )
                .unwrap()
                .worklist,
        )
        .unwrap()
        .len(),
    );
    canonical_sizes.insert(
        "equality_proof_explanation".to_owned(),
        serde_json::to_vec(&equality_explanation).unwrap().len(),
    );
    let equality_discharged_bytes =
        include_bytes!("../../agentir-store/tests/fixtures/equality-discharged-v6.json");
    let equality_discharged = load_workspace_bytes(equality_discharged_bytes)
        .unwrap()
        .workspace;
    let equality_candidate_v3 = equality_discharged
        .candidate_revision(&CandidateId::new("c1"), &CandidateRevisionId::new("cr3"))
        .unwrap();
    canonical_sizes.insert(
        "candidate_v3_equality_proof".to_owned(),
        serde_json::to_vec(&equality_candidate_v3.equality_proofs)
            .unwrap()
            .len(),
    );

    let legacy_v1: WorkspaceArchiveV1 = serde_json::from_slice(include_bytes!(
        "../../agentir-store/tests/fixtures/minimal-v1.json"
    ))
    .unwrap();
    timings.insert(
        "archive_v1_to_v2_migration".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..100 {
                        black_box(migrate_archive_v1_to_v2(legacy_v1.clone()).unwrap());
                    }
                })
            },
            json!({"events": legacy_v1.snapshot.events.len(), "migrations": 100}),
        ),
    );
    let legacy_v2_bytes = include_bytes!("../../agentir-store/tests/fixtures/minimal-v2.json");
    let legacy_v2: WorkspaceArchiveV2 = serde_json::from_slice(legacy_v2_bytes).unwrap();
    timings.insert(
        "archive_v2_to_v3_migration".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..100 {
                        black_box(migrate_archive_v2_to_v3(legacy_v2.clone()).unwrap());
                    }
                })
            },
            json!({"events": legacy_v2.snapshot.events.len(), "migrations": 100}),
        ),
    );
    timings.insert(
        "archive_v2_load_replay".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..100 {
                        black_box(load_workspace_bytes(legacy_v2_bytes).unwrap());
                    }
                })
            },
            json!({"archive_bytes": legacy_v2_bytes.len(), "loads": 100}),
        ),
    );
    let v3_bytes = include_bytes!("../../agentir-store/tests/fixtures/saxpy-v3.json");
    let legacy_v3: WorkspaceArchiveV3 = serde_json::from_slice(v3_bytes).unwrap();
    timings.insert(
        "archive_v3_to_v4_migration".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..100 {
                        black_box(migrate_archive_v3_to_v4(legacy_v3.clone()).unwrap());
                    }
                })
            },
            json!({"events": legacy_v3.snapshot.events.len(), "migrations": 100}),
        ),
    );
    timings.insert(
        "archive_v3_load_replay".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..100 {
                        black_box(load_workspace_bytes(v3_bytes).unwrap());
                    }
                })
            },
            json!({"archive_bytes": v3_bytes.len(), "loads": 100}),
        ),
    );
    let mixed_bytes = include_bytes!("../../agentir-store/tests/fixtures/mixed-v3.json");
    timings.insert(
        "mixed_semantics_archive_replay".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..100 {
                        black_box(load_workspace_bytes(mixed_bytes).unwrap());
                    }
                })
            },
            json!({"archive_bytes": mixed_bytes.len(), "semantics_versions": [1, 2], "loads": 100}),
        ),
    );
    let v4_without_candidates =
        include_bytes!("../../agentir-store/tests/fixtures/saxpy-frozen-v4.json");
    let v4_with_candidates =
        include_bytes!("../../agentir-store/tests/fixtures/candidate-rewrite-sealed-v4.json");
    timings.insert(
        "archive_v4_load_replay_without_candidates".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..100 {
                        black_box(load_workspace_bytes(v4_without_candidates).unwrap());
                    }
                })
            },
            json!({"archive_bytes": v4_without_candidates.len(), "loads": 100}),
        ),
    );
    timings.insert(
        "archive_v4_load_replay_with_candidate_history".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..100 {
                        black_box(load_workspace_bytes(v4_with_candidates).unwrap());
                    }
                })
            },
            json!({"archive_bytes": v4_with_candidates.len(), "loads": 100}),
        ),
    );
    canonical_sizes.insert(
        "archive_v4_without_candidates".to_owned(),
        v4_without_candidates.len(),
    );
    canonical_sizes.insert(
        "archive_v4_with_candidate_history".to_owned(),
        v4_with_candidates.len(),
    );
    let legacy_v4: WorkspaceArchiveV4 = serde_json::from_slice(v4_with_candidates).unwrap();
    timings.insert(
        "archive_v4_to_v5_migration".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..25 {
                        black_box(migrate_archive_v4_to_v5(legacy_v4.clone()).unwrap());
                    }
                })
            },
            json!({"candidate_events": legacy_v4.snapshot.candidate_forest.events.len(), "migrations": 25}),
        ),
    );
    for (name, bytes) in [
        (
            "v5_replay_exact_only",
            include_bytes!("../../agentir-store/tests/fixtures/migrated-v4-exact-v5.json")
                .as_slice(),
        ),
        (
            "v5_replay_speculative",
            include_bytes!("../../agentir-store/tests/fixtures/speculative-open-v5.json")
                .as_slice(),
        ),
        (
            "v5_replay_guarded",
            include_bytes!("../../agentir-store/tests/fixtures/guarded-candidate-v5.json")
                .as_slice(),
        ),
        (
            "v5_replay_refuted",
            include_bytes!("../../agentir-store/tests/fixtures/refuted-candidate-v5.json")
                .as_slice(),
        ),
    ] {
        timings.insert(
            name.to_owned(),
            measure(
                || {
                    elapsed_ns(|| {
                        for _ in 0..25 {
                            black_box(load_workspace_bytes(bytes).unwrap());
                        }
                    })
                },
                json!({"archive_bytes": bytes.len(), "loads": 25}),
            ),
        );
        canonical_sizes.insert(format!("archive_{name}"), bytes.len());
    }
    let legacy_v5_bytes = include_bytes!("../../agentir-store/tests/fixtures/minimal-v5.json");
    let legacy_v5: WorkspaceArchiveV5 = serde_json::from_slice(legacy_v5_bytes).unwrap();
    timings.insert(
        "archive_v5_to_v6_migration".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..25 {
                        black_box(migrate_archive_v5_to_v6(legacy_v5.clone()).unwrap());
                    }
                })
            },
            json!({"candidate_events": legacy_v5.snapshot.candidate_forest.events.len(), "migrations": 25}),
        ),
    );
    for (name, bytes) in [
        ("v6_replay_root_only", equality_root_bytes.as_slice()),
        ("v6_replay_expanded", equality_partial_bytes.as_slice()),
        ("v6_replay_saturated", equality_saturated_bytes.as_slice()),
        ("v6_replay_discharged", equality_discharged_bytes.as_slice()),
        (
            "v6_replay_materialized",
            include_bytes!("../../agentir-store/tests/fixtures/equality-materialized-v6.json")
                .as_slice(),
        ),
    ] {
        timings.insert(
            name.to_owned(),
            measure(
                || elapsed_ns(|| black_box(load_workspace_bytes(bytes).unwrap())),
                json!({"archive_bytes": bytes.len(), "loads": 1}),
            ),
        );
        canonical_sizes.insert(format!("archive_{name}"), bytes.len());
    }

    let memory_candidates = [10_usize, 100, 1_000]
        .into_iter()
        .map(|count| (count, tensor_memory_candidate(count)))
        .collect::<BTreeMap<_, _>>();
    for (count, candidate) in &memory_candidates {
        timings.insert(
            format!("memory_fresh_bufferization_{count}_operations"),
            measure(
                || {
                    let mut workspace = candidate.clone();
                    elapsed_ns(|| {
                        black_box(
                            workspace
                                .memory_create(
                                    &CandidateId::new("c1"),
                                    &CandidateRevisionId::new("cr1"),
                                )
                                .unwrap(),
                        )
                    })
                },
                json!({"operations": count, "buffers": count + 1}),
            ),
        );
    }

    let schedule_roots = [10_usize, 100, 1_000]
        .into_iter()
        .map(|operation_count| {
            let mut workspace = tensor_memory_candidate(operation_count);
            let memory = workspace
                .memory_create(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
                .unwrap();
            workspace
                .target_create(TargetProfile::GenericGpuV1)
                .unwrap();
            let base = workspace.clone();
            timings.insert(
                format!("schedule_serial_creation_{operation_count}_operations"),
                measure(
                    || {
                        let mut sample = base.clone();
                        elapsed_ns(|| {
                            black_box(
                                sample
                                    .schedule_create(
                                        &memory.query.memory_plan,
                                        &memory.query.memory_revision,
                                        &TargetManifestId::new("tm1"),
                                        &TargetManifestRevisionId::new("tmr1"),
                                    )
                                    .unwrap(),
                            )
                        })
                    },
                    json!({"operations": operation_count, "axes": operation_count}),
                ),
            );
            workspace
                .schedule_create(
                    &memory.query.memory_plan,
                    &memory.query.memory_revision,
                    &TargetManifestId::new("tm1"),
                    &TargetManifestRevisionId::new("tmr1"),
                )
                .unwrap();
            (operation_count, workspace)
        })
        .collect::<BTreeMap<_, _>>();
    for (operation_count, workspace) in &schedule_roots {
        timings.insert(
            format!("schedule_iteration_domain_construction_{operation_count}_axes"),
            measure(
                || {
                    elapsed_ns(|| {
                        black_box(
                            workspace
                                .schedule_check(
                                    &SchedulePlanId::new("sp1"),
                                    &ScheduleRevisionId::new("sr1"),
                                )
                                .unwrap(),
                        )
                    })
                },
                json!({"axes": operation_count}),
            ),
        );
    }
    let schedule_workspace = schedule_roots[&100].clone();
    let schedule_plan = schedule_workspace
        .schedule_store()
        .plan(&SchedulePlanId::new("sp1"))
        .unwrap();
    let schedule_revision = schedule_plan
        .revisions
        .get(&ScheduleRevisionId::new("sr1"))
        .unwrap();
    let target = schedule_workspace
        .target_store()
        .manifest(
            &TargetManifestId::new("tm1"),
            &TargetManifestRevisionId::new("tmr1"),
        )
        .unwrap();
    let target_bytes = canonical_target_bytes(target).unwrap();
    let schedule_bytes = canonical_schedule_bytes(schedule_plan, schedule_revision).unwrap();
    canonical_sizes.insert("target_manifest".to_owned(), target_bytes.len());
    canonical_sizes.insert(
        "schedule_serial_exact_state".to_owned(),
        schedule_bytes.len(),
    );
    canonical_sizes.insert(
        "schedule_axes_and_domains".to_owned(),
        serde_json::to_vec(&(
            &schedule_revision.program.axes,
            &schedule_revision.program.domains,
        ))
        .unwrap()
        .len(),
    );
    canonical_sizes.insert(
        "schedule_dependencies".to_owned(),
        serde_json::to_vec(&schedule_revision.program.dependencies)
            .unwrap()
            .len(),
    );
    canonical_sizes.insert(
        "schedule_fusion_facts".to_owned(),
        serde_json::to_vec(&schedule_revision.program.fusion_groups)
            .unwrap()
            .len(),
    );
    canonical_sizes.insert(
        "schedule_bindings".to_owned(),
        serde_json::to_vec(
            &schedule_revision
                .program
                .axes
                .values()
                .filter_map(|axis| axis.binding.as_ref())
                .collect::<Vec<_>>(),
        )
        .unwrap()
        .len(),
    );
    canonical_sizes.insert(
        "schedule_resource_estimate".to_owned(),
        serde_json::to_vec(&schedule_revision.program.resource_estimate)
            .unwrap()
            .len(),
    );
    canonical_sizes.insert(
        "schedule_certificates".to_owned(),
        serde_json::to_vec(&schedule_revision.certificates)
            .unwrap()
            .len(),
    );
    timings.insert(
        "schedule_dependency_analysis_100_operations".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    black_box(
                        schedule_workspace
                            .schedule_check(
                                &SchedulePlanId::new("sp1"),
                                &ScheduleRevisionId::new("sr1"),
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"operations": 100}),
        ),
    );
    timings.insert(
        "schedule_verification_100_operations".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    black_box(
                        schedule_workspace
                            .schedule_check(
                                &SchedulePlanId::new("sp1"),
                                &ScheduleRevisionId::new("sr1"),
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"operations": 100}),
        ),
    );
    timings.insert(
        "schedule_canonicalization_100_operations".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    black_box(canonical_schedule_bytes(schedule_plan, schedule_revision).unwrap())
                })
            },
            json!({"operations": 100}),
        ),
    );
    timings.insert(
        "schedule_hash_query".to_owned(),
        measure(
            || elapsed_ns(|| black_box(&schedule_revision.schedule_hash)),
            json!({"queries": 1}),
        ),
    );
    let transaction_for = |action| ScheduleTransaction {
        schedule_plan: SchedulePlanId::new("sp1"),
        base_schedule_revision: ScheduleRevisionId::new("sr1"),
        expected_schedule_hash: schedule_revision.schedule_hash.clone(),
        expected_memory_hash: schedule_revision.memory_hash.clone(),
        expected_target_hash: schedule_revision.target_hash.clone(),
        actions: vec![action],
    };
    for (name, action) in [
        (
            "schedule_exact_split",
            ScheduleAction::SplitAxis {
                axis: ScheduleAxisId::new("sa1"),
                factor: 4,
            },
        ),
        (
            "schedule_exact_tile",
            ScheduleAction::TileAxes {
                axes: vec![ScheduleAxisId::new("sa1")],
                tile_sizes: vec![4],
            },
        ),
        (
            "schedule_remainder_construction",
            ScheduleAction::SplitAxis {
                axis: ScheduleAxisId::new("sa1"),
                factor: 3,
            },
        ),
        (
            "schedule_legal_fusion",
            ScheduleAction::FuseOperations {
                producer: ScheduleNodeId::new("sn1"),
                consumer: ScheduleNodeId::new("sn2"),
            },
        ),
        (
            "schedule_binding_legality",
            ScheduleAction::BindAxis {
                axis: ScheduleAxisId::new("sa1"),
                level: BindingLevel::GridX,
            },
        ),
        (
            "schedule_unroll_proof",
            ScheduleAction::UnrollAxis {
                axis: ScheduleAxisId::new("sa1"),
                factor: 4,
            },
        ),
    ] {
        let transaction = transaction_for(action);
        timings.insert(
            name.to_owned(),
            measure(
                || {
                    let mut sample = schedule_workspace.clone();
                    elapsed_ns(|| black_box(sample.schedule_apply(&transaction).unwrap()))
                },
                json!({"actions": 1}),
            ),
        );
    }
    for (name, action) in [
        (
            "schedule_rejected_fusion_fast_path",
            ScheduleAction::FuseOperations {
                producer: ScheduleNodeId::new("sn2"),
                consumer: ScheduleNodeId::new("sn1"),
            },
        ),
        (
            "schedule_rejected_vectorization",
            ScheduleAction::VectorizeAxis {
                axis: ScheduleAxisId::new("sa1"),
                width: 16,
            },
        ),
    ] {
        let transaction = transaction_for(action);
        timings.insert(
            name.to_owned(),
            measure(
                || {
                    let mut sample = schedule_workspace.clone();
                    elapsed_ns(|| black_box(sample.schedule_apply(&transaction).unwrap_err()))
                },
                json!({"actions": 1}),
            ),
        );
    }
    timings.insert(
        "schedule_target_resource_estimation".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    black_box(
                        schedule_workspace
                            .schedule_resource_query(
                                &SchedulePlanId::new("sp1"),
                                &ScheduleRevisionId::new("sr1"),
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"nodes": 100}),
        ),
    );
    timings.insert(
        "schedule_fork".to_owned(),
        measure(
            || {
                let mut sample = schedule_workspace.clone();
                elapsed_ns(|| {
                    black_box(
                        sample
                            .schedule_fork(
                                &SchedulePlanId::new("sp1"),
                                &ScheduleRevisionId::new("sr1"),
                                &schedule_revision.schedule_hash,
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"revisions": 1}),
        ),
    );
    timings.insert(
        "schedule_seal".to_owned(),
        measure(
            || {
                let mut sample = schedule_workspace.clone();
                elapsed_ns(|| {
                    black_box(
                        sample
                            .schedule_seal(
                                &SchedulePlanId::new("sp1"),
                                &ScheduleRevisionId::new("sr1"),
                                &schedule_revision.schedule_hash,
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"revisions": 1}),
        ),
    );
    let schedule_archive = agentir_store::encode_workspace_archive(&schedule_workspace).unwrap();
    canonical_sizes.insert(
        "archive_v8_serial_schedule".to_owned(),
        schedule_archive.len(),
    );
    timings.insert(
        "archive_v8_replay_serial_schedule".to_owned(),
        measure(
            || elapsed_ns(|| black_box(load_workspace_bytes(&schedule_archive).unwrap())),
            json!({"archive_bytes": schedule_archive.len()}),
        ),
    );
    let legacy_v7 = include_bytes!("../../agentir-store/tests/fixtures/minimal-v7.json");
    timings.insert(
        "archive_v7_to_v8_migration".to_owned(),
        measure(
            || elapsed_ns(|| black_box(load_workspace_bytes(legacy_v7).unwrap())),
            json!({"archive_bytes": legacy_v7.len()}),
        ),
    );
    let serial10 = &schedule_roots[&10];
    let serial10_revision = serial10
        .schedule_store()
        .revision(&SchedulePlanId::new("sp1"), &ScheduleRevisionId::new("sr1"))
        .unwrap();
    let serial10_memory = serial10
        .memory_store()
        .revision(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
        .unwrap();
    let serial10_impl = serial10
        .memory_impl_program(&MemoryPlanId::new("mp1"))
        .unwrap();
    let serial10_inputs = BTreeMap::from([("x".to_owned(), json!([1.0, 2.0, 3.0, 4.0]))]);
    timings.insert(
        "schedule_serial_reference_evaluation".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    black_box(
                        agentir_eval::evaluate_schedule_with_limits(
                            serial10_revision,
                            serial10_memory,
                            serial10_impl,
                            &serial10_inputs,
                            &BTreeMap::new(),
                            &ResourceLimits::default(),
                        )
                        .unwrap(),
                    )
                })
            },
            json!({"operations": 10, "tensor_elements": 4}),
        ),
    );
    let mut vector_workspace = tensor_memory_candidate(10);
    let vector_memory = vector_workspace
        .memory_create(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
        .unwrap();
    let align = MemoryTransaction {
        memory_plan: MemoryPlanId::new("mp1"),
        base_memory_revision: MemoryRevisionId::new("mr1"),
        expected_memory_hash: vector_memory.query.memory_hash.clone(),
        expected_impl_hash: vector_memory.query.impl_hash.clone(),
        actions: (1..=11)
            .map(|id| MemoryAction::SetAlignment {
                buffer: BufferId::new(format!("buf{id}")),
                alignment: 16,
            })
            .collect(),
    };
    let aligned = vector_workspace.memory_apply(&align).unwrap();
    vector_workspace
        .target_create(TargetProfile::GenericGpuV1)
        .unwrap();
    let vector_schedule = vector_workspace
        .schedule_create(
            &MemoryPlanId::new("mp1"),
            &MemoryRevisionId::new("mr2"),
            &TargetManifestId::new("tm1"),
            &TargetManifestRevisionId::new("tmr1"),
        )
        .unwrap();
    let vector_transaction = ScheduleTransaction {
        schedule_plan: SchedulePlanId::new("sp1"),
        base_schedule_revision: ScheduleRevisionId::new("sr1"),
        expected_schedule_hash: vector_schedule.query.schedule_hash.clone(),
        expected_memory_hash: aligned.query.memory_hash,
        expected_target_hash: vector_schedule.query.target_hash,
        actions: vec![ScheduleAction::VectorizeAxis {
            axis: ScheduleAxisId::new("sa1"),
            width: 4,
        }],
    };
    timings.insert(
        "schedule_vectorization_proof".to_owned(),
        measure(
            || {
                let mut sample = vector_workspace.clone();
                elapsed_ns(|| black_box(sample.schedule_apply(&vector_transaction).unwrap()))
            },
            json!({"width": 4, "buffers": 11}),
        ),
    );
    let mut tiled_workspace = schedule_roots[&10].clone();
    let tiled_root = tiled_workspace
        .schedule_query(&SchedulePlanId::new("sp1"), &ScheduleRevisionId::new("sr1"))
        .unwrap();
    tiled_workspace
        .schedule_apply(&ScheduleTransaction {
            schedule_plan: SchedulePlanId::new("sp1"),
            base_schedule_revision: ScheduleRevisionId::new("sr1"),
            expected_schedule_hash: tiled_root.schedule_hash,
            expected_memory_hash: tiled_root.memory_hash,
            expected_target_hash: tiled_root.target_hash,
            actions: vec![ScheduleAction::TileAxes {
                axes: vec![ScheduleAxisId::new("sa1")],
                tile_sizes: vec![4],
            }],
        })
        .unwrap();
    let tiled_archive = agentir_store::encode_workspace_archive(&tiled_workspace).unwrap();
    canonical_sizes.insert("archive_v8_tiled_schedule".to_owned(), tiled_archive.len());
    timings.insert(
        "archive_v8_replay_tiled_schedule".to_owned(),
        measure(
            || elapsed_ns(|| black_box(load_workspace_bytes(&tiled_archive).unwrap())),
            json!({"archive_bytes": tiled_archive.len()}),
        ),
    );
    let tiled_revision = tiled_workspace
        .schedule_store()
        .revision(&SchedulePlanId::new("sp1"), &ScheduleRevisionId::new("sr2"))
        .unwrap();
    let tiled_memory = tiled_workspace
        .memory_store()
        .revision(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
        .unwrap();
    let tiled_impl = tiled_workspace
        .memory_impl_program(&MemoryPlanId::new("mp1"))
        .unwrap();
    for name in ["tiled", "remainder"] {
        timings.insert(
            format!("schedule_{name}_reference_evaluation"),
            measure(
                || {
                    elapsed_ns(|| {
                        black_box(
                            agentir_eval::evaluate_schedule_with_limits(
                                tiled_revision,
                                tiled_memory,
                                tiled_impl,
                                &serial10_inputs,
                                &BTreeMap::new(),
                                &ResourceLimits::default(),
                            )
                            .unwrap(),
                        )
                    })
                },
                json!({"tile_size": 4, "tensor_elements": 4}),
            ),
        );
    }
    let mut fused_workspace = schedule_roots[&10].clone();
    let fused_root = fused_workspace
        .schedule_query(&SchedulePlanId::new("sp1"), &ScheduleRevisionId::new("sr1"))
        .unwrap();
    fused_workspace
        .schedule_apply(&ScheduleTransaction {
            schedule_plan: SchedulePlanId::new("sp1"),
            base_schedule_revision: ScheduleRevisionId::new("sr1"),
            expected_schedule_hash: fused_root.schedule_hash,
            expected_memory_hash: fused_root.memory_hash,
            expected_target_hash: fused_root.target_hash,
            actions: vec![ScheduleAction::FuseOperations {
                producer: ScheduleNodeId::new("sn1"),
                consumer: ScheduleNodeId::new("sn2"),
            }],
        })
        .unwrap();
    let fused_archive = agentir_store::encode_workspace_archive(&fused_workspace).unwrap();
    canonical_sizes.insert("archive_v8_fused_schedule".to_owned(), fused_archive.len());
    timings.insert(
        "archive_v8_replay_fused_schedule".to_owned(),
        measure(
            || elapsed_ns(|| black_box(load_workspace_bytes(&fused_archive).unwrap())),
            json!({"archive_bytes": fused_archive.len()}),
        ),
    );
    let analyzed_memory = [10_usize, 100, 1_000]
        .into_iter()
        .map(|buffer_count| {
            let mut workspace = tensor_memory_candidate(buffer_count.saturating_sub(1));
            workspace
                .memory_create(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
                .unwrap();
            (buffer_count, workspace)
        })
        .collect::<BTreeMap<_, _>>();
    for (buffer_count, workspace) in &analyzed_memory {
        timings.insert(
            format!("memory_buffer_lifetime_analysis_{buffer_count}_buffers"),
            measure(
                || {
                    elapsed_ns(|| {
                        black_box(
                            workspace
                                .memory_check(
                                    &MemoryPlanId::new("mp1"),
                                    &MemoryRevisionId::new("mr1"),
                                )
                                .unwrap(),
                        )
                    })
                },
                json!({"buffers": buffer_count}),
            ),
        );
    }
    let mut fresh_memory = memory_candidates[&100].clone();
    let fresh_report = fresh_memory
        .memory_create(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
        .unwrap();
    let fresh_revision = fresh_memory
        .memory_store()
        .revision(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
        .unwrap();
    let fresh_plan = fresh_memory
        .memory_store()
        .plan(&MemoryPlanId::new("mp1"))
        .unwrap();
    let fresh_canonical =
        canonical_memory_bytes_with_limit(fresh_plan, fresh_revision, &ResourceLimits::default())
            .unwrap();
    canonical_sizes.insert("memory_fresh_exact_state".to_owned(), fresh_canonical.len());
    canonical_sizes.insert("memory_exact_state".to_owned(), fresh_canonical.len());
    canonical_sizes.insert(
        "memory_fresh_ir".to_owned(),
        serde_json::to_vec(&fresh_revision.program).unwrap().len(),
    );
    canonical_sizes.insert(
        "memory_buffers".to_owned(),
        serde_json::to_vec(&fresh_revision.program.buffers)
            .unwrap()
            .len(),
    );
    canonical_sizes.insert(
        "memory_accesses".to_owned(),
        serde_json::to_vec(
            &fresh_revision
                .program
                .operations
                .values()
                .flat_map(|operation| operation.accesses.iter())
                .collect::<Vec<_>>(),
        )
        .unwrap()
        .len(),
    );
    canonical_sizes.insert(
        "memory_alias_facts".to_owned(),
        serde_json::to_vec(&fresh_revision.program.alias_facts)
            .unwrap()
            .len(),
    );
    canonical_sizes.insert(
        "memory_lifetimes".to_owned(),
        serde_json::to_vec(
            &fresh_revision
                .program
                .buffers
                .values()
                .map(|buffer| &buffer.lifetime)
                .collect::<Vec<_>>(),
        )
        .unwrap()
        .len(),
    );
    timings.insert(
        "memory_verification_100_buffers".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    black_box(
                        fresh_memory
                            .memory_check(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
                            .unwrap(),
                    )
                })
            },
            json!({"buffers": 101}),
        ),
    );
    timings.insert(
        "memory_canonicalization_100_buffers".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    black_box(
                        canonical_memory_bytes_with_limit(
                            fresh_plan,
                            fresh_revision,
                            &ResourceLimits::default(),
                        )
                        .unwrap(),
                    )
                })
            },
            json!({"buffers": 101}),
        ),
    );
    timings.insert(
        "memory_hash_query".to_owned(),
        measure(
            || {
                elapsed_ns(|| {
                    for _ in 0..1_000 {
                        black_box(&fresh_report.query.memory_hash);
                    }
                })
            },
            json!({"queries": 1_000}),
        ),
    );
    for domains in [1_usize, 10, 100] {
        timings.insert(
            format!("memory_alias_query_{domains}_domains"),
            measure(
                || {
                    elapsed_ns(|| {
                        for _ in 0..domains {
                            black_box(
                                fresh_memory
                                    .memory_alias_query(
                                        &MemoryPlanId::new("mp1"),
                                        &MemoryRevisionId::new("mr1"),
                                        &BufferId::new("buf1"),
                                        &BufferId::new("buf2"),
                                    )
                                    .unwrap(),
                            );
                        }
                    })
                },
                json!({"alias_domains": domains}),
            ),
        );
    }
    let reuse_transaction = MemoryTransaction {
        memory_plan: fresh_report.query.memory_plan.clone(),
        base_memory_revision: fresh_report.query.memory_revision.clone(),
        expected_memory_hash: fresh_report.query.memory_hash.clone(),
        expected_impl_hash: fresh_report.query.impl_hash.clone(),
        actions: vec![MemoryAction::RequestInPlaceReuse {
            input: ImplValueId::new("iv100"),
            result: ImplValueId::new("iv101"),
        }],
    };
    timings.insert(
        "memory_safe_reuse_proof".to_owned(),
        measure(
            || {
                let mut workspace = fresh_memory.clone();
                elapsed_ns(|| black_box(workspace.memory_apply(&reuse_transaction).unwrap()))
            },
            json!({"buffers": 101}),
        ),
    );
    let rejected_transaction = MemoryTransaction {
        actions: vec![MemoryAction::RequestInPlaceReuse {
            input: ImplValueId::new("iv1"),
            result: ImplValueId::new("iv2"),
        }],
        ..reuse_transaction.clone()
    };
    timings.insert(
        "memory_rejected_reuse_fast_path".to_owned(),
        measure(
            || {
                let mut workspace = fresh_memory.clone();
                elapsed_ns(|| black_box(workspace.memory_apply(&rejected_transaction).unwrap_err()))
            },
            json!({"reuse_attempts": 1}),
        ),
    );
    let guarded_transaction = MemoryTransaction {
        actions: vec![MemoryAction::RequestGuardedReuse {
            input: ImplValueId::new("iv100"),
            result: ImplValueId::new("iv101"),
            guard_against: BufferId::new("buf1"),
        }],
        ..reuse_transaction.clone()
    };
    timings.insert(
        "memory_guarded_reuse_construction".to_owned(),
        measure(
            || {
                let mut workspace = fresh_memory.clone();
                elapsed_ns(|| black_box(workspace.memory_apply(&guarded_transaction).unwrap()))
            },
            json!({"guard_dependencies": 4}),
        ),
    );
    timings.insert(
        "memory_fork".to_owned(),
        measure(
            || {
                let mut workspace = fresh_memory.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .memory_fork(
                                &MemoryPlanId::new("mp1"),
                                &MemoryRevisionId::new("mr1"),
                                &fresh_report.query.memory_hash,
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"buffers": 101}),
        ),
    );
    timings.insert(
        "memory_seal".to_owned(),
        measure(
            || {
                let mut workspace = fresh_memory.clone();
                elapsed_ns(|| {
                    black_box(
                        workspace
                            .memory_seal(
                                &MemoryPlanId::new("mp1"),
                                &MemoryRevisionId::new("mr1"),
                                &fresh_report.query.memory_hash,
                            )
                            .unwrap(),
                    )
                })
            },
            json!({"buffers": 101}),
        ),
    );
    let legacy_v6_bytes = include_bytes!("../../agentir-store/tests/fixtures/minimal-v6.json");
    let legacy_v6: WorkspaceArchiveV6 = serde_json::from_slice(legacy_v6_bytes).unwrap();
    timings.insert(
        "archive_v6_to_v7_migration".to_owned(),
        measure(
            || elapsed_ns(|| black_box(migrate_archive_v6_to_v7(legacy_v6.clone()).unwrap())),
            json!({"archive_bytes": legacy_v6_bytes.len()}),
        ),
    );
    let fresh_v7 = agentir_store::encode_workspace_archive(&fresh_memory).unwrap();
    canonical_sizes.insert("archive_v7_fresh".to_owned(), fresh_v7.len());
    timings.insert(
        "archive_v7_replay_fresh".to_owned(),
        measure(
            || elapsed_ns(|| black_box(load_workspace_bytes(&fresh_v7).unwrap())),
            json!({"archive_bytes": fresh_v7.len()}),
        ),
    );
    let mut reused_memory = fresh_memory.clone();
    reused_memory.memory_apply(&reuse_transaction).unwrap();
    let reused_revision = reused_memory
        .memory_store()
        .revision(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr2"))
        .unwrap();
    canonical_sizes.insert(
        "memory_reuse_certificate".to_owned(),
        serde_json::to_vec(&reused_revision.certificates)
            .unwrap()
            .len(),
    );
    let reused_v7 = agentir_store::encode_workspace_archive(&reused_memory).unwrap();
    canonical_sizes.insert("archive_v7_reused".to_owned(), reused_v7.len());
    timings.insert(
        "archive_v7_replay_reused".to_owned(),
        measure(
            || elapsed_ns(|| black_box(load_workspace_bytes(&reused_v7).unwrap())),
            json!({"archive_bytes": reused_v7.len()}),
        ),
    );
    let mut guarded_memory = fresh_memory.clone();
    guarded_memory.memory_apply(&guarded_transaction).unwrap();
    let guarded_revision = guarded_memory
        .memory_store()
        .revision(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr2"))
        .unwrap();
    canonical_sizes.insert(
        "memory_guarded_fallback".to_owned(),
        serde_json::to_vec(&guarded_revision.program.reuse_decisions)
            .unwrap()
            .len(),
    );
    let memory_inputs = BTreeMap::from([("x".to_owned(), json!([1.0, 2.0, 3.0, 4.0]))]);
    for (name, outcome) in [("true", true), ("false", false)] {
        timings.insert(
            format!("memory_guarded_evaluation_{name}"),
            measure(
                || {
                    elapsed_ns(|| {
                        black_box(
                            agentir_eval::evaluate_memory_with_limits(
                                guarded_revision,
                                guarded_memory
                                    .memory_impl_program(&MemoryPlanId::new("mp1"))
                                    .unwrap(),
                                &memory_inputs,
                                &BTreeMap::from([(MemoryGuardId::new("mg1"), outcome)]),
                                &ResourceLimits::default(),
                            )
                            .unwrap(),
                        )
                    })
                },
                json!({"guard_outcome": outcome, "tensor_elements": 4}),
            ),
        );
    }
    let guarded_v7 = agentir_store::encode_workspace_archive(&guarded_memory).unwrap();
    canonical_sizes.insert("archive_v7_guarded".to_owned(), guarded_v7.len());
    timings.insert(
        "archive_v7_replay_guarded".to_owned(),
        measure(
            || elapsed_ns(|| black_box(load_workspace_bytes(&guarded_v7).unwrap())),
            json!({"archive_bytes": guarded_v7.len()}),
        ),
    );
    guarded_memory
        .target_create(TargetProfile::GenericGpuV1)
        .unwrap();
    guarded_memory
        .schedule_create(
            &MemoryPlanId::new("mp1"),
            &MemoryRevisionId::new("mr2"),
            &TargetManifestId::new("tm1"),
            &TargetManifestRevisionId::new("tmr1"),
        )
        .unwrap();
    let guarded_schedule = guarded_memory
        .schedule_store()
        .revision(&SchedulePlanId::new("sp1"), &ScheduleRevisionId::new("sr1"))
        .unwrap();
    let guarded_revision = guarded_memory
        .memory_store()
        .revision(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr2"))
        .unwrap();
    let guarded_impl = guarded_memory
        .memory_impl_program(&MemoryPlanId::new("mp1"))
        .unwrap();
    for (name, outcome) in [("true", true), ("false", false)] {
        timings.insert(
            format!("schedule_guarded_memory_{name}_evaluation"),
            measure(
                || {
                    elapsed_ns(|| {
                        black_box(
                            agentir_eval::evaluate_schedule_with_limits(
                                guarded_schedule,
                                guarded_revision,
                                guarded_impl,
                                &memory_inputs,
                                &BTreeMap::from([(MemoryGuardId::new("mg1"), outcome)]),
                                &ResourceLimits::default(),
                            )
                            .unwrap(),
                        )
                    })
                },
                json!({"guard_outcome": outcome, "tensor_elements": 4}),
            ),
        );
    }
    let guarded_v8 = agentir_store::encode_workspace_archive(&guarded_memory).unwrap();
    canonical_sizes.insert("archive_v8_guarded_schedule".to_owned(), guarded_v8.len());
    timings.insert(
        "archive_v8_replay_guarded_schedule".to_owned(),
        measure(
            || elapsed_ns(|| black_box(load_workspace_bytes(&guarded_v8).unwrap())),
            json!({"archive_bytes": guarded_v8.len()}),
        ),
    );
    for (name, bytes) in [
        (
            "equality_materialized",
            include_bytes!(
                "../../agentir-store/tests/fixtures/equality-materialized-memory-v7.json"
            )
            .as_slice(),
        ),
        (
            "mixed_memory_semantics",
            include_bytes!("../../agentir-store/tests/fixtures/mixed-memory-semantics-v7.json")
                .as_slice(),
        ),
    ] {
        canonical_sizes.insert(format!("archive_v7_{name}"), bytes.len());
        timings.insert(
            format!("archive_v7_replay_{name}"),
            measure(
                || elapsed_ns(|| black_box(load_workspace_bytes(bytes).unwrap())),
                json!({"archive_bytes": bytes.len()}),
            ),
        );
    }
    let equality_memory_bytes =
        include_bytes!("../../agentir-store/tests/fixtures/equality-materialized-memory-v7.json");
    let mut equality_schedule = load_workspace_bytes(equality_memory_bytes)
        .unwrap()
        .workspace;
    equality_schedule
        .target_create(TargetProfile::GenericGpuV1)
        .unwrap();
    equality_schedule
        .schedule_create(
            &MemoryPlanId::new("mp1"),
            &MemoryRevisionId::new("mr1"),
            &TargetManifestId::new("tm1"),
            &TargetManifestRevisionId::new("tmr1"),
        )
        .unwrap();
    let equality_v8 = agentir_store::encode_workspace_archive(&equality_schedule).unwrap();
    canonical_sizes.insert(
        "archive_v8_equality_materialized_schedule".to_owned(),
        equality_v8.len(),
    );
    timings.insert(
        "archive_v8_replay_equality_materialized_schedule".to_owned(),
        measure(
            || elapsed_ns(|| black_box(load_workspace_bytes(&equality_v8).unwrap())),
            json!({"archive_bytes": equality_v8.len()}),
        ),
    );

    let saxpy = load_workspace_bytes(v3_bytes).unwrap();
    let saxpy_program = &saxpy
        .workspace
        .revision(&RevisionId::new("r2"))
        .unwrap()
        .program;
    for size in [4_usize, 1_024, 65_536] {
        let inputs = BTreeMap::from([
            ("a".to_owned(), json!(2.0)),
            (
                "x".to_owned(),
                Value::Array((0..size).map(|value| json!(value as f32)).collect()),
            ),
            ("y".to_owned(), Value::Array(vec![json!(1.0); size])),
        ]);
        timings.insert(
            format!("saxpy_reference_evaluation_{size}_elements"),
            measure(
                || {
                    elapsed_ns(|| {
                        black_box(agentir_eval::evaluate(saxpy_program, &inputs).unwrap())
                    })
                },
                json!({"tensor_elements": size}),
            ),
        );
    }

    let exact_size = canonical_bytes(&partial_program).unwrap().len();
    canonical_sizes.insert("exact_state_partial_program".to_owned(), exact_size);
    for (name, source, workload) in [
        (
            "saxpy_lower_emit_validate_reference",
            include_str!("../../../examples/backend_saxpy_wgsl.jsonl"),
            json!({"operations": 1, "kernels": 1, "modules": 1}),
        ),
        (
            "serial_kernel_formation_binding_dispatch",
            include_str!("../../../examples/backend_serial.jsonl"),
            json!({"schedule": "serial", "device_required": false}),
        ),
        (
            "tiled_exact_coverage",
            include_str!("../../../examples/backend_tiled.jsonl"),
            json!({"schedule": "tiled", "device_required": false}),
        ),
        (
            "remainder_bounds_lowering",
            include_str!("../../../examples/backend_remainder.jsonl"),
            json!({"schedule": "remainder", "device_required": false}),
        ),
        (
            "legal_fusion_lowering",
            include_str!("../../../examples/backend_fused.jsonl"),
            json!({"schedule": "fused", "device_required": false}),
        ),
        (
            "vector_unroll_lowering",
            include_str!("../../../examples/backend_vectorized.jsonl"),
            json!({"vector_width": 4, "unroll_factor": 2}),
        ),
        (
            "guarded_package_construction",
            include_str!("../../../examples/backend_guarded_memory.jsonl"),
            json!({"guard": "no_overlap", "branches": 2}),
        ),
        (
            "static_reuse_binding",
            include_str!("../../../examples/backend_reuse.jsonl"),
            json!({"reuse": "compiler_proved_in_place", "dispatches": 2}),
        ),
        (
            "equality_materialized_artifact",
            include_str!("../../../examples/equality_to_artifact.jsonl"),
            json!({"source": "equality_materialization", "device_required": false}),
        ),
        (
            "rejected_lowering_fast_path",
            include_str!("../../../examples/backend_rejected_reduce.jsonl"),
            json!({"unsupported_opcode": "reduce", "publication": false}),
        ),
    ] {
        timings.insert(
            format!("stage5_{name}"),
            measure(
                || {
                    elapsed_ns(|| {
                        let mut engine = agentir_protocol::Engine::new();
                        for line in source.lines().filter(|line| !line.is_empty()) {
                            black_box(engine.process_line(line));
                        }
                    })
                },
                workload,
            ),
        );
        canonical_sizes.insert(format!("stage5_{name}_jsonl_bytes"), source.len());
    }
    for (name, bytes) in [
        (
            "serial",
            include_bytes!("../../agentir-store/tests/fixtures/backend-serial-v9.json").as_slice(),
        ),
        (
            "tiled",
            include_bytes!("../../agentir-store/tests/fixtures/backend-tiled-v9.json").as_slice(),
        ),
        (
            "fused",
            include_bytes!("../../agentir-store/tests/fixtures/backend-fused-v9.json").as_slice(),
        ),
        (
            "vectorized",
            include_bytes!("../../agentir-store/tests/fixtures/backend-vectorized-v9.json")
                .as_slice(),
        ),
        (
            "guarded",
            include_bytes!("../../agentir-store/tests/fixtures/backend-guarded-v9.json").as_slice(),
        ),
        (
            "equality_materialized",
            include_bytes!(
                "../../agentir-store/tests/fixtures/equality-materialized-artifact-v9.json"
            )
            .as_slice(),
        ),
    ] {
        canonical_sizes.insert(format!("archive_v9_{name}"), bytes.len());
        timings.insert(
            format!("archive_v9_replay_{name}"),
            measure(
                || elapsed_ns(|| black_box(load_workspace_bytes(bytes).unwrap())),
                json!({"archive_bytes": bytes.len()}),
            ),
        );
    }
    let legacy_v8 = include_bytes!("../../agentir-store/tests/fixtures/minimal-v8.json");
    timings.insert(
        "archive_v8_to_v9_migration".to_owned(),
        measure(
            || elapsed_ns(|| black_box(load_workspace_bytes(legacy_v8).unwrap())),
            json!({"archive_bytes": legacy_v8.len()}),
        ),
    );
    println!(
        "{}",
        serde_json::to_string(&json!({
            "benchmark_schema_version": 2,
            "metadata": metadata(),
            "timings": timings,
            "canonical_byte_sizes": canonical_sizes,
        }))
        .unwrap()
    );
}
