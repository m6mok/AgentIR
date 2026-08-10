//! Regenerates deterministic Stage 5 archive-v9 fixtures.

use agentir_core::{
    backend::{MeasurementHash, device_fingerprint_hash},
    backend_ir::{
        DeviceFingerprint, HardwareBenchmarkConfig, HardwareMeasurementRecord,
        MEASUREMENT_FORMAT_VERSION,
    },
};
use agentir_protocol::Engine;
use agentir_store::{WorkspaceArchiveV9, load_workspace, save_workspace};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt::Write as _, fs, path::Path};

#[derive(Serialize)]
struct ArchiveBody<'a> {
    format: &'a str,
    format_version: u32,
    compiler_version: &'a str,
    snapshot: &'a agentir_core::persistence::LegacyWorkspaceSnapshotV9,
}

fn body_hash(archive: &WorkspaceArchiveV9) -> String {
    let bytes = serde_json::to_vec(&ArchiveBody {
        format: &archive.format,
        format_version: archive.format_version,
        compiler_version: &archive.compiler_version,
        snapshot: &archive.snapshot,
    })
    .expect("archive body serialization");
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String");
            output
        })
}

fn normalize(path: &Path) {
    let mut archive: WorkspaceArchiveV9 =
        serde_json::from_slice(&fs::read(path).expect("fixture read")).expect("v9 archive");
    for revision in archive.snapshot.revisions.values_mut() {
        revision.created_at_unix_ms = 0;
    }
    archive.archive_hash = body_hash(&archive);
    let mut bytes = serde_json::to_vec(&archive).expect("fixture serialization");
    bytes.push(b'\n');
    fs::write(path, bytes).expect("fixture write");
}

fn run_example(source: &str, destination: &Path) {
    let mut engine = Engine::new();
    for line in source.lines().filter(|line| !line.is_empty()) {
        let response = engine.process_line(line);
        let value: Value = serde_json::from_str(&response).expect("protocol response");
        assert_eq!(value["ok"], true, "{response}");
    }
    if destination.exists() {
        fs::remove_file(destination).expect("replace generated fixture");
    }
    let response = engine.process_line(
        &json!({
            "command": "workspace.save",
            "request_id": "fixture-save",
            "workspace": "w1",
            "path": destination,
        })
        .to_string(),
    );
    assert_eq!(
        serde_json::from_str::<Value>(&response).expect("save response")["ok"],
        true,
        "{response}"
    );
    normalize(destination);
}

fn copy(source: &Path, directory: &Path, names: &[&str]) {
    let bytes = fs::read(source).expect("source fixture");
    for name in names {
        fs::write(directory.join(name), &bytes).expect("fixture copy");
    }
}

fn corrupt(source: &Path, destination: &Path, mutate: impl FnOnce(&mut Value)) {
    let mut value: Value = serde_json::from_slice(&fs::read(source).expect("corruption source"))
        .expect("archive JSON");
    mutate(&mut value);
    let mut archive: WorkspaceArchiveV9 =
        serde_json::from_value(value).expect("corruption must retain the v9 wire shape");
    archive.archive_hash = body_hash(&archive);
    let mut bytes = serde_json::to_vec(&archive).expect("corrupted fixture serialization");
    bytes.push(b'\n');
    fs::write(destination, bytes).expect("corrupted fixture write");
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixtures = root.join("crates/agentir-store/tests/fixtures");
    fs::create_dir_all(&fixtures).expect("fixture directory");

    run_example(
        r#"{"command":"workspace.open","request_id":"open","workspace":"w1"}"#,
        &fixtures.join("minimal-v9.json"),
    );

    let migrated = load_workspace(fixtures.join("minimal-v8.json")).expect("v8 migration");
    let migrated_path = fixtures.join("migrated-v8-v9.json");
    if migrated_path.exists() {
        fs::remove_file(&migrated_path).expect("replace migrated fixture");
    }
    save_workspace(&migrated_path, &migrated.workspace).expect("migrated fixture save");
    normalize(&migrated_path);

    let workflows = [
        (
            "backend_serial.jsonl",
            "backend-serial-v9.json",
            &["target-webgpu-wgsl-v9.json"][..],
        ),
        ("backend_tiled.jsonl", "backend-tiled-v9.json", &[][..]),
        (
            "backend_remainder.jsonl",
            "backend-remainder-v9.json",
            &[][..],
        ),
        ("backend_fused.jsonl", "backend-fused-v9.json", &[][..]),
        (
            "backend_vectorized.jsonl",
            "backend-vectorized-v9.json",
            &[][..],
        ),
        (
            "backend_guarded_memory.jsonl",
            "backend-guarded-v9.json",
            &[][..],
        ),
        (
            "backend_reuse.jsonl",
            "backend-reuse-v9.json",
            &["artifact-multi-dispatch-v9.json"][..],
        ),
        (
            "backend_saxpy_wgsl.jsonl",
            "artifact-saxpy-v9.json",
            &["artifact-sealed-v9.json"][..],
        ),
        (
            "equality_to_artifact.jsonl",
            "equality-materialized-artifact-v9.json",
            &[][..],
        ),
    ];
    for (example, fixture, aliases) in workflows {
        let source = fs::read_to_string(root.join("examples").join(example)).expect("example");
        let destination = fixtures.join(fixture);
        run_example(&source, &destination);
        copy(&destination, &fixtures, aliases);
    }

    let mut measured = load_workspace(fixtures.join("artifact-saxpy-v9.json"))
        .expect("artifact fixture")
        .workspace;
    let package = measured
        .artifact_package(&agentir_core::ids::ArtifactId::new("art1"))
        .expect("artifact")
        .clone();
    let device = DeviceFingerprint {
        backend_api: "fixture".to_owned(),
        adapter_name: "deterministic-fixture-device".to_owned(),
        vendor_id: Some(0),
        device_id: Some(0),
        driver_info: Some("not-executed".to_owned()),
        limits: BTreeMap::new(),
        runtime_version: "fixture-runtime-v1".to_owned(),
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    let record = HardwareMeasurementRecord {
        format_version: MEASUREMENT_FORMAT_VERSION,
        artifact_hash: package.artifact_hash,
        target_hash: package.manifest.anchor.target_hash,
        compiler_build_hash: package.manifest.compiler_build_hash,
        device_fingerprint_hash: device_fingerprint_hash(&device).expect("fingerprint hash"),
        device,
        config: HardwareBenchmarkConfig {
            warmups: 1,
            iterations: 3,
            input_distribution: "deterministic_fixture_only".to_owned(),
            tensor_dimensions: vec![4],
        },
        min_ns: 10,
        median_ns: 11,
        p95_ns: 12,
        max_ns: 12,
        guard_outcomes: BTreeMap::from([("unguarded".to_owned(), 3)]),
        validation_status: "fixture_record_not_hardware_executed".to_owned(),
        runtime_version: "fixture-runtime-v1".to_owned(),
        measurement_hash: MeasurementHash::new("pending"),
    };
    measured
        .measurement_publish(record)
        .expect("measurement fixture publication");
    let measurement_path = fixtures.join("measurement-record-v9.json");
    if measurement_path.exists() {
        fs::remove_file(&measurement_path).expect("replace measurement fixture");
    }
    save_workspace(&measurement_path, &measured).expect("measurement fixture save");
    normalize(&measurement_path);

    let mut future: Value = serde_json::from_slice(
        &fs::read(fixtures.join("minimal-v9.json")).expect("minimal fixture"),
    )
    .expect("minimal JSON");
    future["format_version"] = json!(10);
    fs::write(
        fixtures.join("future-v10.json"),
        serde_json::to_vec(&future).expect("future fixture"),
    )
    .expect("future fixture write");

    let saxpy = fixtures.join("artifact-saxpy-v9.json");
    let remainder = fixtures.join("backend-remainder-v9.json");
    let vector = fixtures.join("backend-vectorized-v9.json");
    let measurement = fixtures.join("measurement-record-v9.json");
    for (name, pointer, replacement) in [
        (
            "corrupted-backend-anchor-v9.json",
            "/snapshot/backend_store/plans/bp1/anchor/spec_hash",
            json!("00"),
        ),
        (
            "corrupted-backend-hash-v9.json",
            "/snapshot/backend_store/plans/bp1/revisions/br1/backend_hash",
            json!("00"),
        ),
        (
            "corrupted-kernel-coverage-v9.json",
            "/snapshot/backend_store/plans/bp1/revisions/br1/program/kernels/bk1/source_schedule_nodes",
            json!([]),
        ),
        (
            "corrupted-binding-layout-v9.json",
            "/snapshot/backend_store/plans/bp1/revisions/br1/program/kernels/bk1/bindings/0/binding",
            json!(99),
        ),
        (
            "corrupted-dispatch-dimensions-v9.json",
            "/snapshot/backend_store/plans/bp1/revisions/br1/program/kernels/bk1/workgroup_size/0",
            json!(0),
        ),
        (
            "corrupted-memory-mapping-v9.json",
            "/snapshot/backend_store/plans/bp1/revisions/br1/program/kernels/bk1/bindings/0/buffer",
            json!("buf999"),
        ),
        (
            "corrupted-backend-certificate-v9.json",
            "/snapshot/backend_store/plans/bp1/revisions/br1/certificate/relation",
            json!("agent_supplied"),
        ),
        (
            "corrupted-artifact-hash-v9.json",
            "/snapshot/artifact_store/packages/art1/artifact_hash",
            json!("00"),
        ),
        (
            "corrupted-wgsl-bytes-v9.json",
            "/snapshot/artifact_store/packages/art1/modules/0/wgsl",
            json!("@compute fn corrupted() {}\n"),
        ),
        (
            "corrupted-manifest-entry-point-v9.json",
            "/snapshot/artifact_store/packages/art1/manifest/entry_points/0/name",
            json!("missing_entry"),
        ),
        (
            "corrupted-compiler-build-hash-v9.json",
            "/snapshot/artifact_store/packages/art1/manifest/compiler_build_hash",
            json!("00"),
        ),
        (
            "corrupted-artifact-certificate-v9.json",
            "/snapshot/artifact_store/packages/art1/certificate/relation",
            json!("agent_supplied"),
        ),
        (
            "corrupted-event-cursor-order-v9.json",
            "/snapshot/backend_store/events/0/event/schedule_event_cursor",
            json!(999),
        ),
        (
            "corrupted-allocator-state-v9.json",
            "/snapshot/backend_store/allocator/artifact",
            json!(0),
        ),
    ] {
        corrupt(&saxpy, &fixtures.join(name), |archive| {
            *archive.pointer_mut(pointer).expect("corruption pointer") = replacement;
        });
    }
    corrupt(
        &remainder,
        &fixtures.join("corrupted-remainder-predicate-v9.json"),
        |archive| {
            *archive
                .pointer_mut("/snapshot/backend_store/plans/bp1/revisions/br1/program/dispatches/0/bounds_checked")
                .expect("remainder predicate") = json!(true);
        },
    );
    corrupt(
        &vector,
        &fixtures.join("corrupted-vector-width-v9.json"),
        |archive| {
            *archive
                .pointer_mut("/snapshot/backend_store/plans/bp1/revisions/br1/program/kernels/bk1/vector_width")
                .expect("vector width") = json!(8);
        },
    );
    for (name, pointer) in [
        (
            "corrupted-measurement-artifact-anchor-v9.json",
            "/snapshot/measurement_store/records/meas1/artifact_hash",
        ),
        (
            "corrupted-measurement-fingerprint-hash-v9.json",
            "/snapshot/measurement_store/records/meas1/device_fingerprint_hash",
        ),
    ] {
        corrupt(&measurement, &fixtures.join(name), |archive| {
            *archive.pointer_mut(pointer).expect("measurement pointer") = json!("00");
        });
    }
}
