//! Domain-separated evaluation hashing.

use crate::model::{
    EvaluationAggregate, EvaluationArchive, EvaluationCorpus, EvaluationDiagnostic,
    EvaluationEpisode, EvaluationErrorCode, EvaluationObservation, EvaluationResult, EvaluationRun,
    PolicyDescriptor,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Corpus canonical hash domain.
pub const CORPUS_HASH_DOMAIN: &[u8] = b"agentir.evaluation.corpus.v1\0";
/// Policy canonical hash domain.
pub const POLICY_HASH_DOMAIN: &[u8] = b"agentir.evaluation.policy.v1\0";
/// Observation canonical hash domain.
pub const OBSERVATION_HASH_DOMAIN: &[u8] = b"agentir.evaluation.observation.v1\0";
/// Episode canonical hash domain.
pub const EPISODE_HASH_DOMAIN: &[u8] = b"agentir.evaluation.episode.v1\0";
/// Ranked episode transcript hash domain.
pub const EPISODE_HASH_V2_DOMAIN: &[u8] = b"agentir.evaluation.episode.v2\0";
/// Aggregate/evaluation canonical hash domain.
pub const AGGREGATE_HASH_DOMAIN: &[u8] = b"agentir.evaluation.aggregate.v1\0";
/// Separate evaluation archive hash domain.
pub const ARCHIVE_HASH_DOMAIN: &[u8] = b"agentir.evaluation.archive.v1\0";
/// Evaluation archive v2 hash domain.
pub const ARCHIVE_HASH_V2_DOMAIN: &[u8] = b"agentir.evaluation.archive.v2\0";
/// Evaluation archive v3 hash domain.
pub const ARCHIVE_HASH_V3_DOMAIN: &[u8] = b"agentir.evaluation.archive.v3\0";

fn canonical<T: Serialize>(value: &T) -> EvaluationResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| {
        EvaluationDiagnostic::new(
            EvaluationErrorCode::EvaluationHashMismatch,
            format!("canonical evaluation encoding failed: {error}"),
        )
    })
}

/// Hashes deterministic JSON bytes using a distinct domain prefix.
pub fn domain_hash<T: Serialize>(domain: &[u8], value: &T) -> EvaluationResult<String> {
    let bytes = canonical(value)?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Clones a retained-hash contract, clears only its self-hash field, and hashes it.
///
/// The caller must still provide the distinct semantic domain; this helper
/// deliberately unifies mechanics without merging hash contracts.
pub fn domain_hash_cleared<T, F>(
    domain: &[u8],
    value: &T,
    clear_self_hash: F,
) -> EvaluationResult<String>
where
    T: Clone + Serialize,
    F: FnOnce(&mut T),
{
    let mut model = value.clone();
    clear_self_hash(&mut model);
    domain_hash(domain, &model)
}

pub(crate) fn corpus_hash(corpus: &EvaluationCorpus) -> EvaluationResult<String> {
    domain_hash_cleared(CORPUS_HASH_DOMAIN, corpus, |model| {
        model.corpus_hash.clear();
    })
}

pub(crate) fn policy_hash(policy: &PolicyDescriptor) -> EvaluationResult<String> {
    domain_hash_cleared(POLICY_HASH_DOMAIN, policy, |model| {
        model.policy_hash.clear();
    })
}

pub(crate) fn observation_hash(observation: &EvaluationObservation) -> EvaluationResult<String> {
    let mut model = observation.clone();
    model.observation_hash.clear();
    model.choice_set_hash = None;
    model.feature_schema_hash = None;
    domain_hash(OBSERVATION_HASH_DOMAIN, &model)
}

pub(crate) fn episode_hash(episode: &EvaluationEpisode) -> EvaluationResult<String> {
    let mut model = episode.clone();
    model.episode_hash = None;
    let ranked = model
        .steps
        .iter()
        .any(|step| step.ranking_trace.is_some() || step.selection.is_some());
    domain_hash(
        if ranked {
            EPISODE_HASH_V2_DOMAIN
        } else {
            EPISODE_HASH_DOMAIN
        },
        &model,
    )
}

pub(crate) fn evaluation_hash(run: &EvaluationRun) -> EvaluationResult<String> {
    let model = (
        &run.corpus_hash,
        &run.policy.policy_hash,
        &run.compiler_build_hash,
        &run.seeds,
        run.episodes
            .iter()
            .map(|episode| episode.episode_hash.as_deref())
            .collect::<Vec<_>>(),
    );
    domain_hash(AGGREGATE_HASH_DOMAIN, &model)
}

pub(crate) fn aggregate_hash(aggregate: &EvaluationAggregate) -> EvaluationResult<String> {
    domain_hash_cleared(AGGREGATE_HASH_DOMAIN, aggregate, |model| {
        model.aggregate_hash.clear();
    })
}

pub(crate) fn archive_hash(archive: &EvaluationArchive) -> EvaluationResult<String> {
    let mut model = archive.clone();
    model.archive_hash.clear();
    domain_hash(
        match model.manifest.version {
            2 => ARCHIVE_HASH_V2_DOMAIN,
            3 => ARCHIVE_HASH_V3_DOMAIN,
            _ => ARCHIVE_HASH_DOMAIN,
        },
        &model,
    )
}
