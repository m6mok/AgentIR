use agentir_core::{
    cpu::CpuArtifactHash,
    cpu_measurement::{
        CpuBenchmarkConfig, CpuClockSource, CpuMeasurementHash, aggregate_cpu_durations,
        cpu_measurement_hash, verify_cpu_measurement,
    },
    ids::{CpuArtifactId, CpuMeasurementId},
    resources::ResourceLimits,
};
use agentir_protocol::Engine;
use agentir_runtime_cpu::{CpuClock, acquire_with_clock};
use agentir_store::{
    MIGRATION_V10_TO_V11, WorkspaceArchiveV9, WorkspaceArchiveV11, load_workspace_bytes,
    migrate_archive_v9_to_v10,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const ARTIFACT_ID: &str = "cpuart-c6eb17c4671f1cb8";
const ARTIFACT_HASH: &str = "c6eb17c4671f1cb8988e92b275357d80a921da61d423bc12211117fef7ea9025";

fn temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("agentir-stage8b-{label}-{nonce}-{sequence}.json"))
}

fn engine_with_saxpy() -> Engine {
    let mut engine = Engine::new();
    for line in include_str!("../../../examples/cpu_saxpy.jsonl").lines() {
        let response: Value = serde_json::from_str(&engine.process_line(line)).unwrap();
        assert_eq!(response["ok"], true, "{response}");
    }
    engine
}

fn save_engine(engine: &mut Engine, label: &str) -> Vec<u8> {
    let path = temp_path(label);
    let response: Value = serde_json::from_str(
        &engine.process_line(
            &json!({
                "command":"workspace.save", "request_id":"save", "workspace":"w1", "path":path
            })
            .to_string(),
        ),
    )
    .unwrap();
    assert_eq!(response["ok"], true, "{response}");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    bytes
}

#[derive(Debug)]
struct SyntheticClock {
    readings: VecDeque<u64>,
    calls: usize,
}

impl SyntheticClock {
    fn new(readings: impl IntoIterator<Item = u64>) -> Self {
        Self {
            readings: readings.into_iter().collect(),
            calls: 0,
        }
    }
}

impl CpuClock for SyntheticClock {
    fn source(&self) -> CpuClockSource {
        CpuClockSource::SyntheticTestFixtureV1
    }
    fn now_ns(&mut self) -> agentir_core::AgentResult<u64> {
        self.calls += 1;
        self.readings.pop_front().ok_or_else(|| {
            agentir_core::AgentError::new(
                agentir_core::ErrorCode::CpuMeasurementOverflow,
                "synthetic test clock exhausted",
            )
        })
    }
}

fn saxpy_workspace() -> agentir_core::Workspace {
    let mut engine = engine_with_saxpy();
    let bytes = save_engine(&mut engine, "fixture");
    load_workspace_bytes(&bytes).unwrap().workspace
}

fn synthetic_workspace() -> (agentir_core::Workspace, SyntheticClock) {
    let mut workspace = saxpy_workspace();
    let package = workspace
        .cpu_artifact_package(&CpuArtifactId::new(ARTIFACT_ID))
        .unwrap()
        .clone();
    let inputs =
        serde_json::from_value(json!({"a":2.0,"x":[1.0,2.0,3.0,4.0],"y":[10.0,20.0,30.0,40.0]}))
            .unwrap();
    let mut clock = SyntheticClock::new([100, 110, 200, 230, 300, 320]);
    let draft = acquire_with_clock(
        &package,
        CpuBenchmarkConfig::v1(1, 3),
        &inputs,
        &ResourceLimits::default(),
        &mut clock,
    )
    .unwrap();
    workspace.cpu_measurement_publish(draft).unwrap();
    (workspace, clock)
}

#[test]
fn projected_limit_and_clock_overflow_fail_before_publication() {
    let workspace = saxpy_workspace();
    let package = workspace
        .cpu_artifact_package(&CpuArtifactId::new(ARTIFACT_ID))
        .unwrap();
    let inputs =
        serde_json::from_value(json!({"a":2.0,"x":[1.0,2.0,3.0,4.0],"y":[10.0,20.0,30.0,40.0]}))
            .unwrap();
    let mut never_read = SyntheticClock::new([]);
    let limits = ResourceLimits {
        execution_elements: 1,
        ..ResourceLimits::default()
    };
    assert!(
        acquire_with_clock(
            package,
            CpuBenchmarkConfig::v1(0, 1),
            &inputs,
            &limits,
            &mut never_read,
        )
        .is_err()
    );
    assert_eq!(never_read.calls, 0);
    let mut regressing = SyntheticClock::new([10, 5]);
    assert!(
        acquire_with_clock(
            package,
            CpuBenchmarkConfig::v1(0, 1),
            &inputs,
            &ResourceLimits::default(),
            &mut regressing,
        )
        .is_err()
    );
    assert!(workspace.cpu_measurement_store().records.is_empty());
    assert!(workspace.cpu_measurement_store().events.is_empty());
}

#[test]
fn synthetic_acquisition_has_exact_integer_aggregates_stable_hashes_and_outputs() {
    let (mut workspace, clock) = synthetic_workspace();
    assert_eq!(clock.calls, 6);
    let record = workspace
        .cpu_measurement_query(&CpuMeasurementId::new("cpum1"))
        .unwrap();
    assert_eq!(record.raw_duration_ns, [10, 30, 20]);
    assert_eq!(
        record.aggregates,
        aggregate_cpu_durations(&[10, 30, 20]).unwrap()
    );
    assert_eq!(record.aggregates.median_ns, 20);
    assert_eq!(record.aggregates.p95_ns, 30);
    assert_eq!(record.outputs["out"], json!([12.0, 24.0, 36.0, 48.0]));
    let hash = cpu_measurement_hash(record).unwrap();
    assert_eq!(hash, record.cpu_measurement_hash);
    let mut different_id = record.clone();
    different_id.id = CpuMeasurementId::new("cpum999");
    assert_eq!(cpu_measurement_hash(&different_id).unwrap(), hash);
    let mut changed_sample = record.clone();
    changed_sample.raw_duration_ns[0] += 1;
    assert_ne!(cpu_measurement_hash(&changed_sample).unwrap(), hash);
    assert!(
        verify_cpu_measurement(
            &changed_sample,
            workspace
                .cpu_artifact_package(&CpuArtifactId::new(ARTIFACT_ID))
                .unwrap()
        )
        .is_err()
    );
    let retained_hash = record.cpu_measurement_hash.clone();
    workspace.set_resource_limits(ResourceLimits {
        benchmark_warmups: 99,
        benchmark_iterations: 99,
        execution_elements: 99,
        ..ResourceLimits::default()
    });
    assert_eq!(
        workspace
            .cpu_measurement_query(&CpuMeasurementId::new("cpum1"))
            .unwrap()
            .cpu_measurement_hash,
        retained_hash
    );
}

#[test]
fn rejected_requests_cannot_supply_runtime_owned_fields_or_consume_ids() {
    let mut engine = engine_with_saxpy();
    for (label, extra) in [
        ("samples", json!({"raw_duration_ns":[1]})),
        (
            "aggregates",
            json!({"aggregates":{"min_ns":1,"median_ns":1,"p95_ns":1,"max_ns":1}}),
        ),
        ("host", json!({"host":{"architecture":"spoofed"}})),
        ("output", json!({"output_hash":"spoofed"})),
        ("hash", json!({"cpu_measurement_hash":"spoofed"})),
    ] {
        let mut request = json!({
            "command":"cpu_measurement.acquire", "request_id":label, "workspace":"w1",
            "cpu_artifact":ARTIFACT_ID, "expected_cpu_artifact_hash":ARTIFACT_HASH,
            "config":{"format_version":1,"warmups":0,"iterations":1,"aggregation":"ordered_integer_ns_v1"},
            "inputs":{"a":2.0,"x":[1.0],"y":[10.0]}
        });
        request
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let response: Value =
            serde_json::from_str(&engine.process_line(&request.to_string())).unwrap();
        assert_eq!(response["ok"], false);
    }
    for request in [
        json!({"command":"cpu_measurement.acquire","request_id":"zero","workspace":"w1","cpu_artifact":ARTIFACT_ID,"expected_cpu_artifact_hash":ARTIFACT_HASH,"config":{"format_version":1,"warmups":0,"iterations":0,"aggregation":"ordered_integer_ns_v1"},"inputs":{"a":2.0,"x":[1.0],"y":[10.0]}}),
        json!({"command":"cpu_measurement.acquire","request_id":"bad-hash","workspace":"w1","cpu_artifact":ARTIFACT_ID,"expected_cpu_artifact_hash":"bad","config":{"format_version":1,"warmups":0,"iterations":1,"aggregation":"ordered_integer_ns_v1"},"inputs":{"a":2.0,"x":[1.0],"y":[10.0]}}),
        json!({"command":"cpu_measurement.acquire","request_id":"bad-input","workspace":"w1","cpu_artifact":ARTIFACT_ID,"expected_cpu_artifact_hash":ARTIFACT_HASH,"config":{"format_version":1,"warmups":0,"iterations":1,"aggregation":"ordered_integer_ns_v1"},"inputs":{"a":2.0,"x":[1.0,2.0],"y":[10.0]}}),
    ] {
        let response: Value =
            serde_json::from_str(&engine.process_line(&request.to_string())).unwrap();
        assert_eq!(response["ok"], false, "{response}");
    }
    let list: Value = serde_json::from_str(&engine.process_line(
        r#"{"command":"cpu_measurement.list","request_id":"list","workspace":"w1"}"#,
    ))
    .unwrap();
    assert_eq!(list["result"], json!([]));
}

#[derive(Serialize)]
struct Body<'a> {
    format: &'a str,
    format_version: u32,
    compiler_version: &'a str,
    snapshot: &'a agentir_core::persistence::WorkspaceSnapshot,
}

fn rehash(archive: &mut WorkspaceArchiveV11) -> Vec<u8> {
    let bytes = serde_json::to_vec(&Body {
        format: &archive.format,
        format_version: archive.format_version,
        compiler_version: &archive.compiler_version,
        snapshot: &archive.snapshot,
    })
    .unwrap();
    archive.archive_hash = format!("{:x}", Sha256::digest(bytes));
    serde_json::to_vec(archive).unwrap()
}

#[test]
fn archive_v11_round_trip_is_exact_and_corruption_is_rejected_without_execution() {
    let (workspace, clock) = synthetic_workspace();
    let bytes = agentir_store::encode_workspace_archive(&workspace).unwrap();
    let loaded = load_workspace_bytes(&bytes).unwrap();
    assert_eq!(clock.calls, 6);
    assert_eq!(loaded.metadata.format_version, 11);
    assert_eq!(loaded.replay.cpu_measurements_verified, 1);
    assert_eq!(loaded.replay.cpu_measurement_events_replayed, 1);
    assert_eq!(
        loaded
            .workspace
            .cpu_measurement_query(&CpuMeasurementId::new("cpum1"))
            .unwrap(),
        workspace
            .cpu_measurement_query(&CpuMeasurementId::new("cpum1"))
            .unwrap()
    );

    for mutation in [
        "sample",
        "aggregate",
        "anchor",
        "output",
        "hash",
        "cursor",
        "order",
    ] {
        let mut archive: WorkspaceArchiveV11 = serde_json::from_slice(&bytes).unwrap();
        match mutation {
            "sample" => {
                archive
                    .snapshot
                    .cpu_measurement_store
                    .records
                    .get_mut(&CpuMeasurementId::new("cpum1"))
                    .unwrap()
                    .raw_duration_ns[0] += 1;
                archive.snapshot.cpu_measurement_store.events[0]
                    .event
                    .record
                    .raw_duration_ns[0] += 1;
            }
            "aggregate" => {
                archive
                    .snapshot
                    .cpu_measurement_store
                    .records
                    .get_mut(&CpuMeasurementId::new("cpum1"))
                    .unwrap()
                    .aggregates
                    .median_ns += 1;
                archive.snapshot.cpu_measurement_store.events[0]
                    .event
                    .record
                    .aggregates
                    .median_ns += 1;
            }
            "anchor" => {
                archive
                    .snapshot
                    .cpu_measurement_store
                    .records
                    .get_mut(&CpuMeasurementId::new("cpum1"))
                    .unwrap()
                    .cpu_artifact_hash = CpuArtifactHash::new("bad");
                archive.snapshot.cpu_measurement_store.events[0]
                    .event
                    .record
                    .cpu_artifact_hash = CpuArtifactHash::new("bad");
            }
            "output" => {
                archive
                    .snapshot
                    .cpu_measurement_store
                    .records
                    .get_mut(&CpuMeasurementId::new("cpum1"))
                    .unwrap()
                    .outputs
                    .insert("out".to_owned(), json!([0.0]));
                archive.snapshot.cpu_measurement_store.events[0]
                    .event
                    .record
                    .outputs
                    .insert("out".to_owned(), json!([0.0]));
            }
            "hash" => {
                archive
                    .snapshot
                    .cpu_measurement_store
                    .records
                    .get_mut(&CpuMeasurementId::new("cpum1"))
                    .unwrap()
                    .cpu_measurement_hash = CpuMeasurementHash::new("bad");
                archive.snapshot.cpu_measurement_store.events[0]
                    .event
                    .record
                    .cpu_measurement_hash = CpuMeasurementHash::new("bad");
            }
            "cursor" => {
                archive.snapshot.cpu_measurement_store.events[0]
                    .event
                    .cpu_artifact_event_cursor = 0;
            }
            "order" => archive.snapshot.cpu_measurement_store.next_id = 2,
            _ => unreachable!(),
        }
        assert!(
            load_workspace_bytes(&rehash(&mut archive)).is_err(),
            "{mutation}"
        );
    }
}

#[test]
fn v10_to_v11_adds_an_empty_store_without_invented_history() {
    let v9: WorkspaceArchiveV9 = serde_json::from_slice(include_bytes!(
        "../../agentir-store/tests/fixtures/minimal-v9.json"
    ))
    .unwrap();
    let v10 = migrate_archive_v9_to_v10(v9).unwrap();
    assert_eq!(v10.snapshot.schema_version, 10);
    let loaded = load_workspace_bytes(&serde_json::to_vec(&v10).unwrap()).unwrap();
    assert_eq!(loaded.metadata.format_version, 10);
    assert_eq!(loaded.migration.applied_steps, [MIGRATION_V10_TO_V11]);
    assert!(loaded.workspace.cpu_measurement_store().records.is_empty());
    assert!(loaded.workspace.cpu_measurement_store().events.is_empty());
}
