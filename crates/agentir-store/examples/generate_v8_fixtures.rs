//! Regenerates deterministic Stage 4 archive-v8 fixtures.

use agentir_core::{
    Action, RevisionId, Transaction, Workspace, WorkspaceId,
    actions::{RegionArgumentSpec, RegionSpec},
    candidate::RelationKind,
    ids::{
        BufferId, CandidateId, CandidateRevisionId, ImplValueId, MemoryPlanId, MemoryRevisionId,
        ScheduleAxisId, ScheduleNodeId, SchedulePlanId, ScheduleRevisionId, TargetManifestId,
        TargetManifestRevisionId,
    },
    memory::{MemoryAction, MemoryHash, MemoryTransaction},
    schedule::{ScheduleAction, ScheduleTransaction},
    target::{TargetHash, TargetProfile},
};
use agentir_store::{WorkspaceArchiveV8, encode_workspace_archive};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

#[derive(Serialize)]
struct Body<'a> {
    format: &'a str,
    format_version: u32,
    compiler_version: &'a str,
    snapshot: &'a agentir_core::persistence::WorkspaceSnapshot,
}

fn hash_body(archive: &WorkspaceArchiveV8) -> String {
    let bytes = serde_json::to_vec(&Body {
        format: &archive.format,
        format_version: archive.format_version,
        compiler_version: &archive.compiler_version,
        snapshot: &archive.snapshot,
    })
    .unwrap();
    let digest = Sha256::digest(bytes);
    let mut output = String::new();
    for byte in digest {
        write!(output, "{byte:02x}").unwrap();
    }
    output
}

fn write_archive(directory: &Path, name: &str, workspace: &Workspace) {
    fs::write(
        directory.join(name),
        encode_workspace_archive(workspace).unwrap(),
    )
    .unwrap();
}

fn write_corrupt(
    directory: &Path,
    name: &str,
    bytes: &[u8],
    mutate: impl FnOnce(&mut WorkspaceArchiveV8),
) {
    let mut archive: WorkspaceArchiveV8 = serde_json::from_slice(bytes).unwrap();
    mutate(&mut archive);
    archive.archive_hash = hash_body(&archive);
    fs::write(directory.join(name), serde_json::to_vec(&archive).unwrap()).unwrap();
}

fn map_workspace(name: &str, operations: usize) -> Workspace {
    let id = WorkspaceId::new(name);
    let mut workspace = Workspace::new(id.clone()).unwrap();
    let mut actions = vec![
        Action::DefineDimension {
            bind: Some("$N".to_owned()),
            name: "N".to_owned(),
            constraints: vec!["N >= 0".to_owned()],
        },
        Action::CreateParameter {
            bind: "$v0".to_owned(),
            name: "x".to_owned(),
            ty: "tensor<f32,[N]>".parse().unwrap(),
        },
    ];
    for index in 0..operations {
        actions.push(Action::CreateOp {
            bind: format!("$v{}", index + 1),
            opcode: "map".to_owned(),
            operands: vec![format!("$v{index}")],
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
        value: format!("$v{operations}"),
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
    let head = workspace.head().clone();
    workspace
        .candidate_create(&head, RelationKind::EquivalentToSpec)
        .unwrap();
    workspace
}

fn schedule_root(name: &str, operations: usize) -> Workspace {
    let mut workspace = map_workspace(name, operations);
    workspace
        .memory_create(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
        .unwrap();
    workspace
        .target_create(TargetProfile::GenericGpuV1)
        .unwrap();
    workspace
        .schedule_create(
            &MemoryPlanId::new("mp1"),
            &MemoryRevisionId::new("mr1"),
            &TargetManifestId::new("tm1"),
            &TargetManifestRevisionId::new("tmr1"),
        )
        .unwrap();
    workspace
}

fn apply_schedule(workspace: &mut Workspace, action: ScheduleAction) {
    let root = workspace
        .schedule_query(&SchedulePlanId::new("sp1"), &ScheduleRevisionId::new("sr1"))
        .unwrap();
    workspace
        .schedule_apply(&ScheduleTransaction {
            schedule_plan: SchedulePlanId::new("sp1"),
            base_schedule_revision: ScheduleRevisionId::new("sr1"),
            expected_schedule_hash: root.schedule_hash,
            expected_memory_hash: root.memory_hash,
            expected_target_hash: root.target_hash,
            actions: vec![action],
        })
        .unwrap();
}

fn main() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    fs::create_dir_all(&directory).unwrap();
    let minimal = Workspace::new(WorkspaceId::new("minimal-v8")).unwrap();
    write_archive(&directory, "minimal-v8.json", &minimal);

    let mut target_only = map_workspace("target-v8", 1);
    target_only
        .target_create(TargetProfile::GenericGpuV1)
        .unwrap();
    write_archive(&directory, "target-generic-v8.json", &target_only);

    let serial = schedule_root("schedule-v8", 2);
    let serial_bytes = encode_workspace_archive(&serial).unwrap();
    fs::write(directory.join("schedule-serial-v8.json"), &serial_bytes).unwrap();

    let mut split = serial.clone();
    apply_schedule(
        &mut split,
        ScheduleAction::SplitAxis {
            axis: ScheduleAxisId::new("sa1"),
            factor: 4,
        },
    );
    write_archive(&directory, "schedule-split-v8.json", &split);
    write_archive(&directory, "schedule-remainder-v8.json", &split);

    let mut tiled = serial.clone();
    apply_schedule(
        &mut tiled,
        ScheduleAction::TileAxes {
            axes: vec![ScheduleAxisId::new("sa1")],
            tile_sizes: vec![4],
        },
    );
    write_archive(&directory, "schedule-tiled-v8.json", &tiled);

    let mut fused = serial.clone();
    apply_schedule(
        &mut fused,
        ScheduleAction::FuseOperations {
            producer: ScheduleNodeId::new("sn1"),
            consumer: ScheduleNodeId::new("sn2"),
        },
    );
    write_archive(&directory, "schedule-fused-v8.json", &fused);

    let mut forked = serial.clone();
    let root = forked
        .schedule_query(&SchedulePlanId::new("sp1"), &ScheduleRevisionId::new("sr1"))
        .unwrap();
    forked
        .schedule_fork(
            &SchedulePlanId::new("sp1"),
            &ScheduleRevisionId::new("sr1"),
            &root.schedule_hash,
        )
        .unwrap();
    write_archive(&directory, "schedule-forked-v8.json", &forked);

    let mut sealed = serial.clone();
    let root = sealed
        .schedule_query(&SchedulePlanId::new("sp1"), &ScheduleRevisionId::new("sr1"))
        .unwrap();
    sealed
        .schedule_seal(
            &SchedulePlanId::new("sp1"),
            &ScheduleRevisionId::new("sr1"),
            &root.schedule_hash,
        )
        .unwrap();
    write_archive(&directory, "schedule-sealed-v8.json", &sealed);

    let mut guarded = map_workspace("guarded-v8", 2);
    let memory = guarded
        .memory_create(&CandidateId::new("c1"), &CandidateRevisionId::new("cr1"))
        .unwrap();
    guarded
        .memory_apply(&MemoryTransaction {
            memory_plan: MemoryPlanId::new("mp1"),
            base_memory_revision: MemoryRevisionId::new("mr1"),
            expected_memory_hash: memory.query.memory_hash,
            expected_impl_hash: memory.query.impl_hash,
            actions: vec![MemoryAction::RequestGuardedReuse {
                input: ImplValueId::new("iv2"),
                result: ImplValueId::new("iv3"),
                guard_against: BufferId::new("buf1"),
            }],
        })
        .unwrap();
    guarded.target_create(TargetProfile::GenericGpuV1).unwrap();
    guarded
        .schedule_create(
            &MemoryPlanId::new("mp1"),
            &MemoryRevisionId::new("mr2"),
            &TargetManifestId::new("tm1"),
            &TargetManifestRevisionId::new("tmr1"),
        )
        .unwrap();
    write_archive(&directory, "schedule-guarded-v8.json", &guarded);

    write_corrupt(
        &directory,
        "corrupted-target-hash-v8.json",
        &serial_bytes,
        |archive| {
            archive
                .snapshot
                .target_store
                .manifests
                .get_mut(&TargetManifestId::new("tm1"))
                .unwrap()
                .manifest
                .target_hash = TargetHash::new("corrupt");
        },
    );
    write_corrupt(
        &directory,
        "corrupted-target-capability-v8.json",
        &serial_bytes,
        |archive| {
            archive
                .snapshot
                .target_store
                .manifests
                .get_mut(&TargetManifestId::new("tm1"))
                .unwrap()
                .manifest
                .capabilities
                .pop();
        },
    );
    write_corrupt(
        &directory,
        "corrupted-schedule-hash-v8.json",
        &serial_bytes,
        |archive| {
            archive
                .snapshot
                .schedule_store
                .plans
                .get_mut(&SchedulePlanId::new("sp1"))
                .unwrap()
                .revisions
                .get_mut(&ScheduleRevisionId::new("sr1"))
                .unwrap()
                .schedule_hash = agentir_core::schedule::ScheduleHash::new("corrupt");
        },
    );
    write_corrupt(
        &directory,
        "corrupted-schedule-coverage-v8.json",
        &serial_bytes,
        |archive| {
            archive
                .snapshot
                .schedule_store
                .plans
                .get_mut(&SchedulePlanId::new("sp1"))
                .unwrap()
                .revisions
                .get_mut(&ScheduleRevisionId::new("sr1"))
                .unwrap()
                .program
                .node_order
                .pop();
        },
    );
    write_corrupt(
        &directory,
        "corrupted-schedule-dependency-v8.json",
        &serial_bytes,
        |archive| {
            archive
                .snapshot
                .schedule_store
                .plans
                .get_mut(&SchedulePlanId::new("sp1"))
                .unwrap()
                .revisions
                .get_mut(&ScheduleRevisionId::new("sr1"))
                .unwrap()
                .program
                .dependencies
                .clear();
        },
    );
    write_corrupt(
        &directory,
        "corrupted-schedule-resource-v8.json",
        &serial_bytes,
        |archive| {
            archive
                .snapshot
                .schedule_store
                .plans
                .get_mut(&SchedulePlanId::new("sp1"))
                .unwrap()
                .revisions
                .get_mut(&ScheduleRevisionId::new("sr1"))
                .unwrap()
                .program
                .resource_estimate
                .threads_per_workgroup = 999;
        },
    );
    write_corrupt(
        &directory,
        "corrupted-schedule-memory-anchor-v8.json",
        &serial_bytes,
        |archive| {
            archive
                .snapshot
                .schedule_store
                .plans
                .get_mut(&SchedulePlanId::new("sp1"))
                .unwrap()
                .revisions
                .get_mut(&ScheduleRevisionId::new("sr1"))
                .unwrap()
                .memory_hash = MemoryHash::new("corrupt");
        },
    );
    write_corrupt(
        &directory,
        "corrupted-schedule-certificate-v8.json",
        &serial_bytes,
        |archive| {
            let method = &mut archive
                .snapshot
                .schedule_store
                .plans
                .get_mut(&SchedulePlanId::new("sp1"))
                .unwrap()
                .revisions
                .get_mut(&ScheduleRevisionId::new("sr1"))
                .unwrap()
                .certificates[0]
                .method;
            "corrupt".clone_into(method);
        },
    );
    fs::write(
        directory.join("future-v9.json"),
        serde_json::to_vec(&json!({"format":"agentir-workspace","format_version":9})).unwrap(),
    )
    .unwrap();
}
