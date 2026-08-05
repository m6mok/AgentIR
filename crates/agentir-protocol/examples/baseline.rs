//! Dependency-light statistical Stage 1.2 performance baseline.

use agentir_core::{
    Action, HoleId, RevisionId, Transaction, Workspace, WorkspaceId,
    canonical::canonical_bytes,
    constraints::ConstraintFacts,
    continuation::InteractionMode,
    resources::ResourceLimits,
    semantic::canonicalize_spec,
    shapes::{ShapeConstraint, same_shape},
    types::Shape,
};
use agentir_store::{
    WorkspaceArchiveV1, WorkspaceArchiveV2, load_workspace_bytes, migrate_archive_v1_to_v2,
    migrate_archive_v2_to_v3,
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
