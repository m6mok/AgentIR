use agentir_core::{
    Action, ErrorCode, RevisionId, Transaction, Workspace, WorkspaceId,
    actions::{RegionArgumentSpec, RegionSpec},
    candidate::RelationKind,
    ids::{
        BufferId, CandidateId, CandidateRevisionId, MemoryPlanId, MemoryRevisionId, ScheduleAxisId,
        ScheduleNodeId, SchedulePlanId, ScheduleRevisionId, TargetManifestId,
        TargetManifestRevisionId,
    },
    memory::{MemoryAction, MemoryTransaction},
    resources::ResourceLimits,
    schedule::{ScheduleAction, ScheduleStatus, ScheduleTransaction},
    schedule_ir::{BindingLevel, TailStrategy},
    target::TargetProfile,
};
use std::collections::BTreeMap;

fn transaction(workspace: &Workspace, actions: Vec<Action>) -> Transaction {
    Transaction {
        workspace: workspace.id().clone(),
        base_revision: workspace.head().clone(),
        actions,
        client_transaction_id: None,
        allow_branch: false,
    }
}

fn map_region() -> RegionSpec {
    RegionSpec {
        arguments: vec![RegionArgumentSpec {
            name: "element".to_owned(),
            ty: "f32".parse().unwrap(),
        }],
        captures: Vec::new(),
        operations: Vec::new(),
        yield_value: "element".to_owned(),
    }
}

fn schedule_workspace() -> Workspace {
    let mut workspace = Workspace::new(WorkspaceId::new("schedule-tests")).unwrap();
    let build = transaction(
        &workspace,
        vec![
            Action::CreateParameter {
                bind: "$x".to_owned(),
                name: "x".to_owned(),
                ty: "tensor<f32,[10]>".parse().unwrap(),
            },
            Action::CreateOp {
                bind: "$first".to_owned(),
                opcode: "map".to_owned(),
                operands: vec!["$x".to_owned()],
                attributes: BTreeMap::new(),
                region: Some(map_region()),
            },
            Action::CreateOp {
                bind: "$out".to_owned(),
                opcode: "map".to_owned(),
                operands: vec!["$first".to_owned()],
                attributes: BTreeMap::new(),
                region: Some(map_region()),
            },
            Action::SetOutput {
                name: "out".to_owned(),
                value: "$out".to_owned(),
            },
        ],
    );
    workspace.apply(&build).unwrap();
    let freeze = transaction(&workspace, vec![Action::FreezeSpec]);
    workspace.apply(&freeze).unwrap();
    workspace
        .candidate_create(&RevisionId::new("r2"), RelationKind::EquivalentToSpec)
        .unwrap();
    workspace
        .memory_create(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
        .unwrap();
    workspace
        .target_create(TargetProfile::GenericGpuV1)
        .unwrap();
    workspace
}

fn create_root(workspace: &mut Workspace, memory_revision: &str) {
    workspace
        .schedule_create(
            &MemoryPlanId::new("mp1"),
            &MemoryRevisionId::new(memory_revision),
            &TargetManifestId::new("tm1"),
            &TargetManifestRevisionId::new("tmr1"),
        )
        .unwrap();
}

#[test]
fn serial_split_fusion_binding_and_replay_are_exact() {
    let mut workspace = schedule_workspace();
    create_root(&mut workspace, "mr1");
    let root = workspace
        .schedule_query(&SchedulePlanId::new("sp1"), &ScheduleRevisionId::new("sr1"))
        .unwrap();
    assert_eq!(root.node_count, 2);
    assert_eq!(root.domain_count, 2);
    assert_eq!(root.axis_count, 2);
    assert_eq!(
        root.memory_hash,
        workspace
            .memory_query(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
            .unwrap()
            .memory_hash
    );

    let split = workspace
        .schedule_apply(&ScheduleTransaction {
            schedule_plan: root.schedule_plan.clone(),
            base_schedule_revision: root.schedule_revision.clone(),
            expected_schedule_hash: root.schedule_hash.clone(),
            expected_memory_hash: root.memory_hash.clone(),
            expected_target_hash: root.target_hash.clone(),
            actions: vec![ScheduleAction::SplitAxis {
                axis: ScheduleAxisId::new("sa1"),
                factor: 4,
            }],
        })
        .unwrap();
    assert_eq!(split.query.remainder_count, 1);
    assert_ne!(root.schedule_hash, split.query.schedule_hash);
    assert!(matches!(
        workspace
            .schedule_axis_query(
                &split.query.schedule_plan,
                &split.query.schedule_revision,
                &ScheduleAxisId::new("sa3")
            )
            .unwrap()
            .tail,
        TailStrategy::CompilerRemainder { remainder: Some(2) }
    ));

    let mut fused_workspace = schedule_workspace();
    create_root(&mut fused_workspace, "mr1");
    let root = fused_workspace
        .schedule_query(&SchedulePlanId::new("sp1"), &ScheduleRevisionId::new("sr1"))
        .unwrap();
    let fused = fused_workspace
        .schedule_apply(&ScheduleTransaction {
            schedule_plan: root.schedule_plan.clone(),
            base_schedule_revision: root.schedule_revision.clone(),
            expected_schedule_hash: root.schedule_hash.clone(),
            expected_memory_hash: root.memory_hash.clone(),
            expected_target_hash: root.target_hash.clone(),
            actions: vec![
                ScheduleAction::FuseOperations {
                    producer: ScheduleNodeId::new("sn1"),
                    consumer: ScheduleNodeId::new("sn2"),
                },
                ScheduleAction::BindAxis {
                    axis: ScheduleAxisId::new("sa1"),
                    level: BindingLevel::BlockX,
                },
            ],
        })
        .unwrap();
    assert_eq!(fused.query.fusion_count, 1);
    assert_eq!(fused.query.binding_count, 1);
    let sealed = fused_workspace
        .schedule_seal(
            &fused.query.schedule_plan,
            &fused.query.schedule_revision,
            &fused.query.schedule_hash,
        )
        .unwrap();
    assert_eq!(sealed.query.status, ScheduleStatus::Sealed);
    let snapshot = fused_workspace.snapshot();
    let (replayed, report) = Workspace::from_snapshot(snapshot.clone()).unwrap();
    assert_eq!(replayed.snapshot(), snapshot);
    assert_eq!(report.target_manifests_verified, 1);
    assert_eq!(report.schedule_plans_verified, 1);
    assert_eq!(report.schedule_events_replayed, 3);
}

#[test]
fn illegal_resource_and_vector_actions_are_atomic() {
    let mut workspace = schedule_workspace();
    create_root(&mut workspace, "mr1");
    let root = workspace
        .schedule_query(&SchedulePlanId::new("sp1"), &ScheduleRevisionId::new("sr1"))
        .unwrap();
    let before = workspace.snapshot();
    let error = workspace
        .schedule_apply(&ScheduleTransaction {
            schedule_plan: root.schedule_plan.clone(),
            base_schedule_revision: root.schedule_revision.clone(),
            expected_schedule_hash: root.schedule_hash.clone(),
            expected_memory_hash: root.memory_hash.clone(),
            expected_target_hash: root.target_hash.clone(),
            actions: vec![ScheduleAction::SetLaunchShape {
                grid: [1, 1, 1],
                workgroup: [1_025, 1, 1],
            }],
        })
        .expect_err("target capacity overflow is rejected");
    assert_eq!(error.code, ErrorCode::TargetResourceExceeded);
    assert_eq!(workspace.snapshot(), before);

    let error = workspace
        .schedule_apply(&ScheduleTransaction {
            schedule_plan: root.schedule_plan,
            base_schedule_revision: root.schedule_revision,
            expected_schedule_hash: root.schedule_hash,
            expected_memory_hash: root.memory_hash,
            expected_target_hash: root.target_hash,
            actions: vec![ScheduleAction::VectorizeAxis {
                axis: ScheduleAxisId::new("sa1"),
                width: 4,
            }],
        })
        .expect_err("fresh f32 buffers are only four-byte aligned");
    assert_eq!(error.code, ErrorCode::VectorAlignmentUnsatisfied);
    assert_eq!(workspace.snapshot(), before);
}

#[test]
fn aligned_memory_allows_exact_vectorization_without_changing_memory_hash() {
    let mut workspace = schedule_workspace();
    let memory = workspace
        .memory_query(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
        .unwrap();
    let aligned = workspace
        .memory_apply(&MemoryTransaction {
            memory_plan: memory.memory_plan,
            base_memory_revision: memory.memory_revision,
            expected_memory_hash: memory.memory_hash,
            expected_impl_hash: memory.impl_hash,
            actions: vec![
                MemoryAction::SetAlignment {
                    buffer: BufferId::new("buf1"),
                    alignment: 16,
                },
                MemoryAction::SetAlignment {
                    buffer: BufferId::new("buf2"),
                    alignment: 16,
                },
            ],
        })
        .unwrap();
    create_root(&mut workspace, "mr2");
    let root = workspace
        .schedule_query(&SchedulePlanId::new("sp1"), &ScheduleRevisionId::new("sr1"))
        .unwrap();
    let vectorized = workspace
        .schedule_apply(&ScheduleTransaction {
            schedule_plan: root.schedule_plan,
            base_schedule_revision: root.schedule_revision,
            expected_schedule_hash: root.schedule_hash,
            expected_memory_hash: root.memory_hash,
            expected_target_hash: root.target_hash,
            actions: vec![ScheduleAction::VectorizeAxis {
                axis: ScheduleAxisId::new("sa1"),
                width: 4,
            }],
        })
        .unwrap();
    assert_eq!(vectorized.query.vectorization_count, 1);
    assert_eq!(vectorized.query.memory_hash, aligned.query.memory_hash);
}

#[test]
fn stage4_limits_are_hash_independent_and_rejections_consume_no_ids() {
    let mut workspace = schedule_workspace();
    create_root(&mut workspace, "mr1");
    let root = workspace
        .schedule_query(&SchedulePlanId::new("sp1"), &ScheduleRevisionId::new("sr1"))
        .unwrap();
    let target_hash = workspace
        .target_query(
            &TargetManifestId::new("tm1"),
            &TargetManifestRevisionId::new("tmr1"),
        )
        .unwrap()
        .target_hash;
    let before = workspace.snapshot();
    let limits = ResourceLimits {
        schedule_revisions_per_workspace: 1,
        target_manifests_per_workspace: 1,
        ..ResourceLimits::default()
    };
    workspace.set_resource_limits(limits);
    let error = workspace
        .schedule_apply(&ScheduleTransaction {
            schedule_plan: root.schedule_plan,
            base_schedule_revision: root.schedule_revision,
            expected_schedule_hash: root.schedule_hash.clone(),
            expected_memory_hash: root.memory_hash,
            expected_target_hash: root.target_hash,
            actions: vec![ScheduleAction::UnrollAxis {
                axis: ScheduleAxisId::new("sa1"),
                factor: 2,
            }],
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ResourceLimitExceeded);
    assert_eq!(workspace.snapshot(), before);
    assert_eq!(
        workspace
            .schedule_query(&SchedulePlanId::new("sp1"), &ScheduleRevisionId::new("sr1"))
            .unwrap()
            .schedule_hash,
        root.schedule_hash
    );
    assert_eq!(
        workspace
            .target_create(TargetProfile::GenericGpuV1)
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimitExceeded
    );
    workspace.set_resource_limits(ResourceLimits::default());
    let second = workspace
        .target_create(TargetProfile::GenericGpuV1)
        .unwrap();
    assert_eq!(second.query.target_manifest.as_str(), "tm2");
    assert_eq!(
        workspace
            .target_query(
                &TargetManifestId::new("tm1"),
                &TargetManifestRevisionId::new("tmr1")
            )
            .unwrap()
            .target_hash,
        target_hash
    );
}
