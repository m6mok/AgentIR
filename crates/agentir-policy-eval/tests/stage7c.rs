use agentir_core::{
    backend_ir::{ArtifactStatus, HardwareBenchmarkConfig},
    ids::ArtifactId,
};
use agentir_policy_eval::{
    EvaluationErrorCode, EvaluationTaskId, MeasurementAcquisitionArtifact,
    MeasurementAcquisitionCatalog, MeasurementAcquisitionFailurePolicy,
    MeasurementAcquisitionOrderingPolicy, MeasurementAcquisitionPlan,
    MeasurementAcquisitionPlanRequest, MeasurementValidationPolicy,
};

fn artifact(hash: &str) -> MeasurementAcquisitionArtifact {
    MeasurementAcquisitionArtifact {
        artifact_id: ArtifactId::new(format!("artifact-{hash}")),
        artifact_hash: hash.to_owned(),
        spec_hash: "spec-1".to_owned(),
        target_hash: "target-1".to_owned(),
        compiler_build_hash: "build-1".to_owned(),
        status: ArtifactStatus::Validated,
        offline_valid: true,
    }
}

fn request(hashes: &[&str], records: u64) -> MeasurementAcquisitionPlanRequest {
    MeasurementAcquisitionPlanRequest {
        corpus_hash: "corpus-1".to_owned(),
        task_id: EvaluationTaskId("task-1".to_owned()),
        root_anchor_hash: "root-1".to_owned(),
        artifact_hashes: hashes.iter().map(|hash| (*hash).to_owned()).collect(),
        benchmark_config: HardwareBenchmarkConfig {
            warmups: 1,
            iterations: 3,
            input_distribution: "deterministic_zero_v1".to_owned(),
            tensor_dimensions: vec![4],
        },
        records_per_artifact: records,
        validation_policy: MeasurementValidationPolicy::SyntheticFixtureV1,
        ordering_policy: MeasurementAcquisitionOrderingPolicy::RoundRobinArtifactHashV1,
        failure_policy: MeasurementAcquisitionFailurePolicy::StopOnFirstFailureV1,
        checkpoint_cadence_slots: 1,
    }
}

fn catalog() -> MeasurementAcquisitionCatalog {
    MeasurementAcquisitionCatalog::synthetic_fixture(
        "workspace-1".to_owned(),
        "root-1".to_owned(),
        vec![artifact("c"), artifact("a"), artifact("b")],
    )
    .unwrap()
}

#[test]
fn plan_is_canonical_and_round_robin() {
    let catalog = catalog();
    let left = MeasurementAcquisitionPlan::new(&catalog, request(&["c", "a", "b"], 3)).unwrap();
    let right = MeasurementAcquisitionPlan::new(&catalog, request(&["b", "c", "a"], 3)).unwrap();
    assert_eq!(left, right);
    assert_eq!(left.artifact_hashes, ["a", "b", "c"]);
    assert_eq!(
        left.slots()
            .unwrap()
            .into_iter()
            .map(|slot| (slot.slot_index, slot.round_index, slot.artifact_hash))
            .collect::<Vec<_>>(),
        vec![
            (0, 0, "a".to_owned()),
            (1, 0, "b".to_owned()),
            (2, 0, "c".to_owned()),
            (3, 1, "a".to_owned()),
            (4, 1, "b".to_owned()),
            (5, 1, "c".to_owned()),
            (6, 2, "a".to_owned()),
            (7, 2, "b".to_owned()),
            (8, 2, "c".to_owned()),
        ]
    );
    left.verify().unwrap();
}

#[test]
fn plan_boundaries_and_mixed_anchors_reject() {
    let catalog = catalog();
    assert_eq!(
        MeasurementAcquisitionPlan::new(&catalog, request(&[], 1))
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationAcquisitionPlanInvalid
    );
    let mut mixed = catalog.clone();
    mixed.artifacts.get_mut("b").unwrap().spec_hash = "spec-2".to_owned();
    assert_eq!(
        MeasurementAcquisitionPlan::new(&mixed, request(&["a", "b"], 1))
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationAcquisitionMixedSpec
    );
    let mut hardware = request(&["a"], 1);
    hardware.validation_policy = MeasurementValidationPolicy::HardwareExecutedV1;
    assert_eq!(
        MeasurementAcquisitionPlan::new(&catalog, hardware)
            .unwrap_err()
            .code,
        EvaluationErrorCode::EvaluationAcquisitionUnsupportedMode
    );
}

#[test]
fn plan_hash_is_stable_and_domain_separated() {
    let plan = MeasurementAcquisitionPlan::new(&catalog(), request(&["a", "b", "c"], 3)).unwrap();
    assert_eq!(
        plan.measurement_acquisition_plan_hash,
        "8383cffe6fc500bfc27ea599cfcb3f8903bfa6af62b6518240d4ba16415b7cda"
    );
    assert_ne!(
        agentir_policy_eval::MEASUREMENT_ACQUISITION_PLAN_HASH_DOMAIN,
        agentir_policy_eval::MEASUREMENT_COHORT_HASH_DOMAIN
    );
}
