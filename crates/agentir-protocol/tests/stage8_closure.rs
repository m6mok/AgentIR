use agentir_core::{
    Workspace,
    cpu::{CpuArtifactPackage, canonical_cpu_artifact_bytes},
    cpu_measurement::{
        CpuBenchmarkConfig, CpuClockSource, cpu_benchmark_config_hash, cpu_host_fingerprint_hash,
        cpu_input_hash, cpu_measurement_hash, cpu_output_hash,
    },
    ids::CpuMeasurementId,
    resources::ResourceLimits,
};
use agentir_protocol::Engine;
use agentir_runtime_cpu::{
    CPU_MEASUREMENT_RUNTIME_VERSION, CpuClock, CpuExecutionTestDouble, acquire,
    acquire_with_test_doubles,
};
use agentir_store::{
    MIGRATION_V10_TO_V11, WorkspaceArchiveV9, WorkspaceArchiveV11, encode_workspace_archive,
    load_workspace_bytes, migrate_archive_v9_to_v10,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn temp_path(label: &str) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "agentir-stage8c-{label}-{}-{sequence}.json",
        std::process::id()
    ))
}

fn production_pipeline_workspace() -> Workspace {
    let mut engine = Engine::new();
    for line in include_str!("../../../examples/cpu_saxpy.jsonl").lines() {
        let response: Value = serde_json::from_str(&engine.process_line(line)).unwrap();
        assert_eq!(response["ok"], true, "{response}");
    }
    let path = temp_path("pipeline");
    let save: Value = serde_json::from_str(
        &engine.process_line(
            &json!({
                "command":"workspace.save",
                "request_id":"stage8c-save",
                "workspace":"w1",
                "path":path
            })
            .to_string(),
        ),
    )
    .unwrap();
    assert_eq!(save["ok"], true, "{save}");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    load_workspace_bytes(&bytes).unwrap().workspace
}

#[derive(Debug)]
struct CountingClock {
    readings: VecDeque<u64>,
    calls: u64,
}

impl CountingClock {
    fn new(readings: impl IntoIterator<Item = u64>) -> Self {
        Self {
            readings: readings.into_iter().collect(),
            calls: 0,
        }
    }
}

impl CpuClock for CountingClock {
    fn source(&self) -> CpuClockSource {
        CpuClockSource::SyntheticTestFixtureV1
    }

    fn now_ns(&mut self) -> agentir_core::AgentResult<u64> {
        self.calls += 1;
        self.readings.pop_front().ok_or_else(|| {
            agentir_core::AgentError::new(
                agentir_core::ErrorCode::CpuMeasurementOverflow,
                "Stage 8 closure synthetic clock exhausted",
            )
        })
    }
}

#[derive(Debug, Default)]
struct CountingExecutor {
    calls: u64,
}

impl CpuExecutionTestDouble for CountingExecutor {
    fn execute(
        &mut self,
        package: &CpuArtifactPackage,
        inputs: &BTreeMap<String, Value>,
        limits: &ResourceLimits,
    ) -> agentir_core::AgentResult<agentir_backend_cpu::CpuExecutionResult> {
        self.calls += 1;
        agentir_backend_cpu::execute(package, inputs, limits)
    }
}

#[derive(Serialize)]
struct ArchiveBody<'a> {
    format: &'a str,
    format_version: u32,
    compiler_version: &'a str,
    snapshot: &'a agentir_core::persistence::WorkspaceSnapshot,
}

fn rehash(archive: &mut WorkspaceArchiveV11) -> Vec<u8> {
    let body = serde_json::to_vec(&ArchiveBody {
        format: &archive.format,
        format_version: archive.format_version,
        compiler_version: &archive.compiler_version,
        snapshot: &archive.snapshot,
    })
    .unwrap();
    archive.archive_hash = format!("{:x}", Sha256::digest(body));
    serde_json::to_vec(archive).unwrap()
}

#[test]
fn stage8_closes_through_offline_cpu_execution_measurement_and_replay() {
    let baseline = production_pipeline_workspace();
    assert!(baseline.cpu_measurement_store().records.is_empty());
    let baseline_bytes = encode_workspace_archive(&baseline).unwrap();

    let mut synthetic = load_workspace_bytes(&baseline_bytes).unwrap().workspace;
    let package = synthetic
        .cpu_artifact_store()
        .packages
        .values()
        .next()
        .unwrap()
        .clone();
    synthetic.cpu_artifact_check(&package.id).unwrap();
    let artifact_hash_before = package.cpu_artifact_hash.clone();
    let artifact_bytes_before = canonical_cpu_artifact_bytes(&package).unwrap();
    let anchor_before = package.anchor.clone();
    let inputs: BTreeMap<String, Value> = serde_json::from_value(json!({
        "a":2.0,
        "x":[1.0,2.0,3.0,4.0],
        "y":[10.0,20.0,30.0,40.0]
    }))
    .unwrap();
    let direct =
        agentir_backend_cpu::execute(&package, &inputs, &ResourceLimits::default()).unwrap();
    assert_eq!(direct.outputs["out"], json!([12.0, 24.0, 36.0, 48.0]));

    let mut rejected_clock = CountingClock::new([]);
    let mut rejected_executor = CountingExecutor::default();
    let invalid_inputs: BTreeMap<String, Value> = serde_json::from_value(json!({
        "a":2.0,
        "x":[1.0,2.0],
        "y":[10.0]
    }))
    .unwrap();
    assert!(
        acquire_with_test_doubles(
            &package,
            CpuBenchmarkConfig::v1(0, 1),
            &invalid_inputs,
            &ResourceLimits::default(),
            &mut rejected_clock,
            &mut rejected_executor,
        )
        .is_err()
    );
    assert_eq!(rejected_clock.calls, 0);
    assert_eq!(rejected_executor.calls, 0);
    assert_eq!(synthetic.cpu_measurement_store().next_id, 0);
    assert!(synthetic.cpu_measurement_store().events.is_empty());

    let config = CpuBenchmarkConfig::v1(1, 3);
    let mut clock = CountingClock::new([100, 110, 200, 230, 300, 320]);
    let mut executor = CountingExecutor::default();
    let draft = acquire_with_test_doubles(
        &package,
        config.clone(),
        &inputs,
        &ResourceLimits::default(),
        &mut clock,
        &mut executor,
    )
    .unwrap();
    assert_eq!(clock.calls, 6);
    assert_eq!(executor.calls, 4);
    let record = synthetic.cpu_measurement_publish(draft).unwrap();
    assert_eq!(record.id, CpuMeasurementId::new("cpum1"));
    assert_eq!(synthetic.cpu_measurement_store().next_id, 1);
    assert_eq!(synthetic.cpu_measurement_store().events.len(), 1);
    assert_eq!(record.outputs["out"], json!([12.0, 24.0, 36.0, 48.0]));
    assert_eq!(record.raw_duration_ns, [10, 30, 20]);
    assert_eq!(record.aggregates.min_ns, 10);
    assert_eq!(record.aggregates.median_ns, 20);
    assert_eq!(record.aggregates.p95_ns, 30);
    assert_eq!(record.aggregates.max_ns, 30);
    assert_eq!(record.cpu_artifact, package.id);
    assert_eq!(record.cpu_artifact_hash, artifact_hash_before);
    assert_eq!(record.compiler_build_hash, package.compiler_build_hash);
    assert_eq!(record.runtime_version, CPU_MEASUREMENT_RUNTIME_VERSION);
    assert_eq!(record.config, config);
    assert_eq!(
        record.cpu_benchmark_config_hash,
        cpu_benchmark_config_hash(&record.config).unwrap()
    );
    assert_eq!(
        record.cpu_input_hash,
        cpu_input_hash(&record.inputs).unwrap()
    );
    assert_eq!(
        record.cpu_host_fingerprint_hash,
        cpu_host_fingerprint_hash(&record.host).unwrap()
    );
    assert_eq!(
        record.output_hash,
        cpu_output_hash(&record.outputs).unwrap()
    );
    assert_eq!(
        record.cpu_measurement_hash,
        cpu_measurement_hash(&record).unwrap()
    );
    assert_eq!(
        record.host.clock_source,
        CpuClockSource::SyntheticTestFixtureV1
    );

    let package_after = synthetic.cpu_artifact_package(&package.id).unwrap();
    assert_eq!(package_after.anchor, anchor_before);
    assert_eq!(package_after.cpu_artifact_hash, artifact_hash_before);
    assert_eq!(
        canonical_cpu_artifact_bytes(package_after).unwrap(),
        artifact_bytes_before
    );

    let clock_calls = clock.calls;
    let execution_calls = executor.calls;
    let listed = synthetic.cpu_measurement_list();
    assert_eq!(listed.len(), 1);
    assert_eq!(&listed[0], &record);
    assert_eq!(
        synthetic.cpu_measurement_query(&record.id).unwrap(),
        &record
    );
    assert_eq!(synthetic.cpu_measurement_check(&record.id).unwrap(), record);
    assert_eq!(clock.calls, clock_calls);
    assert_eq!(executor.calls, execution_calls);

    let archive_bytes = encode_workspace_archive(&synthetic).unwrap();
    let loaded = load_workspace_bytes(&archive_bytes).unwrap();
    assert_eq!(loaded.metadata.format_version, 11);
    assert_eq!(loaded.replay.cpu_artifacts_verified, 1);
    assert_eq!(loaded.replay.cpu_measurements_verified, 1);
    assert_eq!(loaded.replay.cpu_measurement_events_replayed, 1);
    assert_eq!(
        loaded.workspace.cpu_measurement_check(&record.id).unwrap(),
        record
    );
    assert_eq!(clock.calls, clock_calls);
    assert_eq!(executor.calls, execution_calls);

    let mut corrupt_record: WorkspaceArchiveV11 = serde_json::from_slice(&archive_bytes).unwrap();
    corrupt_record
        .snapshot
        .cpu_measurement_store
        .records
        .get_mut(&record.id)
        .unwrap()
        .raw_duration_ns[0] += 1;
    corrupt_record.snapshot.cpu_measurement_store.events[0]
        .event
        .record
        .raw_duration_ns[0] += 1;
    assert!(load_workspace_bytes(&rehash(&mut corrupt_record)).is_err());

    let mut corrupt_cursor: WorkspaceArchiveV11 = serde_json::from_slice(&archive_bytes).unwrap();
    corrupt_cursor.snapshot.cpu_measurement_store.events[0]
        .event
        .cpu_artifact_event_cursor = 0;
    assert!(load_workspace_bytes(&rehash(&mut corrupt_cursor)).is_err());
    assert_eq!(clock.calls, clock_calls);
    assert_eq!(executor.calls, execution_calls);

    let mut production = load_workspace_bytes(&baseline_bytes).unwrap().workspace;
    let production_package = production
        .cpu_artifact_package(&package.id)
        .unwrap()
        .clone();
    let production_draft = acquire(
        &production_package,
        CpuBenchmarkConfig::v1(0, 1),
        &inputs,
        &ResourceLimits::default(),
    )
    .unwrap();
    let production_record = production
        .cpu_measurement_publish(production_draft)
        .unwrap();
    assert_eq!(
        production_record.host.clock_source,
        CpuClockSource::ProductionMonotonicV1
    );
    assert_eq!(
        production_record.outputs["out"],
        json!([12.0, 24.0, 36.0, 48.0])
    );
    assert_eq!(synthetic.cpu_measurement_store().records.len(), 1);
    assert_eq!(production.cpu_measurement_store().records.len(), 1);
}

#[test]
fn stage8_closure_retains_pure_v10_to_v11_migration() {
    let v9: WorkspaceArchiveV9 = serde_json::from_slice(include_bytes!(
        "../../agentir-store/tests/fixtures/minimal-v9.json"
    ))
    .unwrap();
    let v10 = migrate_archive_v9_to_v10(v9).unwrap();
    let loaded = load_workspace_bytes(&serde_json::to_vec(&v10).unwrap()).unwrap();
    assert_eq!(loaded.migration.applied_steps, [MIGRATION_V10_TO_V11]);
    assert!(loaded.workspace.cpu_measurement_store().records.is_empty());
    assert!(loaded.workspace.cpu_measurement_store().events.is_empty());
    assert_eq!(loaded.replay.cpu_measurements_verified, 0);
    assert_eq!(loaded.replay.cpu_measurement_events_replayed, 0);
}
