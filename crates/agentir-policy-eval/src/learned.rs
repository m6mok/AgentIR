//! Deterministic offline learned-ranking dataset, training, and inference.
//!
//! This module has no compiler correctness authority and performs no network,
//! provider, device, GPU, or wall-clock-dependent operation.

use crate::{
    continuation::FrameCompleteness,
    hashing::domain_hash,
    model::{
        EvaluationArchive, EvaluationDiagnostic, EvaluationErrorCode, EvaluationResult, PolicyKind,
    },
    ranking::{
        ChoiceScore, ChoiceVisibleFeatures, EvaluationChoiceId, EvaluationChoiceSet, FeatureSchema,
        FeatureSchemaVersion, FeatureType, FeatureValue, FixedScore, RankingDecision,
        RankingLimits, RankingPolicyDescriptor, RankingPolicyKind, TieBreakRule,
        feature_schema_hash, ranking_policy_hash,
    },
    work::WorkUnitCounters,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Dataset-manifest hash domain.
pub const RANKING_DATASET_HASH_DOMAIN: &[u8] = b"agentir.evaluation.ranking_dataset.v1\0";
/// Dataset-example hash domain.
pub const RANKING_EXAMPLE_HASH_DOMAIN: &[u8] = b"agentir.evaluation.ranking_example.v1\0";
/// Semantic split-group identity domain.
pub const DATASET_GROUP_HASH_DOMAIN: &[u8] = b"agentir.evaluation.dataset_group.v1\0";
/// Dataset-split hash domain.
pub const DATASET_SPLIT_HASH_DOMAIN: &[u8] = b"agentir.evaluation.dataset_split.v1\0";
/// Training-configuration hash domain.
pub const TRAINING_CONFIGURATION_HASH_DOMAIN: &[u8] =
    b"agentir.evaluation.training_configuration.v1\0";
/// Training-run hash domain.
pub const TRAINING_RUN_HASH_DOMAIN: &[u8] = b"agentir.evaluation.training_run.v1\0";
/// Learned-model hash domain.
pub const LEARNED_MODEL_HASH_DOMAIN: &[u8] = b"agentir.evaluation.learned_model.v1\0";
/// Inference-input hash domain.
pub const RANKING_INPUT_HASH_DOMAIN: &[u8] = b"agentir.evaluation.ranking_input.v1\0";
/// Exact inference-record hash domain.
pub const INFERENCE_HASH_DOMAIN: &[u8] = b"agentir.evaluation.inference.v1\0";
/// Training-checkpoint hash domain.
pub const TRAINING_CHECKPOINT_HASH_DOMAIN: &[u8] = b"agentir.evaluation.training_checkpoint.v1\0";

/// Operational limits for offline learned-ranking data and computation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LearnedRankingLimits {
    /// Maximum examples in one dataset.
    pub examples: u64,
    /// Maximum choices in one example or inference.
    pub choices: u64,
    /// Maximum encoded feature count per choice.
    pub features: u64,
    /// Maximum canonical bytes per example.
    pub example_bytes: u64,
    /// Maximum canonical bytes per dataset.
    pub dataset_bytes: u64,
    /// Maximum training epochs.
    pub epochs: u64,
    /// Maximum checked weight updates.
    pub updates: u64,
    /// Maximum serialized model bytes.
    pub model_bytes: u64,
    /// Maximum deterministic work units.
    pub work_units: u64,
}

impl Default for LearnedRankingLimits {
    fn default() -> Self {
        Self {
            examples: 100_000,
            choices: 1_000,
            features: 256,
            example_bytes: 16 * 1024 * 1024,
            dataset_bytes: 256 * 1024 * 1024,
            epochs: 1_000,
            updates: 100_000_000,
            model_bytes: 16 * 1024 * 1024,
            work_units: 1_000_000_000,
        }
    }
}

/// Exact inference input containing only Stage 6B policy-visible fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankingInput {
    /// Input codec version.
    pub version: u32,
    /// Visible feature-schema version.
    pub feature_schema_version: FeatureSchemaVersion,
    /// Exact visible feature-schema identity.
    pub feature_schema_hash: String,
    /// Exact source choice-set identity.
    pub choice_set_hash: String,
    /// Ordered exact choice identities.
    pub ordered_choice_ids: Vec<EvaluationChoiceId>,
    /// Visible feature vectors in the same order.
    pub visible_features: Vec<ChoiceVisibleFeatures>,
    /// Compiler ordering in the same order.
    pub compiler_order: Vec<u64>,
    /// Complete versus bounded frame status.
    pub completeness: FrameCompleteness,
    /// Permitted Stage 6 interaction mode.
    pub interaction_mode: PolicyKind,
    /// Whether the policy has bounded escape rights.
    pub escape_permitted: bool,
    /// Deterministic policy-visible task/corpus metadata only.
    pub task_metadata: BTreeMap<String, String>,
    /// Independent input hash.
    pub ranking_input_hash: String,
}

/// Historical outcome label, stored separately from inference input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct RankingLabel {
    /// Label contract version.
    pub version: u32,
    /// Historically selected visible choice.
    pub selected_choice: Option<EvaluationChoiceId>,
    /// Whether production accepted the selection.
    pub accepted: bool,
    /// Whether the compiler reported progress.
    pub progress_producing: bool,
    /// Visible proof-frontier delta classification.
    pub proof_frontier_delta: String,
    /// Decisions remaining in the historical episode.
    pub decisions_remaining: u64,
    /// Whether a later repair occurred.
    pub repair_occurred: bool,
    /// Historical task completion status.
    pub task_completed: bool,
    /// Historical compiler-owned semantic task criterion.
    pub semantic_task_criterion: bool,
    /// Deterministic context cost.
    pub context_bytes: u64,
    /// Deterministic token cost when known.
    pub deterministic_tokens: Option<u64>,
    /// Optional compatible confidence-only performance observation.
    pub compatible_performance: Option<CompatiblePerformanceLabel>,
}

/// Optional performance label whose artifact/device anchors are explicit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatiblePerformanceLabel {
    /// Exact artifact identity.
    pub artifact_hash: String,
    /// Exact device identity.
    pub device_fingerprint_hash: String,
    /// Confidence-only median observation.
    pub median_ns: u64,
}

/// One immutable learned-ranking example.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RankingExample {
    /// Exact source episode hash.
    pub source_episode_hash: String,
    /// Exact source choice-set hash.
    pub source_choice_set_hash: String,
    /// Policy-visible inference input.
    pub input: RankingInput,
    /// Separate historical label.
    pub label: Option<RankingLabel>,
    /// Stable semantic-state group used for splitting.
    pub semantic_group_hash: String,
    /// Independent example hash.
    pub ranking_example_hash: String,
}

/// Immutable dataset manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankingDatasetManifest {
    /// Dataset contract version.
    pub version: u32,
    /// Stable generator identity.
    pub generator: String,
    /// Exact source corpus identity.
    pub corpus_hash: String,
    /// Exact visible feature schema identity.
    pub feature_schema_hash: String,
    /// Fixed generator seed.
    pub seed: u64,
    /// Ordered example count.
    pub example_count: u64,
    /// Ordered exact source episode hashes.
    pub source_episode_hashes: Vec<String>,
    /// Ordered exact source choice-set hashes.
    pub source_choice_set_hashes: Vec<String>,
    /// Independent dataset hash.
    pub ranking_dataset_hash: String,
}

/// Complete immutable learned-ranking dataset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RankingDataset {
    /// Dataset manifest.
    pub manifest: RankingDatasetManifest,
    /// Examples in deterministic source order.
    pub examples: Vec<RankingExample>,
}

/// Stable split membership.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitKind {
    /// Training groups.
    Train,
    /// Validation groups.
    Validation,
    /// Held-out test groups.
    Test,
    /// Explicitly excluded or incomparable groups.
    Excluded,
}

/// Deterministic group-wise dataset split.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetSplit {
    /// Split contract version.
    pub version: u32,
    /// Exact dataset identity.
    pub ranking_dataset_hash: String,
    /// Fixed split seed.
    pub seed: u64,
    /// Stable group-to-split assignment.
    pub groups: BTreeMap<String, SplitKind>,
    /// Counts by split.
    pub counts: BTreeMap<SplitKind, u64>,
    /// Whether semantic overlap is forbidden.
    pub prohibit_semantic_overlap: bool,
    /// Independent split hash.
    pub dataset_split_hash: String,
}

/// Bounded deterministic training configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainingConfiguration {
    /// Configuration contract version.
    pub version: u32,
    /// Stable algorithm identity.
    pub algorithm: String,
    /// Fixed training seed.
    pub seed: u64,
    /// Exact epoch count; zero is permitted.
    pub epochs: u64,
    /// Checked integer learning rate.
    pub learning_rate: i64,
    /// Maximum absolute model weight.
    pub maximum_weight_magnitude: i64,
    /// Maximum examples visited.
    pub maximum_examples: u64,
    /// Maximum feature dimensions.
    pub maximum_features: u64,
    /// Maximum checked updates.
    pub maximum_updates: u64,
    /// Maximum serialized model bytes.
    pub maximum_model_bytes: u64,
    /// Maximum deterministic work units.
    pub maximum_work_units: u64,
    /// Fixed score scale.
    pub score_scale: i64,
    /// Independent configuration hash.
    pub training_configuration_hash: String,
}

impl TrainingConfiguration {
    /// Returns the bounded deterministic v1 configuration.
    pub fn v1(seed: u64) -> EvaluationResult<Self> {
        let mut configuration = Self {
            version: 1,
            algorithm: "pairwise_integer_perceptron_v1".to_owned(),
            seed,
            epochs: 8,
            learning_rate: 1,
            maximum_weight_magnitude: 1_000_000_000,
            maximum_examples: 100_000,
            maximum_features: 256,
            maximum_updates: 100_000_000,
            maximum_model_bytes: 16 * 1024 * 1024,
            maximum_work_units: 1_000_000_000,
            score_scale: FixedScore::SCALE_V1,
            training_configuration_hash: String::new(),
        };
        configuration.training_configuration_hash = training_configuration_hash(&configuration)?;
        Ok(configuration)
    }
}

/// Restartable deterministic training checkpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainingCheckpoint {
    /// Dataset anchor.
    pub ranking_dataset_hash: String,
    /// Split anchor.
    pub dataset_split_hash: String,
    /// Configuration anchor.
    pub training_configuration_hash: String,
    /// Next epoch to execute.
    pub next_epoch: u64,
    /// Retained checked weights.
    pub weights: Vec<i64>,
    /// Retained checked bias.
    pub bias: i64,
    /// Checked update count.
    pub updates: u64,
    /// Independent checkpoint hash.
    pub checkpoint_hash: String,
}

/// Deterministic fixed-point learned model artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedModelArtifact {
    /// Model format version.
    pub version: u32,
    /// Stable model family.
    pub model_kind: String,
    /// Dataset identity used for training.
    pub ranking_dataset_hash: String,
    /// Split identity used for training.
    pub dataset_split_hash: String,
    /// Training configuration identity.
    pub training_configuration_hash: String,
    /// Exact visible feature-schema identity.
    pub feature_schema_hash: String,
    /// Stable feature codec.
    pub feature_codec: String,
    /// Ordered encoded feature names.
    pub feature_names: Vec<String>,
    /// Checked integer weights.
    pub weights: Vec<i64>,
    /// Checked integer bias.
    pub bias: i64,
    /// Fixed score scale.
    pub score_scale: i64,
    /// Maximum choices accepted at inference.
    pub maximum_choices: u64,
    /// Independent model hash.
    pub learned_model_hash: String,
}

/// Completed deterministic training provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainingRun {
    /// Training-run contract version.
    pub version: u32,
    /// Dataset anchor.
    pub ranking_dataset_hash: String,
    /// Split anchor.
    pub dataset_split_hash: String,
    /// Configuration anchor.
    pub training_configuration_hash: String,
    /// Completed epochs.
    pub completed_epochs: u64,
    /// Checked update count.
    pub updates: u64,
    /// Produced model identity.
    pub learned_model_hash: String,
    /// Restart checkpoint at completion.
    pub checkpoint: TrainingCheckpoint,
    /// Non-semantic deterministic work counts.
    pub work_units: WorkUnitCounters,
    /// Independent training-run hash.
    pub training_run_hash: String,
}

/// Exact fixed-point inference provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceRecord {
    /// Inference contract version.
    pub version: u32,
    /// Exact policy-visible input hash.
    pub ranking_input_hash: String,
    /// Exact choice-set anchor.
    pub choice_set_hash: String,
    /// Exact model identity.
    pub learned_model_hash: String,
    /// Exact learned ranking-policy identity.
    pub ranking_policy_hash: String,
    /// Exact score for every choice in input order.
    pub scores: Vec<ChoiceScore>,
    /// Non-semantic deterministic work counts.
    pub work_units: WorkUnitCounters,
    /// Independent inference hash.
    pub inference_hash: String,
}

/// Builds a checked policy-visible inference input from one exact choice set.
pub fn ranking_input(
    choice_set: &EvaluationChoiceSet,
    schema: &FeatureSchema,
    completeness: FrameCompleteness,
    interaction_mode: PolicyKind,
    escape_permitted: bool,
    task_metadata: BTreeMap<String, String>,
) -> EvaluationResult<RankingInput> {
    if choice_set.feature_schema_hash != schema.feature_schema_hash
        || schema.feature_schema_hash != feature_schema_hash(schema)?
    {
        return Err(error(
            EvaluationErrorCode::EvaluationModelIncompatible,
            "choice set and feature schema are incompatible",
        ));
    }
    let mut input = RankingInput {
        version: 1,
        feature_schema_version: schema.version,
        feature_schema_hash: schema.feature_schema_hash.clone(),
        choice_set_hash: choice_set.choice_set_hash.clone(),
        ordered_choice_ids: choice_set
            .choices
            .iter()
            .map(|choice| choice.id.clone())
            .collect(),
        visible_features: choice_set
            .choices
            .iter()
            .map(|choice| choice.visible_features.clone())
            .collect(),
        compiler_order: choice_set
            .choices
            .iter()
            .map(|choice| choice.compiler_order)
            .collect(),
        completeness,
        interaction_mode,
        escape_permitted,
        task_metadata,
        ranking_input_hash: String::new(),
    };
    validate_ranking_input(&input)?;
    input.ranking_input_hash = ranking_input_hash(&input)?;
    Ok(input)
}

/// Generates an immutable offline dataset from exact retained archive frames.
pub fn generate_dataset(
    archive: &EvaluationArchive,
    seed: u64,
    limits: &LearnedRankingLimits,
) -> EvaluationResult<RankingDataset> {
    let schemas: BTreeMap<_, _> = archive
        .feature_schemas
        .iter()
        .map(|schema| (schema.feature_schema_hash.as_str(), schema))
        .collect();
    let choice_sets: BTreeMap<_, _> = archive
        .choice_sets
        .iter()
        .map(|set| (set.choice_set_hash.as_str(), set))
        .collect();
    let mut examples = Vec::new();
    for run in &archive.runs {
        for episode in &run.episodes {
            let episode_hash = episode.episode_hash.clone().ok_or_else(|| {
                error(
                    EvaluationErrorCode::EvaluationDatasetInvalid,
                    "dataset source episode is not finalized",
                )
            })?;
            for (step_index, step) in episode.steps.iter().enumerate() {
                let Some(trace) = &step.ranking_trace else {
                    continue;
                };
                let choice_set =
                    choice_sets
                        .get(trace.choice_set_hash.as_str())
                        .ok_or_else(|| {
                            error(
                                EvaluationErrorCode::EvaluationDatasetInvalid,
                                "dataset source choice set is missing",
                            )
                        })?;
                let schema = schemas
                    .get(choice_set.feature_schema_hash.as_str())
                    .ok_or_else(|| {
                        error(
                            EvaluationErrorCode::EvaluationDatasetInvalid,
                            "dataset source feature schema is missing",
                        )
                    })?;
                let input = ranking_input(
                    choice_set,
                    schema,
                    FrameCompleteness::Complete,
                    run.policy.kind,
                    run.policy.capabilities.typed_escape,
                    BTreeMap::from([
                        ("task_id".to_owned(), episode.task_id.0.clone()),
                        ("corpus_version".to_owned(), archive.corpus.version.clone()),
                    ]),
                )?;
                let selected = trace.selected_choice.clone();
                let proof_frontier_delta = selected
                    .as_ref()
                    .and_then(|selected| {
                        choice_set
                            .choices
                            .iter()
                            .find(|choice| &choice.id == selected)
                    })
                    .map_or_else(|| "none".to_owned(), |choice| choice.proof_effect.clone());
                let result = episode.result.as_ref();
                let performance =
                    result
                        .and_then(|result| result.performance.as_ref())
                        .map(|observation| CompatiblePerformanceLabel {
                            artifact_hash: observation.artifact_hash.clone(),
                            device_fingerprint_hash: observation.device_fingerprint_hash.clone(),
                            median_ns: observation.median_ns,
                        });
                let label = RankingLabel {
                    version: 1,
                    selected_choice: selected,
                    accepted: step.outcome.accepted,
                    progress_producing: step.outcome.progress_producing,
                    proof_frontier_delta,
                    decisions_remaining: u64::try_from(
                        episode
                            .steps
                            .len()
                            .saturating_sub(step_index.saturating_add(1)),
                    )
                    .unwrap_or(u64::MAX),
                    repair_occurred: episode
                        .steps
                        .iter()
                        .skip(step_index.saturating_add(1))
                        .any(|later| !later.outcome.accepted),
                    task_completed: result.is_some_and(|result| result.success),
                    semantic_task_criterion: result
                        .is_some_and(|result| result.semantic.criterion_satisfied),
                    context_bytes: step.context.observation_bytes,
                    deterministic_tokens: step.context.deterministic_tokens,
                    compatible_performance: performance,
                };
                let semantic_group_hash = semantic_group_hash(&input)?;
                let mut example = RankingExample {
                    source_episode_hash: episode_hash.clone(),
                    source_choice_set_hash: choice_set.choice_set_hash.clone(),
                    input,
                    label: Some(label),
                    semantic_group_hash,
                    ranking_example_hash: String::new(),
                };
                example.ranking_example_hash = ranking_example_hash(&example)?;
                check_len(
                    examples.len().saturating_add(1),
                    limits.examples,
                    "dataset examples",
                )?;
                check_bytes(&example, limits.example_bytes, "dataset example bytes")?;
                examples.push(example);
            }
        }
    }
    let feature_schema_hash = examples
        .first()
        .map(|example| example.input.feature_schema_hash.clone())
        .or_else(|| {
            archive
                .feature_schemas
                .first()
                .map(|schema| schema.feature_schema_hash.clone())
        })
        .unwrap_or_default();
    let mut dataset = RankingDataset {
        manifest: RankingDatasetManifest {
            version: 1,
            generator: "agentir_stage6c_offline_dataset_v1".to_owned(),
            corpus_hash: archive.corpus.corpus_hash.clone(),
            feature_schema_hash,
            seed,
            example_count: u64::try_from(examples.len()).unwrap_or(u64::MAX),
            source_episode_hashes: examples
                .iter()
                .map(|example| example.source_episode_hash.clone())
                .collect(),
            source_choice_set_hashes: examples
                .iter()
                .map(|example| example.source_choice_set_hash.clone())
                .collect(),
            ranking_dataset_hash: String::new(),
        },
        examples,
    };
    validate_dataset(&dataset, limits, true)?;
    dataset.manifest.ranking_dataset_hash = ranking_dataset_hash(&dataset)?;
    check_bytes(&dataset, limits.dataset_bytes, "dataset bytes")?;
    validate_dataset(&dataset, limits, true)?;
    Ok(dataset)
}

/// Builds a deterministic dataset from already separated visible inputs and historical labels.
pub fn build_dataset(
    corpus_hash: impl Into<String>,
    feature_schema_hash: impl Into<String>,
    seed: u64,
    records: Vec<(String, RankingInput, RankingLabel)>,
    limits: &LearnedRankingLimits,
) -> EvaluationResult<RankingDataset> {
    let mut examples = Vec::with_capacity(records.len());
    for (source_episode_hash, input, label) in records {
        validate_ranking_input(&input)?;
        if label.version != 1 {
            return Err(error(
                EvaluationErrorCode::EvaluationDatasetInvalid,
                "future or unsupported label version",
            ));
        }
        let semantic_group_hash = semantic_group_hash(&input)?;
        let mut example = RankingExample {
            source_episode_hash,
            source_choice_set_hash: input.choice_set_hash.clone(),
            input,
            label: Some(label),
            semantic_group_hash,
            ranking_example_hash: String::new(),
        };
        example.ranking_example_hash = ranking_example_hash(&example)?;
        examples.push(example);
    }
    let mut dataset = RankingDataset {
        manifest: RankingDatasetManifest {
            version: 1,
            generator: "agentir_stage6c_offline_dataset_v1".to_owned(),
            corpus_hash: corpus_hash.into(),
            feature_schema_hash: feature_schema_hash.into(),
            seed,
            example_count: u64::try_from(examples.len()).unwrap_or(u64::MAX),
            source_episode_hashes: examples
                .iter()
                .map(|example| example.source_episode_hash.clone())
                .collect(),
            source_choice_set_hashes: examples
                .iter()
                .map(|example| example.source_choice_set_hash.clone())
                .collect(),
            ranking_dataset_hash: String::new(),
        },
        examples,
    };
    validate_dataset(&dataset, limits, true)?;
    dataset.manifest.ranking_dataset_hash = ranking_dataset_hash(&dataset)?;
    validate_dataset(&dataset, limits, true)?;
    Ok(dataset)
}

/// Validates dataset structure and rejects input/label leakage.
pub fn validate_dataset(
    dataset: &RankingDataset,
    limits: &LearnedRankingLimits,
    require_labels: bool,
) -> EvaluationResult<()> {
    if dataset.manifest.version != 1
        || dataset.manifest.generator != "agentir_stage6c_offline_dataset_v1"
        || dataset.manifest.example_count
            != u64::try_from(dataset.examples.len()).unwrap_or(u64::MAX)
        || dataset.manifest.source_episode_hashes
            != dataset
                .examples
                .iter()
                .map(|example| example.source_episode_hash.clone())
                .collect::<Vec<_>>()
        || dataset.manifest.source_choice_set_hashes
            != dataset
                .examples
                .iter()
                .map(|example| example.source_choice_set_hash.clone())
                .collect::<Vec<_>>()
        || dataset.examples.iter().any(|example| {
            example.input.feature_schema_hash != dataset.manifest.feature_schema_hash
        })
    {
        return Err(error(
            EvaluationErrorCode::EvaluationDatasetInvalid,
            "dataset manifest shape is invalid",
        ));
    }
    check_len(dataset.examples.len(), limits.examples, "dataset examples")?;
    let mut example_hashes = BTreeSet::new();
    for example in &dataset.examples {
        validate_ranking_input(&example.input)?;
        if example.semantic_group_hash != semantic_group_hash(&example.input)?
            || example.ranking_example_hash != ranking_example_hash(example)?
            || example.source_choice_set_hash != example.input.choice_set_hash
            || !example_hashes.insert(example.ranking_example_hash.clone())
        {
            return Err(error(
                EvaluationErrorCode::EvaluationDatasetInvalid,
                "dataset example hash, source anchor, or uniqueness invariant failed",
            ));
        }
        if require_labels && example.label.is_none() {
            return Err(error(
                EvaluationErrorCode::EvaluationDatasetInvalid,
                "training dataset example is missing its separate label",
            ));
        }
        if example
            .label
            .as_ref()
            .is_some_and(|label| label.version != 1)
        {
            return Err(error(
                EvaluationErrorCode::EvaluationDatasetInvalid,
                "future or unsupported label version",
            ));
        }
        check_bytes(example, limits.example_bytes, "dataset example bytes")?;
    }
    if !dataset.manifest.ranking_dataset_hash.is_empty()
        && dataset.manifest.ranking_dataset_hash != ranking_dataset_hash(dataset)?
    {
        return Err(error(
            EvaluationErrorCode::EvaluationDatasetInvalid,
            "dataset hash mismatch",
        ));
    }
    check_bytes(dataset, limits.dataset_bytes, "dataset bytes")
}

/// Creates a fixed-seed group split with no semantic-state overlap.
pub fn split_dataset(dataset: &RankingDataset, seed: u64) -> EvaluationResult<DatasetSplit> {
    let mut groups = BTreeMap::new();
    let mut counts = BTreeMap::from([
        (SplitKind::Train, 0_u64),
        (SplitKind::Validation, 0_u64),
        (SplitKind::Test, 0_u64),
        (SplitKind::Excluded, 0_u64),
    ]);
    for example in &dataset.examples {
        if groups.contains_key(&example.semantic_group_hash) {
            continue;
        }
        let bucket_hash = domain_hash(
            DATASET_SPLIT_HASH_DOMAIN,
            &(seed, &example.semantic_group_hash),
        )?;
        let bucket = u8::from_str_radix(&bucket_hash[..2], 16).unwrap_or(255) % 100;
        let kind = match bucket {
            0..=69 => SplitKind::Train,
            70..=84 => SplitKind::Validation,
            85..=94 => SplitKind::Test,
            _ => SplitKind::Excluded,
        };
        groups.insert(example.semantic_group_hash.clone(), kind);
        counts
            .entry(kind)
            .and_modify(|count| *count = count.saturating_add(1));
    }
    let mut split = DatasetSplit {
        version: 1,
        ranking_dataset_hash: dataset.manifest.ranking_dataset_hash.clone(),
        seed,
        groups,
        counts,
        prohibit_semantic_overlap: true,
        dataset_split_hash: String::new(),
    };
    split.dataset_split_hash = dataset_split_hash(&split)?;
    validate_split(dataset, &split)?;
    Ok(split)
}

/// Validates exact split coverage and absence of semantic-state leakage.
pub fn validate_split(dataset: &RankingDataset, split: &DatasetSplit) -> EvaluationResult<()> {
    if split.version != 1
        || split.ranking_dataset_hash != dataset.manifest.ranking_dataset_hash
        || split.dataset_split_hash != dataset_split_hash(split)?
        || !split.prohibit_semantic_overlap
    {
        return Err(error(
            EvaluationErrorCode::EvaluationDatasetLeakage,
            "dataset split identity or policy is invalid",
        ));
    }
    let expected_groups: BTreeSet<_> = dataset
        .examples
        .iter()
        .map(|example| example.semantic_group_hash.as_str())
        .collect();
    let actual_groups: BTreeSet<_> = split.groups.keys().map(String::as_str).collect();
    if expected_groups != actual_groups {
        return Err(error(
            EvaluationErrorCode::EvaluationDatasetLeakage,
            "dataset split must classify every semantic group exactly once",
        ));
    }
    let mut recomputed = BTreeMap::from([
        (SplitKind::Train, 0_u64),
        (SplitKind::Validation, 0_u64),
        (SplitKind::Test, 0_u64),
        (SplitKind::Excluded, 0_u64),
    ]);
    for kind in split.groups.values() {
        recomputed
            .entry(*kind)
            .and_modify(|count| *count = count.saturating_add(1));
    }
    if recomputed != split.counts {
        return Err(error(
            EvaluationErrorCode::EvaluationDatasetLeakage,
            "dataset split counts do not match group assignments",
        ));
    }
    Ok(())
}

/// Trains the deterministic pairwise integer ranker, optionally from a checkpoint.
pub fn train_linear_ranker(
    dataset: &RankingDataset,
    split: &DatasetSplit,
    schema: &FeatureSchema,
    configuration: &TrainingConfiguration,
    checkpoint: Option<&TrainingCheckpoint>,
    limits: &LearnedRankingLimits,
) -> EvaluationResult<(TrainingRun, LearnedModelArtifact)> {
    validate_dataset(dataset, limits, true)?;
    validate_split(dataset, split)?;
    validate_training_configuration(configuration, limits)?;
    if schema.feature_schema_hash != dataset.manifest.feature_schema_hash
        || schema.feature_schema_hash != feature_schema_hash(schema)?
    {
        return Err(error(
            EvaluationErrorCode::EvaluationModelIncompatible,
            "training schema is incompatible with the dataset",
        ));
    }
    let feature_names = schema
        .definitions
        .iter()
        .map(|definition| definition.name.clone())
        .chain(std::iter::once("compiler_order".to_owned()))
        .collect::<Vec<_>>();
    check_len(
        feature_names.len(),
        configuration.maximum_features.min(limits.features),
        "training features",
    )?;
    let training_examples = dataset
        .examples
        .iter()
        .filter(|example| split.groups.get(&example.semantic_group_hash) == Some(&SplitKind::Train))
        .collect::<Vec<_>>();
    if training_examples.is_empty() {
        return Err(error(
            EvaluationErrorCode::EvaluationTrainingInvalid,
            "training split is empty",
        ));
    }
    check_len(
        training_examples.len(),
        configuration.maximum_examples.min(limits.examples),
        "training examples",
    )?;
    let (mut weights, bias, start_epoch, mut updates) = if let Some(checkpoint) = checkpoint {
        validate_checkpoint(
            dataset,
            split,
            configuration,
            checkpoint,
            feature_names.len(),
        )?;
        (
            checkpoint.weights.clone(),
            checkpoint.bias,
            checkpoint.next_epoch,
            checkpoint.updates,
        )
    } else {
        (vec![0_i64; feature_names.len()], 0_i64, 0_u64, 0_u64)
    };
    let mut work = WorkUnitCounters::default();
    let mut ordered = training_examples;
    ordered.sort_by_key(|example| {
        domain_hash(
            TRAINING_CONFIGURATION_HASH_DOMAIN,
            &(configuration.seed, &example.ranking_example_hash),
        )
        .unwrap_or_default()
    });
    for _epoch in start_epoch..configuration.epochs {
        for example in &ordered {
            let label = example.label.as_ref().ok_or_else(|| {
                error(
                    EvaluationErrorCode::EvaluationTrainingInvalid,
                    "training example label is missing",
                )
            })?;
            if !(label.accepted || label.progress_producing || label.task_completed) {
                continue;
            }
            let Some(selected) = label.selected_choice.as_ref() else {
                continue;
            };
            let encoded = encode_input(&example.input, schema, limits)?;
            let Some(selected_index) = example
                .input
                .ordered_choice_ids
                .iter()
                .position(|choice| choice == selected)
            else {
                return Err(error(
                    EvaluationErrorCode::EvaluationTrainingInvalid,
                    "training label selected choice is outside its input",
                ));
            };
            for (index, other) in encoded.iter().enumerate() {
                if index == selected_index {
                    continue;
                }
                let preferred = &encoded[selected_index];
                let preferred_score = checked_dot(&weights, preferred, bias)?;
                let other_score = checked_dot(&weights, other, bias)?;
                if preferred_score <= other_score {
                    for (weight, (positive, negative)) in
                        weights.iter_mut().zip(preferred.iter().zip(other))
                    {
                        let delta = positive
                            .checked_sub(*negative)
                            .and_then(|value| value.checked_mul(configuration.learning_rate))
                            .ok_or_else(training_overflow)?;
                        let next = weight.checked_add(delta).ok_or_else(training_overflow)?;
                        if next == i64::MIN
                            || next.unsigned_abs()
                                > configuration.maximum_weight_magnitude.unsigned_abs()
                        {
                            return Err(training_overflow());
                        }
                        *weight = next;
                        updates = updates.checked_add(1).ok_or_else(training_overflow)?;
                        if updates > configuration.maximum_updates.min(limits.updates) {
                            return Err(error(
                                EvaluationErrorCode::EvaluationTrainingInvalid,
                                "training update limit exceeded",
                            ));
                        }
                    }
                }
                work.feature_extraction = work
                    .feature_extraction
                    .checked_add(u64::try_from(feature_names.len()).unwrap_or(u64::MAX))
                    .ok_or_else(training_overflow)?;
                work.score_validation = work
                    .score_validation
                    .checked_add(2)
                    .ok_or_else(training_overflow)?;
            }
        }
    }
    work.validate_limit(configuration.maximum_work_units.min(limits.work_units))?;
    let mut model = LearnedModelArtifact {
        version: 1,
        model_kind: "pairwise_integer_linear_v1".to_owned(),
        ranking_dataset_hash: dataset.manifest.ranking_dataset_hash.clone(),
        dataset_split_hash: split.dataset_split_hash.clone(),
        training_configuration_hash: configuration.training_configuration_hash.clone(),
        feature_schema_hash: schema.feature_schema_hash.clone(),
        feature_codec: "visible_feature_integer_codec_v1".to_owned(),
        feature_names,
        weights,
        bias,
        score_scale: configuration.score_scale,
        maximum_choices: limits.choices,
        learned_model_hash: String::new(),
    };
    validate_model(&model, schema, limits, true)?;
    model.learned_model_hash = learned_model_hash(&model)?;
    check_bytes(
        &model,
        configuration.maximum_model_bytes.min(limits.model_bytes),
        "learned model bytes",
    )?;
    let mut completed_checkpoint = TrainingCheckpoint {
        ranking_dataset_hash: dataset.manifest.ranking_dataset_hash.clone(),
        dataset_split_hash: split.dataset_split_hash.clone(),
        training_configuration_hash: configuration.training_configuration_hash.clone(),
        next_epoch: configuration.epochs,
        weights: model.weights.clone(),
        bias: model.bias,
        updates,
        checkpoint_hash: String::new(),
    };
    completed_checkpoint.checkpoint_hash = training_checkpoint_hash(&completed_checkpoint)?;
    let mut run = TrainingRun {
        version: 1,
        ranking_dataset_hash: dataset.manifest.ranking_dataset_hash.clone(),
        dataset_split_hash: split.dataset_split_hash.clone(),
        training_configuration_hash: configuration.training_configuration_hash.clone(),
        completed_epochs: configuration.epochs,
        updates,
        learned_model_hash: model.learned_model_hash.clone(),
        checkpoint: completed_checkpoint,
        work_units: work,
        training_run_hash: String::new(),
    };
    run.training_run_hash = training_run_hash(&run)?;
    Ok((run, model))
}

/// Creates a distinct learned-policy descriptor bound to one exact model.
pub fn learned_policy(
    model: &LearnedModelArtifact,
    mode: PolicyKind,
    allowed_escape: bool,
    maximum_inference_work_units: u64,
) -> EvaluationResult<RankingPolicyDescriptor> {
    let mut policy = RankingPolicyDescriptor {
        name: "offline_integer_linear_v1".to_owned(),
        version: "1".to_owned(),
        kind: RankingPolicyKind::LearnedLinear,
        base_interaction_mode: mode,
        feature_schema_hash: model.feature_schema_hash.clone(),
        configuration: BTreeMap::from([
            (
                "learned_model_hash".to_owned(),
                json!(model.learned_model_hash),
            ),
            ("dataset_schema_version".to_owned(), json!(1)),
            ("feature_codec".to_owned(), json!(model.feature_codec)),
            ("score_scale".to_owned(), json!(model.score_scale)),
            ("maximum_choices".to_owned(), json!(model.maximum_choices)),
            (
                "maximum_inference_work_units".to_owned(),
                json!(maximum_inference_work_units),
            ),
            ("deterministic".to_owned(), json!(true)),
        ]),
        score_representation: "signed_fixed_point_i64_scale_1000000_v1".to_owned(),
        tie_break_rule: TieBreakRule::CompilerOrderThenChoiceId,
        maximum_choices: model.maximum_choices,
        allowed_escape,
        opaque_model_identifier: Some(model.learned_model_hash.clone()),
        prompt_template_hash: None,
        ranking_policy_hash: String::new(),
    };
    policy.ranking_policy_hash = ranking_policy_hash(&policy)?;
    Ok(policy)
}

/// Executes bounded deterministic inference without compiler mutation.
pub fn infer(
    input: &RankingInput,
    schema: &FeatureSchema,
    model: &LearnedModelArtifact,
    policy: &RankingPolicyDescriptor,
    limits: &LearnedRankingLimits,
) -> EvaluationResult<(RankingDecision, InferenceRecord)> {
    validate_ranking_input(input)?;
    validate_model(model, schema, limits, false)?;
    validate_learned_policy(policy, model, input)?;
    check_len(
        input.ordered_choice_ids.len(),
        limits.choices,
        "inference choices",
    )?;
    let encoded = encode_input(input, schema, limits)?;
    let mut scores = Vec::with_capacity(encoded.len());
    for (choice_id, vector) in input.ordered_choice_ids.iter().zip(&encoded) {
        let units = checked_dot(&model.weights, vector, model.bias)?;
        scores.push(ChoiceScore {
            choice_id: choice_id.clone(),
            score: FixedScore::v1(units, RankingLimits::default().score_magnitude)?,
        });
    }
    let mut work_units = WorkUnitCounters {
        feature_extraction: u64::try_from(encoded.len().saturating_mul(model.weights.len()))
            .unwrap_or(u64::MAX),
        score_validation: u64::try_from(scores.len()).unwrap_or(u64::MAX),
        canonical_encoding: 1,
        hashing: 1,
        ..WorkUnitCounters::default()
    };
    work_units.tie_resolution = 0;
    let maximum_work = policy
        .configuration
        .get("maximum_inference_work_units")
        .and_then(Value::as_u64)
        .unwrap_or(limits.work_units)
        .min(limits.work_units);
    work_units.validate_limit(maximum_work)?;
    let mut record = InferenceRecord {
        version: 1,
        ranking_input_hash: input.ranking_input_hash.clone(),
        choice_set_hash: input.choice_set_hash.clone(),
        learned_model_hash: model.learned_model_hash.clone(),
        ranking_policy_hash: policy.ranking_policy_hash.clone(),
        scores: scores.clone(),
        work_units,
        inference_hash: String::new(),
    };
    record.inference_hash = inference_hash(&record)?;
    Ok((RankingDecision::Scores { scores }, record))
}

/// Verifies retained inference by exact fixed-point recomputation.
pub fn verify_inference(
    input: &RankingInput,
    schema: &FeatureSchema,
    model: &LearnedModelArtifact,
    policy: &RankingPolicyDescriptor,
    record: &InferenceRecord,
    limits: &LearnedRankingLimits,
) -> EvaluationResult<()> {
    if record.inference_hash != inference_hash(record)? {
        return Err(error(
            EvaluationErrorCode::EvaluationInferenceInvalid,
            "inference record hash mismatch",
        ));
    }
    let (_, recomputed) = infer(input, schema, model, policy, limits)?;
    if record.version != 1
        || record.ranking_input_hash != input.ranking_input_hash
        || record.choice_set_hash != input.choice_set_hash
        || record.learned_model_hash != model.learned_model_hash
        || record.ranking_policy_hash != policy.ranking_policy_hash
        || record.scores != recomputed.scores
    {
        return Err(error(
            EvaluationErrorCode::EvaluationInferenceInvalid,
            "retained inference differs from exact fixed-point recomputation",
        ));
    }
    Ok(())
}

/// Verifies one retained training configuration against operational limits.
pub fn validate_training_configuration_contract(
    configuration: &TrainingConfiguration,
    limits: &LearnedRankingLimits,
) -> EvaluationResult<()> {
    validate_training_configuration(configuration, limits)
}

/// Verifies one retained learned-model artifact.
pub fn validate_model_artifact(
    model: &LearnedModelArtifact,
    schema: &FeatureSchema,
    limits: &LearnedRankingLimits,
) -> EvaluationResult<()> {
    validate_model(model, schema, limits, false)
}

/// Verifies retained training provenance and its exact artifact anchors.
pub fn validate_training_run(
    run: &TrainingRun,
    dataset: &RankingDataset,
    split: &DatasetSplit,
    configuration: &TrainingConfiguration,
    model: &LearnedModelArtifact,
) -> EvaluationResult<()> {
    if run.version != 1
        || run.training_run_hash != training_run_hash(run)?
        || run.ranking_dataset_hash != dataset.manifest.ranking_dataset_hash
        || run.dataset_split_hash != split.dataset_split_hash
        || run.training_configuration_hash != configuration.training_configuration_hash
        || run.learned_model_hash != model.learned_model_hash
        || run.checkpoint.checkpoint_hash != training_checkpoint_hash(&run.checkpoint)?
        || run.checkpoint.ranking_dataset_hash != run.ranking_dataset_hash
        || run.checkpoint.dataset_split_hash != run.dataset_split_hash
        || run.checkpoint.training_configuration_hash != run.training_configuration_hash
        || run.checkpoint.next_epoch != run.completed_epochs
        || run.checkpoint.updates != run.updates
        || run.checkpoint.weights != model.weights
        || run.checkpoint.bias != model.bias
    {
        return Err(error(
            EvaluationErrorCode::EvaluationTrainingInvalid,
            "training run, checkpoint, or learned model anchors are invalid",
        ));
    }
    Ok(())
}

/// Computes dataset identity without trusting the stored hash.
pub fn ranking_dataset_hash(dataset: &RankingDataset) -> EvaluationResult<String> {
    let mut model = dataset.clone();
    model.manifest.ranking_dataset_hash.clear();
    domain_hash(RANKING_DATASET_HASH_DOMAIN, &model)
}

/// Computes example identity without trusting the stored hash.
pub fn ranking_example_hash(example: &RankingExample) -> EvaluationResult<String> {
    let mut model = example.clone();
    model.ranking_example_hash.clear();
    domain_hash(RANKING_EXAMPLE_HASH_DOMAIN, &model)
}

/// Computes split identity without trusting the stored hash.
pub fn dataset_split_hash(split: &DatasetSplit) -> EvaluationResult<String> {
    let mut model = split.clone();
    model.dataset_split_hash.clear();
    domain_hash(DATASET_SPLIT_HASH_DOMAIN, &model)
}

/// Computes configuration identity without trusting the stored hash.
pub fn training_configuration_hash(
    configuration: &TrainingConfiguration,
) -> EvaluationResult<String> {
    let mut model = configuration.clone();
    model.training_configuration_hash.clear();
    domain_hash(TRAINING_CONFIGURATION_HASH_DOMAIN, &model)
}

/// Computes training-run identity while excluding non-semantic work counters.
pub fn training_run_hash(run: &TrainingRun) -> EvaluationResult<String> {
    let mut model = run.clone();
    model.training_run_hash.clear();
    model.work_units = WorkUnitCounters::default();
    domain_hash(TRAINING_RUN_HASH_DOMAIN, &model)
}

/// Computes model identity without trusting the stored hash.
pub fn learned_model_hash(model: &LearnedModelArtifact) -> EvaluationResult<String> {
    let mut value = model.clone();
    value.learned_model_hash.clear();
    domain_hash(LEARNED_MODEL_HASH_DOMAIN, &value)
}

/// Computes input identity without trusting the stored hash.
pub fn ranking_input_hash(input: &RankingInput) -> EvaluationResult<String> {
    let mut model = input.clone();
    model.ranking_input_hash.clear();
    domain_hash(RANKING_INPUT_HASH_DOMAIN, &model)
}

/// Computes inference identity while excluding non-semantic work counters.
pub fn inference_hash(record: &InferenceRecord) -> EvaluationResult<String> {
    let mut model = record.clone();
    model.inference_hash.clear();
    model.work_units = WorkUnitCounters::default();
    domain_hash(INFERENCE_HASH_DOMAIN, &model)
}

fn semantic_group_hash(input: &RankingInput) -> EvaluationResult<String> {
    let model = (
        input.version,
        input.feature_schema_version,
        &input.feature_schema_hash,
        &input.ordered_choice_ids,
        &input.visible_features,
        &input.compiler_order,
        input.completeness,
        input.interaction_mode,
        input.escape_permitted,
        &input.task_metadata,
    );
    domain_hash(DATASET_GROUP_HASH_DOMAIN, &model)
}

fn training_checkpoint_hash(checkpoint: &TrainingCheckpoint) -> EvaluationResult<String> {
    let mut model = checkpoint.clone();
    model.checkpoint_hash.clear();
    domain_hash(TRAINING_CHECKPOINT_HASH_DOMAIN, &model)
}

fn validate_ranking_input(input: &RankingInput) -> EvaluationResult<()> {
    if input.version != 1
        || input.feature_schema_version != FeatureSchemaVersion(1)
        || input.ordered_choice_ids.len() != input.visible_features.len()
        || input.ordered_choice_ids.len() != input.compiler_order.len()
        || input
            .compiler_order
            .iter()
            .enumerate()
            .any(|(index, order)| *order != u64::try_from(index).unwrap_or(u64::MAX))
        || !input.ranking_input_hash.is_empty()
            && input.ranking_input_hash != ranking_input_hash(input)?
    {
        return Err(error(
            EvaluationErrorCode::EvaluationDatasetInvalid,
            "ranking input structure or identity is invalid",
        ));
    }
    let allowed_metadata = ["task_id", "corpus_version", "task_category", "task_size"];
    if input
        .task_metadata
        .keys()
        .any(|key| !allowed_metadata.contains(&key.as_str()) || is_forbidden_input_name(key))
    {
        return Err(error(
            EvaluationErrorCode::EvaluationDatasetLeakage,
            "ranking input contains forbidden or non-visible metadata",
        ));
    }
    let mut ids = BTreeSet::new();
    if input.ordered_choice_ids.iter().any(|id| !ids.insert(id)) {
        return Err(error(
            EvaluationErrorCode::EvaluationDatasetInvalid,
            "ranking input contains duplicate choice identities",
        ));
    }
    Ok(())
}

fn is_forbidden_input_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    [
        "future",
        "outcome",
        "correct_choice",
        "reference",
        "proof_state",
        "benchmark",
        "final_success",
        "policy_score",
        "provider",
        "split",
        "label",
        "device",
    ]
    .iter()
    .any(|forbidden| normalized.contains(forbidden))
}

fn validate_training_configuration(
    configuration: &TrainingConfiguration,
    limits: &LearnedRankingLimits,
) -> EvaluationResult<()> {
    if configuration.version != 1
        || configuration.algorithm != "pairwise_integer_perceptron_v1"
        || configuration.training_configuration_hash != training_configuration_hash(configuration)?
        || configuration.learning_rate <= 0
        || configuration.maximum_weight_magnitude <= 0
        || configuration.score_scale != FixedScore::SCALE_V1
        || configuration.epochs > limits.epochs
        || configuration.maximum_updates > limits.updates
        || configuration.maximum_model_bytes > limits.model_bytes
        || configuration.maximum_work_units > limits.work_units
    {
        return Err(error(
            EvaluationErrorCode::EvaluationTrainingInvalid,
            "training configuration is invalid or exceeds operational limits",
        ));
    }
    Ok(())
}

fn validate_checkpoint(
    dataset: &RankingDataset,
    split: &DatasetSplit,
    configuration: &TrainingConfiguration,
    checkpoint: &TrainingCheckpoint,
    feature_count: usize,
) -> EvaluationResult<()> {
    if checkpoint.checkpoint_hash != training_checkpoint_hash(checkpoint)?
        || checkpoint.ranking_dataset_hash != dataset.manifest.ranking_dataset_hash
        || checkpoint.dataset_split_hash != split.dataset_split_hash
        || checkpoint.training_configuration_hash != configuration.training_configuration_hash
        || checkpoint.next_epoch > configuration.epochs
        || checkpoint.weights.len() != feature_count
    {
        return Err(error(
            EvaluationErrorCode::EvaluationTrainingInvalid,
            "training checkpoint is corrupt or incompatible",
        ));
    }
    Ok(())
}

fn validate_model(
    model: &LearnedModelArtifact,
    schema: &FeatureSchema,
    limits: &LearnedRankingLimits,
    allow_empty_hash: bool,
) -> EvaluationResult<()> {
    let expected_names = schema
        .definitions
        .iter()
        .map(|definition| definition.name.as_str())
        .chain(std::iter::once("compiler_order"))
        .collect::<Vec<_>>();
    if model.version != 1
        || model.model_kind != "pairwise_integer_linear_v1"
        || model.feature_codec != "visible_feature_integer_codec_v1"
        || model.feature_schema_hash != schema.feature_schema_hash
        || model.score_scale != FixedScore::SCALE_V1
        || model
            .feature_names
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != expected_names
        || model.weights.len() != model.feature_names.len()
        || model.maximum_choices > limits.choices
        || (!allow_empty_hash && model.learned_model_hash != learned_model_hash(model)?)
        || (allow_empty_hash
            && !model.learned_model_hash.is_empty()
            && model.learned_model_hash != learned_model_hash(model)?)
    {
        return Err(error(
            EvaluationErrorCode::EvaluationModelInvalid,
            "learned model format, schema, codec, dimensions, or hash is invalid",
        ));
    }
    check_bytes(model, limits.model_bytes, "learned model bytes")
}

fn validate_learned_policy(
    policy: &RankingPolicyDescriptor,
    model: &LearnedModelArtifact,
    input: &RankingInput,
) -> EvaluationResult<()> {
    if policy.kind != RankingPolicyKind::LearnedLinear
        || policy.ranking_policy_hash != ranking_policy_hash(policy)?
        || policy.feature_schema_hash != model.feature_schema_hash
        || policy.feature_schema_hash != input.feature_schema_hash
        || policy.opaque_model_identifier.as_deref() != Some(model.learned_model_hash.as_str())
        || policy.base_interaction_mode != input.interaction_mode
        || policy.allowed_escape != input.escape_permitted
    {
        return Err(error(
            EvaluationErrorCode::EvaluationModelIncompatible,
            "learned policy, model, schema, or inference frame is incompatible",
        ));
    }
    Ok(())
}

fn encode_input(
    input: &RankingInput,
    schema: &FeatureSchema,
    limits: &LearnedRankingLimits,
) -> EvaluationResult<Vec<Vec<i64>>> {
    if input.feature_schema_hash != schema.feature_schema_hash {
        return Err(error(
            EvaluationErrorCode::EvaluationModelIncompatible,
            "feature codec schema mismatch",
        ));
    }
    check_len(
        input.ordered_choice_ids.len(),
        limits.choices,
        "encoded choices",
    )?;
    let feature_count = schema.definitions.len().saturating_add(1);
    check_len(feature_count, limits.features, "encoded features")?;
    input
        .visible_features
        .iter()
        .zip(&input.compiler_order)
        .map(|(visible, order)| {
            let mut encoded = Vec::with_capacity(feature_count);
            for definition in &schema.definitions {
                let value = visible.values.get(&definition.name).ok_or_else(|| {
                    error(
                        EvaluationErrorCode::EvaluationDatasetInvalid,
                        "feature codec input is missing a schema feature",
                    )
                })?;
                let type_matches = matches!(
                    (definition.feature_type, value),
                    (FeatureType::Integer, FeatureValue::Integer(_))
                        | (FeatureType::Boolean, FeatureValue::Boolean(_))
                        | (FeatureType::Text, FeatureValue::Text(_))
                        | (FeatureType::TextList, FeatureValue::TextList(_))
                );
                if !type_matches {
                    return Err(error(
                        EvaluationErrorCode::EvaluationDatasetInvalid,
                        "feature codec input has the wrong schema type",
                    ));
                }
                if definition.normalization == "non_negative_i64"
                    && matches!(value, FeatureValue::Integer(value) if *value < 0)
                {
                    return Err(error(
                        EvaluationErrorCode::EvaluationDatasetInvalid,
                        "feature codec input violates non-negative normalization",
                    ));
                }
                encoded.push(encode_feature(value)?);
            }
            encoded.push(i64::try_from(*order).map_err(|_| {
                error(
                    EvaluationErrorCode::EvaluationTrainingOverflow,
                    "compiler order exceeds fixed-point codec range",
                )
            })?);
            Ok(encoded)
        })
        .collect()
}

fn encode_feature(value: &FeatureValue) -> EvaluationResult<i64> {
    match value {
        FeatureValue::Integer(value) => Ok(*value),
        FeatureValue::Boolean(value) => Ok(i64::from(*value)),
        FeatureValue::Text(value) => stable_text_value(value),
        FeatureValue::TextList(values) => {
            if values.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(error(
                    EvaluationErrorCode::EvaluationDatasetInvalid,
                    "text-list feature must be sorted and unique",
                ));
            }
            values.iter().try_fold(0_i64, |total, value| {
                total
                    .checked_add(stable_text_value(value)?)
                    .ok_or_else(training_overflow)
            })
        }
    }
}

fn stable_text_value(value: &str) -> EvaluationResult<i64> {
    let digest = domain_hash(RANKING_INPUT_HASH_DOMAIN, &value)?;
    let prefix = u64::from_str_radix(&digest[..16], 16).map_err(|_| {
        error(
            EvaluationErrorCode::EvaluationDatasetInvalid,
            "feature text hash could not be decoded",
        )
    })?;
    Ok(i64::try_from(prefix % 2_001).unwrap_or(0) - 1_000)
}

fn checked_dot(weights: &[i64], features: &[i64], bias: i64) -> EvaluationResult<i64> {
    if weights.len() != features.len() {
        return Err(error(
            EvaluationErrorCode::EvaluationModelInvalid,
            "model weight and encoded feature dimensions differ",
        ));
    }
    weights
        .iter()
        .zip(features)
        .try_fold(bias, |total, (weight, feature)| {
            weight
                .checked_mul(*feature)
                .and_then(|term| total.checked_add(term))
                .ok_or_else(training_overflow)
        })
}

fn check_len(actual: usize, maximum: u64, resource: &str) -> EvaluationResult<()> {
    let actual = u64::try_from(actual).unwrap_or(u64::MAX);
    if actual > maximum {
        return Err(error(
            EvaluationErrorCode::EvaluationTrainingInvalid,
            format!("{resource} limit exceeded"),
        )
        .expected_actual(json!(maximum), json!(actual)));
    }
    Ok(())
}

fn check_bytes<T: Serialize>(value: &T, maximum: u64, resource: &str) -> EvaluationResult<()> {
    let bytes = serde_json::to_vec(value).map_err(|encoding| {
        error(
            EvaluationErrorCode::EvaluationDatasetInvalid,
            format!("{resource} encoding failed: {encoding}"),
        )
    })?;
    check_len(bytes.len(), maximum, resource)
}

fn training_overflow() -> EvaluationDiagnostic {
    error(
        EvaluationErrorCode::EvaluationTrainingOverflow,
        "checked integer training or inference arithmetic overflow",
    )
}

fn error(code: EvaluationErrorCode, message: impl Into<String>) -> EvaluationDiagnostic {
    EvaluationDiagnostic::new(code, message)
}
