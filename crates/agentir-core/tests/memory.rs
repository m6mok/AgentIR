use agentir_core::{
    Action, ErrorCode, RevisionId, Transaction, Workspace, WorkspaceId,
    actions::{RegionArgumentSpec, RegionSpec},
    candidate::RelationKind,
    ids::{
        BufferId, CandidateId, CandidateRevisionId, ImplValueId, MemoryPlanId, MemoryRevisionId,
    },
    memory::{MemoryAction, MemoryStatus, MemoryTransaction},
    memory_ir::{
        AddressSpace, AliasProvenance, AliasRelation, ReuseDecision, alias_relation, buffer_of,
        can_reuse, last_use, lifetime_of, may_overlap, prove_static_reuse, required_alignment,
    },
    resources::ResourceLimits,
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

fn generated_memory_workspace(id: &str, map_count: usize) -> Workspace {
    let mut workspace = Workspace::new(WorkspaceId::new(id)).unwrap();
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
    for index in 0..map_count {
        actions.push(Action::CreateOp {
            bind: format!("$value{}", index + 1),
            opcode: "map".to_owned(),
            operands: vec![format!("$value{index}")],
            attributes: BTreeMap::new(),
            region: Some(map_region()),
        });
    }
    actions.push(Action::SetOutput {
        name: "out".to_owned(),
        value: format!("$value{map_count}"),
    });
    let build = transaction(&workspace, actions);
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
}

fn memory_workspace() -> Workspace {
    generated_memory_workspace("memory-tests", 2)
}

#[test]
fn fresh_bufferization_reuse_seal_and_snapshot_replay_are_exact() {
    let mut workspace = memory_workspace();
    let root = workspace
        .memory_query(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
        .unwrap();
    assert_eq!(root.fresh_buffer_count, 2);
    let reused = workspace
        .memory_apply(&MemoryTransaction {
            memory_plan: root.memory_plan.clone(),
            base_memory_revision: root.memory_revision.clone(),
            expected_memory_hash: root.memory_hash.clone(),
            expected_impl_hash: root.impl_hash.clone(),
            actions: vec![MemoryAction::RequestInPlaceReuse {
                input: ImplValueId::new("iv2"),
                result: ImplValueId::new("iv3"),
            }],
        })
        .unwrap();
    assert_eq!(reused.query.reused_buffer_count, 1);
    assert_ne!(root.memory_hash, reused.query.memory_hash);
    assert!(matches!(
        workspace
            .memory_program(&root.memory_plan, &reused.query.memory_revision)
            .unwrap()
            .reuse_decisions[&ImplValueId::new("iv3")],
        ReuseDecision::InPlace { .. }
    ));
    let sealed = workspace
        .memory_seal(
            &root.memory_plan,
            &reused.query.memory_revision,
            &reused.query.memory_hash,
        )
        .unwrap();
    assert_eq!(sealed.query.status, MemoryStatus::Sealed);

    let snapshot = workspace.snapshot();
    let (replayed, report) = Workspace::from_snapshot(snapshot.clone()).unwrap();
    assert_eq!(replayed.snapshot(), snapshot);
    assert_eq!(report.memory_plans_verified, 1);
    assert_eq!(report.memory_events_replayed, 3);
}

#[test]
fn rejected_reuse_and_resource_limit_are_atomic_and_do_not_consume_ids() {
    let mut workspace = memory_workspace();
    let root = workspace
        .memory_query(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
        .unwrap();
    let before = workspace.snapshot();
    let error = workspace
        .memory_apply(&MemoryTransaction {
            memory_plan: root.memory_plan,
            base_memory_revision: root.memory_revision,
            expected_memory_hash: root.memory_hash,
            expected_impl_hash: root.impl_hash,
            actions: vec![MemoryAction::RequestInPlaceReuse {
                input: ImplValueId::new("iv1"),
                result: ImplValueId::new("iv2"),
            }],
        })
        .expect_err("external borrowed input is never overwritten");
    assert_eq!(error.code, ErrorCode::InPlaceReuseUnsafe);
    assert_eq!(workspace.snapshot(), before);

    let limits = ResourceLimits {
        memory_guard_depth: 0,
        ..ResourceLimits::default()
    };
    workspace.set_resource_limits(limits);
    let root = workspace
        .memory_query(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
        .unwrap();
    let before = workspace.snapshot();
    let error = workspace
        .memory_apply(&MemoryTransaction {
            memory_plan: root.memory_plan,
            base_memory_revision: root.memory_revision,
            expected_memory_hash: root.memory_hash,
            expected_impl_hash: root.impl_hash,
            actions: vec![MemoryAction::RequestGuardedReuse {
                input: ImplValueId::new("iv2"),
                result: ImplValueId::new("iv3"),
                guard_against: BufferId::new("buf1"),
            }],
        })
        .expect_err("guard depth limit rejects before publication");
    assert_eq!(error.code, ErrorCode::ResourceLimitExceeded);
    assert_eq!(workspace.snapshot(), before);

    let limits = ResourceLimits {
        memory_revisions_per_workspace: 1,
        ..ResourceLimits::default()
    };
    workspace.set_resource_limits(limits);
    let root = workspace
        .memory_query(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
        .unwrap();
    let before = workspace.snapshot();
    let error = workspace
        .memory_apply(&MemoryTransaction {
            memory_plan: root.memory_plan,
            base_memory_revision: root.memory_revision,
            expected_memory_hash: root.memory_hash,
            expected_impl_hash: root.impl_hash,
            actions: vec![MemoryAction::SetAddressSpace {
                buffer: agentir_core::ids::BufferId::new("buf2"),
                address_space: AddressSpace::Private,
            }],
        })
        .expect_err("revision limit rejects atomically");
    assert_eq!(error.code, ErrorCode::ResourceLimitExceeded);
    assert_eq!(workspace.snapshot(), before);
}

#[test]
fn memory_hash_covers_physical_layout_but_not_interactive_limits() {
    let mut workspace = memory_workspace();
    let root = workspace
        .memory_query(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
        .unwrap();
    let changed = workspace
        .memory_apply(&MemoryTransaction {
            memory_plan: root.memory_plan.clone(),
            base_memory_revision: root.memory_revision,
            expected_memory_hash: root.memory_hash.clone(),
            expected_impl_hash: root.impl_hash,
            actions: vec![MemoryAction::SetAddressSpace {
                buffer: agentir_core::ids::BufferId::new("buf2"),
                address_space: AddressSpace::Private,
            }],
        })
        .unwrap();
    assert_ne!(root.memory_hash, changed.query.memory_hash);
    let hash = changed.query.memory_hash;
    workspace.set_resource_limits(ResourceLimits::hard_safety_caps());
    assert_eq!(
        workspace
            .memory_check(&root.memory_plan, &changed.query.memory_revision)
            .unwrap()
            .query
            .memory_hash,
        hash
    );
}

#[test]
fn compiler_owned_alias_and_lifetime_queries_are_deterministic_and_non_mutating() {
    let workspace = memory_workspace();
    let before = workspace.snapshot();
    let program = workspace
        .memory_program(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
        .unwrap();
    let implementation = workspace
        .memory_impl_program(&MemoryPlanId::new("mp1"))
        .unwrap();

    assert_eq!(
        buffer_of(program, &ImplValueId::new("iv2")).unwrap().id,
        BufferId::new("buf2")
    );
    assert_eq!(
        lifetime_of(program, &BufferId::new("buf2"))
            .unwrap()
            .last_use,
        last_use(implementation, &ImplValueId::new("iv2")).unwrap()
    );
    assert_eq!(
        alias_relation(program, &BufferId::new("buf2"), &BufferId::new("buf3"))
            .unwrap()
            .relation,
        AliasRelation::NoAlias
    );
    assert!(!may_overlap(program, &BufferId::new("buf2"), &BufferId::new("buf3")).unwrap());
    assert!(can_reuse(
        program,
        implementation,
        &ImplValueId::new("iv2"),
        &ImplValueId::new("iv3")
    ));
    assert_eq!(
        required_alignment(program, &BufferId::new("buf2")).unwrap(),
        4
    );
    assert_eq!(workspace.snapshot(), before);
}

#[test]
fn unverified_alias_claim_never_authorizes_reuse() {
    let workspace = memory_workspace();
    let mut program = workspace
        .memory_program(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
        .unwrap()
        .clone();
    let implementation = workspace
        .memory_impl_program(&MemoryPlanId::new("mp1"))
        .unwrap();
    let fact = program
        .alias_facts
        .iter_mut()
        .find(|fact| fact.first == BufferId::new("buf1") && fact.second == BufferId::new("buf2"))
        .unwrap();
    fact.provenance = AliasProvenance::UnverifiedClaim;

    let error = prove_static_reuse(
        &program,
        implementation,
        &ImplValueId::new("iv2"),
        &ImplValueId::new("iv3"),
    )
    .expect_err("unverified provenance is audit metadata, never proof");
    assert_eq!(error.code, ErrorCode::AliasProofMissing);
}

#[test]
fn bounded_generated_memory_roots_are_reproducible() {
    for map_count in 1..=8 {
        let first = generated_memory_workspace(&format!("generated-{map_count}"), map_count);
        let second = generated_memory_workspace(&format!("generated-{map_count}"), map_count);
        let first_query = first
            .memory_query(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
            .unwrap();
        let second_query = second
            .memory_query(&MemoryPlanId::new("mp1"), &MemoryRevisionId::new("mr1"))
            .unwrap();
        assert_eq!(first_query, second_query);
        assert_eq!(first.memory_store(), second.memory_store());
        assert_eq!(first_query.buffer_count, map_count + 1);
        assert_eq!(first_query.fresh_buffer_count, map_count);
    }
}
