//! Persistent BackendIR plans, artifact packages, hashes, and replayable records.

use crate::{
    backend_ir::{
        ARTIFACT_FORMAT_VERSION, ARTIFACT_VALIDATOR_VERSION, ArtifactCertificate, ArtifactPackage,
        ArtifactStatus, BACKEND_CANONICAL_VERSION, BACKEND_SEMANTICS_VERSION,
        BACKEND_VALIDATOR_VERSION, BackendAnchor, BackendCertificate, BackendEvidence,
        BackendObligation, BackendProgram, BackendStatus, DeviceFingerprint,
        HardwareMeasurementRecord,
    },
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{
        ArtifactId, ArtifactModuleId, BackendKernelId, BackendPlanId, BackendRevisionId,
        BackendValueId, MeasurementId, ScheduleNodeId,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fmt::Write as _,
};

/// Backend event replay semantics version.
pub const BACKEND_EVENT_SEMANTICS_VERSION: u32 = 1;
/// Artifact event replay semantics version.
pub const ARTIFACT_EVENT_SEMANTICS_VERSION: u32 = 1;
/// Measurement event replay semantics version.
pub const MEASUREMENT_EVENT_SEMANTICS_VERSION: u32 = 1;
/// Domain separator for exact BackendIR identity.
pub const BACKEND_HASH_DOMAIN: &[u8] = b"agentir.backend.wgsl.exact.v1\0";
/// Domain separator for compiler build identity.
pub const COMPILER_BUILD_HASH_DOMAIN: &[u8] = b"agentir.compiler.build.v1\0";
/// Domain separator for exact artifact packages.
pub const ARTIFACT_HASH_DOMAIN: &[u8] = b"agentir.artifact.wgsl.package.v1\0";
/// Domain separator for device fingerprints.
pub const DEVICE_FINGERPRINT_HASH_DOMAIN: &[u8] = b"agentir.device.fingerprint.v1\0";
/// Domain separator for hardware measurement records.
pub const MEASUREMENT_HASH_DOMAIN: &[u8] = b"agentir.measurement.hardware.v1\0";

macro_rules! hash_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Creates a hash from lowercase hexadecimal text.
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Returns lowercase hexadecimal text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

hash_type!(
    BackendHash,
    "Exact identity of one typed BackendIR revision."
);
hash_type!(
    CompilerBuildHash,
    "Identity of the deterministic compiler build contract."
);
hash_type!(
    ArtifactHash,
    "Exact identity of a reproducible artifact package."
);
hash_type!(
    DeviceFingerprintHash,
    "Identity of reported device/runtime capabilities."
);
hash_type!(
    MeasurementHash,
    "Exact identity of one confidence-only measurement record."
);

fn digest(domain: &[u8], bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn backend_error(code: ErrorCode, message: impl Into<String>) -> AgentError {
    AgentError::new(code, message)
}

/// Backend/artifact-local staged monotonic allocator.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendAllocator {
    plan: u64,
    revision: u64,
    kernel: u64,
    value: u64,
    artifact: u64,
    module: u64,
    measurement: u64,
}

impl BackendAllocator {
    /// Allocates a backend plan identity.
    pub fn plan(&mut self) -> BackendPlanId {
        self.plan = self.plan.saturating_add(1);
        BackendPlanId::new(format!("bp{}", self.plan))
    }

    /// Allocates a backend revision identity.
    pub fn revision(&mut self) -> BackendRevisionId {
        self.revision = self.revision.saturating_add(1);
        BackendRevisionId::new(format!("br{}", self.revision))
    }

    /// Allocates a kernel identity.
    pub fn kernel(&mut self) -> BackendKernelId {
        self.kernel = self.kernel.saturating_add(1);
        BackendKernelId::new(format!("bk{}", self.kernel))
    }

    /// Allocates a backend SSA value identity.
    pub fn value(&mut self) -> BackendValueId {
        self.value = self.value.saturating_add(1);
        BackendValueId::new(format!("bv{}", self.value))
    }

    /// Allocates an artifact identity.
    pub fn artifact(&mut self) -> ArtifactId {
        self.artifact = self.artifact.saturating_add(1);
        ArtifactId::new(format!("art{}", self.artifact))
    }

    /// Allocates an artifact module identity.
    pub fn module(&mut self) -> ArtifactModuleId {
        self.module = self.module.saturating_add(1);
        ArtifactModuleId::new(format!("wm{}", self.module))
    }

    /// Allocates a measurement identity.
    pub fn measurement(&mut self) -> MeasurementId {
        self.measurement = self.measurement.saturating_add(1);
        MeasurementId::new(format!("meas{}", self.measurement))
    }
}

/// One immutable BackendIR revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendRevision {
    /// Revision identity.
    pub id: BackendRevisionId,
    /// Immutable parents.
    pub parents: Vec<BackendRevisionId>,
    /// Separate typed backend graph.
    pub program: BackendProgram,
    /// Exact BackendIR hash.
    pub backend_hash: BackendHash,
    /// Lifecycle state.
    pub status: BackendStatus,
    /// Compiler-owned equivalence certificate.
    pub certificate: BackendCertificate,
    /// Confidence-only evidence.
    pub evidence: Vec<BackendEvidence>,
    /// Remaining correctness obligations.
    pub obligations: Vec<BackendObligation>,
}

/// Immutable backend plan revision DAG.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendPlan {
    /// Plan identity.
    pub id: BackendPlanId,
    /// Immutable Stage 1-4 anchor.
    pub anchor: BackendAnchor,
    /// Current branch head.
    pub head: BackendRevisionId,
    /// Immutable revisions.
    pub revisions: BTreeMap<BackendRevisionId, BackendRevision>,
}

/// Summary returned by backend queries and checks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendQuery {
    /// Plan identity.
    pub backend_plan: BackendPlanId,
    /// Revision identity.
    pub backend_revision: BackendRevisionId,
    /// Exact backend hash.
    pub backend_hash: BackendHash,
    /// Immutable schedule hash.
    pub schedule_hash: crate::schedule::ScheduleHash,
    /// Immutable target hash.
    pub target_hash: crate::target::TargetHash,
    /// Lifecycle state.
    pub status: BackendStatus,
    /// Number of kernels.
    pub kernel_count: usize,
    /// Number of ordered dispatches.
    pub dispatch_count: usize,
    /// Total storage bindings across kernels.
    pub binding_count: usize,
    /// Ordered distinct vector widths retained by kernels.
    pub vector_widths: Vec<u32>,
    /// Ordered distinct bounded unroll factors retained by kernels.
    pub unroll_factors: Vec<u32>,
    /// Whether a lazy exact guard path exists.
    pub guarded: bool,
    /// Open obligation count.
    pub open_obligations: usize,
}

/// Full BackendIR structural verification report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendCheckReport {
    /// Read-only query summary.
    pub query: BackendQuery,
    /// Whether the graph is structurally well typed.
    pub well_typed: bool,
    /// Whether compiler-owned proof establishes BackendEquivalentToSchedule.
    pub equivalent_to_schedule: bool,
    /// Whether the revision may emit an artifact.
    pub emittable: bool,
}

/// Bounded deterministic lowering continuation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendContinuation {
    /// Selected schedule hash.
    pub schedule_hash: crate::schedule::ScheduleHash,
    /// Supported backend kind.
    pub backend_kind: crate::backend_ir::BackendKind,
    /// Conservative serial lowering is supported for the elementwise subset.
    pub serial_available: bool,
    /// Supported vector widths.
    pub vector_widths: Vec<u32>,
    /// Unsupported constructs found by structural preflight.
    pub unsupported: Vec<String>,
}

/// Replayable BackendIR event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackendEvent {
    /// A complete compiler-lowered plan was atomically published.
    Lowered {
        /// Exact published plan.
        plan: BackendPlan,
        /// Candidate event dependency cursor.
        candidate_event_cursor: u64,
        /// Equality event dependency cursor.
        equality_event_cursor: u64,
        /// Memory event dependency cursor.
        memory_event_cursor: u64,
        /// Target event dependency cursor.
        target_event_cursor: u64,
        /// Schedule event dependency cursor.
        schedule_event_cursor: u64,
    },
    /// An immutable backend revision was copied into an independent plan.
    Forked {
        /// Parent plan identity.
        source_plan: BackendPlanId,
        /// Parent immutable revision.
        source_revision: BackendRevisionId,
        /// Exact expected parent hash.
        expected_backend_hash: BackendHash,
        /// Complete compiler-owned forked plan.
        plan: BackendPlan,
    },
    /// A proved backend revision was sealed immutably.
    Sealed {
        /// Plan identity.
        backend_plan: BackendPlanId,
        /// Parent revision.
        base_revision: BackendRevisionId,
        /// Exact expected parent hash.
        expected_backend_hash: BackendHash,
        /// Published sealed revision.
        revision: BackendRevision,
    },
}

/// Backend event with independent replay semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedBackendEvent {
    /// Backend event semantics version.
    pub semantics_version: u32,
    /// Replayable event.
    pub event: BackendEvent,
}

/// Persistent BackendIR plans and replay state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendStore {
    /// Backend plans keyed by compiler identity.
    pub plans: BTreeMap<BackendPlanId, BackendPlan>,
    /// Shared backend/artifact allocator.
    pub allocator: BackendAllocator,
    /// Ordered backend event log.
    pub events: Vec<VersionedBackendEvent>,
}

#[derive(Serialize)]
struct BackendHashModel<'a> {
    codec: &'static str,
    canonical_version: u32,
    semantics_version: u32,
    event_semantics_version: u32,
    validator_version: u32,
    plan: &'a BackendPlanId,
    revision: &'a BackendRevisionId,
    parents: &'a [BackendRevisionId],
    anchor: &'a BackendAnchor,
    program: &'a BackendProgram,
    status: BackendStatus,
    certificate_relation: &'a str,
    certificate_schedule_hash: &'a crate::schedule::ScheduleHash,
    certificate_coverage: &'a [ScheduleNodeId],
    certificate_conditions: &'a [String],
    evidence: &'a [BackendEvidence],
    obligations: &'a [BackendObligation],
}

/// Returns deterministic exact BackendIR canonical bytes.
pub fn canonical_backend_bytes(
    plan: &BackendPlanId,
    anchor: &BackendAnchor,
    revision: &BackendRevision,
) -> AgentResult<Vec<u8>> {
    serde_json::to_vec(&BackendHashModel {
        codec: "agentir.backend.wgsl.exact",
        canonical_version: BACKEND_CANONICAL_VERSION,
        semantics_version: BACKEND_SEMANTICS_VERSION,
        event_semantics_version: BACKEND_EVENT_SEMANTICS_VERSION,
        validator_version: BACKEND_VALIDATOR_VERSION,
        plan,
        revision: &revision.id,
        parents: &revision.parents,
        anchor,
        program: &revision.program,
        status: revision.status,
        certificate_relation: &revision.certificate.relation,
        certificate_schedule_hash: &revision.certificate.schedule_hash,
        certificate_coverage: &revision.certificate.schedule_node_coverage,
        certificate_conditions: &revision.certificate.conditions,
        evidence: &revision.evidence,
        obligations: &revision.obligations,
    })
    .map_err(|error| {
        backend_error(
            ErrorCode::CanonicalizationFailed,
            format!("BackendIR canonicalization failed: {error}"),
        )
    })
}

fn compute_backend_hash(
    plan: &BackendPlanId,
    anchor: &BackendAnchor,
    revision: &BackendRevision,
) -> AgentResult<BackendHash> {
    Ok(BackendHash(digest(
        BACKEND_HASH_DOMAIN,
        &canonical_backend_bytes(plan, anchor, revision)?,
    )))
}

fn statement_values(
    statements: &[crate::backend_ir::BackendStatement],
    output: &mut Vec<BackendValueId>,
) {
    for statement in statements {
        match statement {
            crate::backend_ir::BackendStatement::Store { index, value, .. } => {
                output.push(index.clone());
                output.push(value.clone());
            }
            crate::backend_ir::BackendStatement::SerialLoop { index, body, .. } => {
                output.push(index.clone());
                statement_values(body, output);
            }
            crate::backend_ir::BackendStatement::IfBounds { predicate, body } => {
                output.push(predicate.clone());
                statement_values(body, output);
            }
        }
    }
}

/// Verifies a BackendIR graph against exact compiler-owned schedule coverage.
pub fn verify_backend_program(
    program: &BackendProgram,
    expected_schedule_nodes: &[ScheduleNodeId],
) -> AgentResult<()> {
    if program.kernel_order.len() != program.kernels.len()
        || program.kernel_order.iter().collect::<BTreeSet<_>>().len() != program.kernel_order.len()
        || program
            .kernel_order
            .iter()
            .any(|kernel| !program.kernels.contains_key(kernel))
    {
        return Err(backend_error(
            ErrorCode::BackendCoverageInvalid,
            "kernel order is incomplete, duplicated, or references a missing kernel",
        ));
    }
    let coverage = program
        .kernel_order
        .iter()
        .flat_map(|kernel| {
            program.kernels[kernel]
                .source_schedule_nodes
                .iter()
                .cloned()
        })
        .collect::<Vec<_>>();
    if coverage != expected_schedule_nodes {
        return Err(backend_error(
            ErrorCode::BackendCoverageInvalid,
            "backend kernels do not cover ScheduleIR nodes in exact order",
        )
        .with_types(
            serde_json::json!(expected_schedule_nodes),
            serde_json::json!(coverage),
        )
        .with_repair("lower the selected immutable ScheduleIR revision again"));
    }
    for kernel in program.kernels.values() {
        if kernel.entry_point.is_empty()
            || kernel.workgroup_size.contains(&0)
            || kernel
                .workgroup_size
                .iter()
                .try_fold(1_u64, |total, value| total.checked_mul(u64::from(*value)))
                .is_none()
            || ![1, 2, 4].contains(&kernel.vector_width)
            || kernel.unroll_factor == 0
        {
            return Err(backend_error(
                ErrorCode::BackendDispatchInvalid,
                "kernel entry point, workgroup, vector, or unroll shape is invalid",
            ));
        }
        let mut seen_bindings = BTreeSet::new();
        for binding in &kernel.bindings {
            if binding.group != 0
                || binding.alignment == 0
                || !binding.alignment.is_power_of_two()
                || !seen_bindings.insert(binding.binding)
            {
                return Err(backend_error(
                    ErrorCode::BackendBindingInvalid,
                    "kernel storage binding layout is invalid or duplicated",
                ));
            }
        }
        if !kernel.parameter_block.entries.is_empty()
            && (seen_bindings.contains(&kernel.parameter_block.binding)
                || kernel.parameter_block.group != 0
                || kernel.parameter_block.byte_size % 16 != 0)
        {
            return Err(backend_error(
                ErrorCode::BackendBindingInvalid,
                "uniform parameter block overlaps storage bindings or violates v1 alignment",
            ));
        }
        let mut used_values = Vec::new();
        statement_values(&kernel.statements, &mut used_values);
        if used_values
            .iter()
            .any(|value| !kernel.values.contains_key(value))
        {
            return Err(backend_error(
                ErrorCode::BackendDispatchInvalid,
                "kernel statement references a missing typed BackendIR value",
            ));
        }
    }
    if program.dispatches.len() != program.kernels.len()
        || program
            .dispatches
            .iter()
            .enumerate()
            .any(|(index, dispatch)| {
                dispatch.order != u64::try_from(index).unwrap_or(u64::MAX)
                    || !program.kernels.contains_key(&dispatch.kernel)
                    || dispatch.workgroup_size != program.kernels[&dispatch.kernel].workgroup_size
            })
    {
        return Err(backend_error(
            ErrorCode::BackendDispatchInvalid,
            "dispatch graph is not a complete deterministic kernel order",
        ));
    }
    if let Some(guard) = &program.guard {
        let dispatch_orders = program
            .dispatches
            .iter()
            .map(|dispatch| dispatch.order)
            .collect::<BTreeSet<_>>();
        if guard.true_dispatches.is_empty()
            || guard.false_dispatches.is_empty()
            || guard
                .true_dispatches
                .iter()
                .chain(&guard.false_dispatches)
                .any(|order| !dispatch_orders.contains(order))
        {
            return Err(backend_error(
                ErrorCode::BackendDispatchInvalid,
                "guarded dispatch plan lacks one exact branch or references a missing dispatch",
            ));
        }
    }
    Ok(())
}

impl BackendStore {
    /// Returns one backend plan.
    pub fn plan(&self, plan: &BackendPlanId) -> AgentResult<&BackendPlan> {
        self.plans.get(plan).ok_or_else(|| {
            backend_error(
                ErrorCode::BackendPlanNotFound,
                format!("backend plan `{plan}` does not exist"),
            )
        })
    }

    /// Returns one immutable backend revision.
    pub fn revision(
        &self,
        plan: &BackendPlanId,
        revision: &BackendRevisionId,
    ) -> AgentResult<&BackendRevision> {
        self.plan(plan)?.revisions.get(revision).ok_or_else(|| {
            backend_error(
                ErrorCode::BackendRevisionNotFound,
                format!("backend revision `{revision}` does not exist"),
            )
        })
    }

    /// Atomically lowers a new backend plan through a trusted compiler component.
    #[allow(clippy::too_many_arguments)]
    pub fn lower_with<F>(
        &mut self,
        anchor: BackendAnchor,
        expected_schedule_nodes: &[ScheduleNodeId],
        candidate_event_cursor: u64,
        equality_event_cursor: u64,
        memory_event_cursor: u64,
        target_event_cursor: u64,
        schedule_event_cursor: u64,
        lower: F,
    ) -> AgentResult<BackendCheckReport>
    where
        F: FnOnce(&mut BackendAllocator) -> AgentResult<BackendProgram>,
    {
        let mut allocator = self.allocator.clone();
        let program = lower(&mut allocator)?;
        verify_backend_program(&program, expected_schedule_nodes)?;
        let plan_id = allocator.plan();
        let revision_id = allocator.revision();
        let mut revision = BackendRevision {
            id: revision_id.clone(),
            parents: Vec::new(),
            program,
            backend_hash: BackendHash::new("pending"),
            status: BackendStatus::Proved,
            certificate: BackendCertificate {
                relation: "backend_equivalent_to_schedule".to_owned(),
                schedule_hash: anchor.schedule_hash.clone(),
                backend_hash: BackendHash::new("pending"),
                schedule_node_coverage: expected_schedule_nodes.to_vec(),
                conditions: vec![
                    "immutable_anchor_chain_verified".to_owned(),
                    "ordered_schedule_node_coverage_exact".to_owned(),
                    "memory_buffer_binding_map_verified".to_owned(),
                    "dispatch_index_and_remainder_mapping_exact".to_owned(),
                    "numeric_contract_preserved".to_owned(),
                ],
                semantics_version: BACKEND_SEMANTICS_VERSION,
                validator_version: BACKEND_VALIDATOR_VERSION,
            },
            evidence: Vec::new(),
            obligations: Vec::new(),
        };
        revision.backend_hash = compute_backend_hash(&plan_id, &anchor, &revision)?;
        revision.certificate.backend_hash = revision.backend_hash.clone();
        let plan = BackendPlan {
            id: plan_id.clone(),
            anchor,
            head: revision_id.clone(),
            revisions: BTreeMap::from([(revision_id, revision)]),
        };
        let event = VersionedBackendEvent {
            semantics_version: BACKEND_EVENT_SEMANTICS_VERSION,
            event: BackendEvent::Lowered {
                plan: plan.clone(),
                candidate_event_cursor,
                equality_event_cursor,
                memory_event_cursor,
                target_event_cursor,
                schedule_event_cursor,
            },
        };
        self.plans.insert(plan_id.clone(), plan);
        self.allocator = allocator;
        self.events.push(event);
        self.check(&plan_id, &self.plans[&plan_id].head.clone())
    }

    /// Reads one backend summary.
    pub fn query(
        &self,
        plan: &BackendPlanId,
        revision: &BackendRevisionId,
    ) -> AgentResult<BackendQuery> {
        let plan_data = self.plan(plan)?;
        let revision_data = self.revision(plan, revision)?;
        let vector_widths = revision_data
            .program
            .kernel_order
            .iter()
            .map(|kernel| revision_data.program.kernels[kernel].vector_width)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let unroll_factors = revision_data
            .program
            .kernel_order
            .iter()
            .map(|kernel| revision_data.program.kernels[kernel].unroll_factor)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        Ok(BackendQuery {
            backend_plan: plan.clone(),
            backend_revision: revision.clone(),
            backend_hash: revision_data.backend_hash.clone(),
            schedule_hash: plan_data.anchor.schedule_hash.clone(),
            target_hash: plan_data.anchor.target_hash.clone(),
            status: revision_data.status,
            kernel_count: revision_data.program.kernels.len(),
            dispatch_count: revision_data.program.dispatches.len(),
            binding_count: revision_data
                .program
                .kernels
                .values()
                .map(|kernel| kernel.bindings.len())
                .sum(),
            vector_widths,
            unroll_factors,
            guarded: revision_data.program.guard.is_some(),
            open_obligations: revision_data
                .obligations
                .iter()
                .filter(|obligation| obligation.status != "proved")
                .count(),
        })
    }

    /// Fully checks BackendIR structure, proof certificate, and exact hash.
    pub fn check(
        &self,
        plan: &BackendPlanId,
        revision: &BackendRevisionId,
    ) -> AgentResult<BackendCheckReport> {
        let plan_data = self.plan(plan)?;
        let revision_data = self.revision(plan, revision)?;
        verify_backend_program(
            &revision_data.program,
            &revision_data.certificate.schedule_node_coverage,
        )?;
        if revision_data.certificate.relation != "backend_equivalent_to_schedule"
            || revision_data.certificate.schedule_hash != plan_data.anchor.schedule_hash
            || revision_data.certificate.semantics_version != BACKEND_SEMANTICS_VERSION
            || revision_data.certificate.validator_version != BACKEND_VALIDATOR_VERSION
            || revision_data.certificate.backend_hash != revision_data.backend_hash
            || !revision_data.obligations.is_empty()
        {
            return Err(backend_error(
                ErrorCode::BackendEquivalenceUnproved,
                "BackendEquivalentToSchedule certificate is missing or inconsistent",
            ));
        }
        let actual = compute_backend_hash(plan, &plan_data.anchor, revision_data)?;
        if actual != revision_data.backend_hash {
            return Err(backend_error(
                ErrorCode::BackendHashMismatch,
                "stored backend_hash does not match exact typed BackendIR state",
            )
            .with_types(revision_data.backend_hash.to_string(), actual.to_string()));
        }
        Ok(BackendCheckReport {
            query: self.query(plan, revision)?,
            well_typed: true,
            equivalent_to_schedule: true,
            emittable: matches!(
                revision_data.status,
                BackendStatus::Proved | BackendStatus::Sealed
            ),
        })
    }

    /// Seals a proved backend revision as one immutable child.
    pub fn seal(
        &mut self,
        plan: &BackendPlanId,
        base_revision: &BackendRevisionId,
        expected_hash: &BackendHash,
    ) -> AgentResult<BackendCheckReport> {
        self.check(plan, base_revision)?;
        let plan_data = self.plan(plan)?;
        if &plan_data.head != base_revision {
            return Err(backend_error(
                ErrorCode::StaleBackendBase,
                "backend.seal requires the current plan head",
            ));
        }
        let base = self.revision(plan, base_revision)?;
        if &base.backend_hash != expected_hash {
            return Err(backend_error(
                ErrorCode::BackendHashMismatch,
                "backend.seal expected hash differs from the selected revision",
            )
            .with_types(expected_hash.to_string(), base.backend_hash.to_string()));
        }
        let mut allocator = self.allocator.clone();
        let next_id = allocator.revision();
        let mut next = base.clone();
        next.id = next_id.clone();
        next.parents = vec![base_revision.clone()];
        next.status = BackendStatus::Sealed;
        next.backend_hash = BackendHash::new("pending");
        next.certificate.backend_hash = BackendHash::new("pending");
        next.backend_hash = compute_backend_hash(plan, &plan_data.anchor, &next)?;
        next.certificate.backend_hash = next.backend_hash.clone();
        let event = VersionedBackendEvent {
            semantics_version: BACKEND_EVENT_SEMANTICS_VERSION,
            event: BackendEvent::Sealed {
                backend_plan: plan.clone(),
                base_revision: base_revision.clone(),
                expected_backend_hash: expected_hash.clone(),
                revision: next.clone(),
            },
        };
        let plan_data = self.plans.get_mut(plan).expect("plan checked above");
        plan_data.head = next_id.clone();
        plan_data.revisions.insert(next_id.clone(), next);
        self.allocator = allocator;
        self.events.push(event);
        self.check(plan, &next_id)
    }

    /// Forks one proved immutable revision into an independent backend plan.
    pub fn fork(
        &mut self,
        source_plan: &BackendPlanId,
        source_revision: &BackendRevisionId,
        expected_hash: &BackendHash,
    ) -> AgentResult<BackendCheckReport> {
        self.check(source_plan, source_revision)?;
        let source_plan_data = self.plan(source_plan)?.clone();
        let source = self.revision(source_plan, source_revision)?.clone();
        if &source.backend_hash != expected_hash {
            return Err(backend_error(
                ErrorCode::BackendHashMismatch,
                "backend.fork expected hash differs from the selected revision",
            )
            .with_types(expected_hash.to_string(), source.backend_hash.to_string()));
        }
        let mut allocator = self.allocator.clone();
        let plan_id = allocator.plan();
        let revision_id = allocator.revision();
        let mut revision = source;
        revision.id = revision_id.clone();
        revision.parents = vec![source_revision.clone()];
        revision.backend_hash = BackendHash::new("pending");
        revision.certificate.backend_hash = BackendHash::new("pending");
        revision.backend_hash =
            compute_backend_hash(&plan_id, &source_plan_data.anchor, &revision)?;
        revision.certificate.backend_hash = revision.backend_hash.clone();
        let plan = BackendPlan {
            id: plan_id.clone(),
            anchor: source_plan_data.anchor,
            head: revision_id.clone(),
            revisions: BTreeMap::from([(revision_id.clone(), revision)]),
        };
        self.plans.insert(plan_id.clone(), plan.clone());
        self.allocator = allocator;
        self.events.push(VersionedBackendEvent {
            semantics_version: BACKEND_EVENT_SEMANTICS_VERSION,
            event: BackendEvent::Forked {
                source_plan: source_plan.clone(),
                source_revision: source_revision.clone(),
                expected_backend_hash: expected_hash.clone(),
                plan,
            },
        });
        self.check(&plan_id, &revision_id)
    }

    /// Revalidates every backend plan and exact hash.
    pub fn verify_all(&self) -> AgentResult<()> {
        for (plan, plan_data) in &self.plans {
            if !plan_data.revisions.contains_key(&plan_data.head) {
                return Err(backend_error(
                    ErrorCode::BackendRevisionNotFound,
                    "backend plan head references a missing revision",
                ));
            }
            for revision in plan_data.revisions.keys() {
                self.check(plan, revision)?;
            }
        }
        Ok(())
    }

    /// Verifies that the shared backend/artifact/measurement allocator resumes
    /// exactly after every retained persistent identity.
    pub fn verify_allocator_state(
        &self,
        artifacts: &ArtifactStore,
        measurements: &MeasurementStore,
    ) -> AgentResult<()> {
        fn suffix(value: &impl ToString, prefix: &str) -> AgentResult<u64> {
            value
                .to_string()
                .strip_prefix(prefix)
                .and_then(|suffix| suffix.parse::<u64>().ok())
                .ok_or_else(|| {
                    backend_error(
                        ErrorCode::BackendEventOrderInvalid,
                        format!("persistent backend identity does not match `{prefix}*`"),
                    )
                })
        }
        let maximum = |values: Vec<AgentResult<u64>>| -> AgentResult<u64> {
            values
                .into_iter()
                .try_fold(0_u64, |current, value| Ok(current.max(value?)))
        };
        let expected = BackendAllocator {
            plan: maximum(self.plans.keys().map(|id| suffix(id, "bp")).collect())?,
            revision: maximum(
                self.plans
                    .values()
                    .flat_map(|plan| plan.revisions.keys())
                    .map(|id| suffix(id, "br"))
                    .collect(),
            )?,
            kernel: maximum(
                self.plans
                    .values()
                    .flat_map(|plan| plan.revisions.values())
                    .flat_map(|revision| revision.program.kernels.keys())
                    .map(|id| suffix(id, "bk"))
                    .collect(),
            )?,
            value: maximum(
                self.plans
                    .values()
                    .flat_map(|plan| plan.revisions.values())
                    .flat_map(|revision| revision.program.kernels.values())
                    .flat_map(|kernel| kernel.values.keys())
                    .map(|id| suffix(id, "bv"))
                    .collect(),
            )?,
            artifact: maximum(
                artifacts
                    .packages
                    .keys()
                    .map(|id| suffix(id, "art"))
                    .collect(),
            )?,
            module: maximum(
                artifacts
                    .packages
                    .values()
                    .flat_map(|package| package.modules.iter())
                    .map(|module| suffix(&module.id, "wm"))
                    .collect(),
            )?,
            measurement: maximum(
                measurements
                    .records
                    .keys()
                    .map(|id| suffix(id, "meas"))
                    .collect(),
            )?,
        };
        if self.allocator != expected {
            return Err(backend_error(
                ErrorCode::BackendEventOrderInvalid,
                "backend allocator state does not resume after retained IDs",
            )
            .with_detail("expected_allocator", serde_json::json!(expected))
            .with_detail("actual_allocator", serde_json::json!(self.allocator)));
        }
        Ok(())
    }
}

/// Current deterministic compiler build identity.
#[must_use]
pub fn compiler_build_hash() -> CompilerBuildHash {
    let model = format!(
        "agentir:{}:backend={}:validator={}:artifact={}",
        env!("CARGO_PKG_VERSION"),
        BACKEND_SEMANTICS_VERSION,
        BACKEND_VALIDATOR_VERSION,
        ARTIFACT_VALIDATOR_VERSION
    );
    CompilerBuildHash(digest(COMPILER_BUILD_HASH_DOMAIN, model.as_bytes()))
}

#[derive(Serialize)]
struct ArtifactHashModel<'a> {
    manifest: &'a crate::backend_ir::ArtifactManifest,
    modules: &'a [crate::backend_ir::ArtifactModule],
    offline_validation: &'a crate::backend_ir::OfflineValidationReport,
    status: ArtifactStatus,
    certificate_relation: &'a str,
    certificate_conditions: &'a [String],
    validator_version: u32,
}

/// Returns deterministic canonical bytes for an exact artifact package.
pub fn canonical_artifact_bytes(package: &ArtifactPackage) -> AgentResult<Vec<u8>> {
    serde_json::to_vec(&ArtifactHashModel {
        manifest: &package.manifest,
        modules: &package.modules,
        offline_validation: &package.offline_validation,
        status: package.status,
        certificate_relation: &package.certificate.relation,
        certificate_conditions: &package.certificate.conditions,
        validator_version: package.certificate.validator_version,
    })
    .map_err(|error| {
        backend_error(
            ErrorCode::CanonicalizationFailed,
            format!("artifact package canonicalization failed: {error}"),
        )
    })
}

/// Computes an artifact package hash without device/runtime state.
pub fn artifact_hash(package: &ArtifactPackage) -> AgentResult<ArtifactHash> {
    Ok(ArtifactHash(digest(
        ARTIFACT_HASH_DOMAIN,
        &canonical_artifact_bytes(package)?,
    )))
}

/// Structurally verifies source/manifest consistency and emission proof data.
pub fn verify_artifact(package: &ArtifactPackage, backend: &BackendRevision) -> AgentResult<()> {
    if package.manifest.format != "agentir.wgsl.package"
        || package.manifest.format_version != ARTIFACT_FORMAT_VERSION
        || package.manifest.backend_hash != backend.backend_hash
        || package.manifest.dispatches != backend.program.dispatches
        || package.manifest.guard != backend.program.guard
        || package.manifest.outputs != backend.program.outputs
        || package.manifest.compiler_build_hash != compiler_build_hash()
    {
        return Err(backend_error(
            ErrorCode::ArtifactManifestInvalid,
            "artifact manifest differs from immutable BackendIR or compiler build",
        ));
    }
    let expected_layouts = backend
        .program
        .kernel_order
        .iter()
        .map(|kernel_id| {
            let kernel = &backend.program.kernels[kernel_id];
            crate::backend_ir::ArtifactBindingLayout {
                kernel: kernel.id.clone(),
                storage_bindings: kernel.bindings.clone(),
                parameter_block: kernel.parameter_block.clone(),
                logical_extent: kernel.logical_extent.clone(),
                outputs: kernel.outputs.clone(),
            }
        })
        .collect::<Vec<_>>();
    if package.manifest.binding_layouts != expected_layouts {
        return Err(backend_error(
            ErrorCode::ArtifactManifestInvalid,
            "artifact binding ABI differs from immutable BackendIR",
        ));
    }
    if !package.offline_validation.parsed || !package.offline_validation.validated {
        return Err(backend_error(
            ErrorCode::WgslValidationFailed,
            "artifact package lacks a successful offline WGSL validation report",
        ));
    }
    let modules = package
        .modules
        .iter()
        .map(|module| (module.id.clone(), module))
        .collect::<BTreeMap<_, _>>();
    if package.manifest.modules
        != package
            .modules
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>()
        || package
            .modules
            .iter()
            .any(|module| module.wgsl.is_empty() || module.wgsl.contains('\r'))
        || package.manifest.entry_points.iter().any(|entry| {
            modules.get(&entry.module).is_none_or(|module| {
                !module.entry_points.contains(&entry.name)
                    || !module.wgsl.contains(&format!("fn {}(", entry.name))
            }) || backend
                .program
                .kernels
                .get(&entry.kernel)
                .is_none_or(|kernel| {
                    kernel.entry_point != entry.name
                        || kernel.workgroup_size != entry.workgroup_size
                })
        })
    {
        return Err(backend_error(
            ErrorCode::ArtifactManifestInvalid,
            "WGSL modules, entry points, or manifest order are inconsistent",
        ));
    }
    if package.certificate.relation != "artifact_equivalent_to_backend"
        || package.certificate.backend_hash != backend.backend_hash
        || package.certificate.compiler_build_hash != package.manifest.compiler_build_hash
        || package.certificate.artifact_hash != package.artifact_hash
        || package.certificate.validator_version != ARTIFACT_VALIDATOR_VERSION
    {
        return Err(backend_error(
            ErrorCode::ArtifactEquivalenceUnproved,
            "ArtifactEquivalentToBackend certificate is missing or inconsistent",
        ));
    }
    let actual = artifact_hash(package)?;
    if actual != package.artifact_hash {
        return Err(backend_error(
            ErrorCode::ArtifactHashMismatch,
            "artifact_hash does not match exact manifest and WGSL bytes",
        )
        .with_types(package.artifact_hash.to_string(), actual.to_string()));
    }
    Ok(())
}

/// Artifact package summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactQuery {
    /// Artifact identity.
    pub artifact: ArtifactId,
    /// Exact package hash.
    pub artifact_hash: ArtifactHash,
    /// Source backend hash.
    pub backend_hash: BackendHash,
    /// Compiler build identity.
    pub compiler_build_hash: CompilerBuildHash,
    /// Lifecycle state.
    pub status: ArtifactStatus,
    /// Number of modules.
    pub module_count: usize,
    /// Number of entry points.
    pub entry_point_count: usize,
    /// Exact total WGSL bytes.
    pub wgsl_bytes: usize,
}

/// Artifact structural validation report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactCheckReport {
    /// Read-only summary.
    pub query: ArtifactQuery,
    /// Whether offline WGSL validation succeeded.
    pub offline_valid: bool,
    /// Whether compiler-owned proof establishes ArtifactEquivalentToBackend.
    pub equivalent_to_backend: bool,
}

/// Replayable artifact event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactEvent {
    /// Source backend plan.
    pub backend_plan: BackendPlanId,
    /// Source backend revision.
    pub backend_revision: BackendRevisionId,
    /// Exact emitted package.
    pub package: ArtifactPackage,
    /// Backend event dependency cursor.
    pub backend_event_cursor: u64,
}

/// Artifact event with independent replay semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedArtifactEvent {
    /// Artifact event semantics version.
    pub semantics_version: u32,
    /// Replayable event.
    pub event: ArtifactEvent,
}

/// Persistent deterministic artifact packages.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactStore {
    /// Packages by compiler identity.
    pub packages: BTreeMap<ArtifactId, ArtifactPackage>,
    /// Ordered artifact event log.
    pub events: Vec<VersionedArtifactEvent>,
}

impl ArtifactStore {
    /// Atomically emits one artifact through a trusted compiler component.
    pub fn emit_with<F>(
        &mut self,
        backend_store: &mut BackendStore,
        backend_plan: &BackendPlanId,
        backend_revision: &BackendRevisionId,
        emit: F,
    ) -> AgentResult<ArtifactCheckReport>
    where
        F: FnOnce(&mut BackendAllocator, ArtifactId) -> AgentResult<ArtifactPackage>,
    {
        backend_store.check(backend_plan, backend_revision)?;
        let backend = backend_store
            .revision(backend_plan, backend_revision)?
            .clone();
        let mut allocator = backend_store.allocator.clone();
        let artifact_id = allocator.artifact();
        let mut package = emit(&mut allocator, artifact_id.clone())?;
        if package.id != artifact_id {
            return Err(backend_error(
                ErrorCode::ArtifactManifestInvalid,
                "trusted emitter returned an unexpected artifact identity",
            ));
        }
        package.status = ArtifactStatus::Validated;
        package.artifact_hash = ArtifactHash::new("pending");
        package.certificate = ArtifactCertificate {
            relation: "artifact_equivalent_to_backend".to_owned(),
            backend_hash: backend.backend_hash.clone(),
            compiler_build_hash: package.manifest.compiler_build_hash.clone(),
            artifact_hash: ArtifactHash::new("pending"),
            conditions: vec![
                "backend_program_emitted_deterministically".to_owned(),
                "module_entry_point_and_abi_consistency_verified".to_owned(),
                "exact_wgsl_bytes_offline_validated".to_owned(),
            ],
            validator_version: ARTIFACT_VALIDATOR_VERSION,
        };
        package.artifact_hash = artifact_hash(&package)?;
        package.certificate.artifact_hash = package.artifact_hash.clone();
        verify_artifact(&package, &backend)?;
        let event = VersionedArtifactEvent {
            semantics_version: ARTIFACT_EVENT_SEMANTICS_VERSION,
            event: ArtifactEvent {
                backend_plan: backend_plan.clone(),
                backend_revision: backend_revision.clone(),
                package: package.clone(),
                backend_event_cursor: u64::try_from(backend_store.events.len()).unwrap_or(u64::MAX),
            },
        };
        self.packages.insert(artifact_id.clone(), package);
        self.events.push(event);
        backend_store.allocator = allocator;
        self.check(&artifact_id, &backend)
    }

    /// Returns artifact summaries in deterministic identity order.
    #[must_use]
    pub fn list(&self) -> Vec<ArtifactQuery> {
        self.packages.values().map(artifact_query).collect()
    }

    /// Returns one artifact package.
    pub fn package(&self, artifact: &ArtifactId) -> AgentResult<&ArtifactPackage> {
        self.packages.get(artifact).ok_or_else(|| {
            backend_error(
                ErrorCode::ArtifactNotFound,
                format!("artifact `{artifact}` does not exist"),
            )
        })
    }

    /// Reads one artifact summary.
    pub fn query(&self, artifact: &ArtifactId) -> AgentResult<ArtifactQuery> {
        Ok(artifact_query(self.package(artifact)?))
    }

    /// Fully validates one artifact against its immutable BackendIR revision.
    pub fn check(
        &self,
        artifact: &ArtifactId,
        backend: &BackendRevision,
    ) -> AgentResult<ArtifactCheckReport> {
        let package = self.package(artifact)?;
        verify_artifact(package, backend)?;
        Ok(ArtifactCheckReport {
            query: artifact_query(package),
            offline_valid: true,
            equivalent_to_backend: true,
        })
    }
}

fn artifact_query(package: &ArtifactPackage) -> ArtifactQuery {
    ArtifactQuery {
        artifact: package.id.clone(),
        artifact_hash: package.artifact_hash.clone(),
        backend_hash: package.manifest.backend_hash.clone(),
        compiler_build_hash: package.manifest.compiler_build_hash.clone(),
        status: package.status,
        module_count: package.modules.len(),
        entry_point_count: package.manifest.entry_points.len(),
        wgsl_bytes: package.modules.iter().map(|module| module.wgsl.len()).sum(),
    }
}

/// Computes a device fingerprint hash, separate from correctness and artifact hashes.
pub fn device_fingerprint_hash(
    fingerprint: &DeviceFingerprint,
) -> AgentResult<DeviceFingerprintHash> {
    let bytes = serde_json::to_vec(fingerprint).map_err(|error| {
        backend_error(
            ErrorCode::CanonicalizationFailed,
            format!("device fingerprint canonicalization failed: {error}"),
        )
    })?;
    Ok(DeviceFingerprintHash(digest(
        DEVICE_FINGERPRINT_HASH_DOMAIN,
        &bytes,
    )))
}

/// Computes a confidence-only hardware measurement hash.
pub fn measurement_hash(record: &HardwareMeasurementRecord) -> AgentResult<MeasurementHash> {
    #[derive(Serialize)]
    struct Model<'a> {
        format_version: u32,
        artifact_hash: &'a ArtifactHash,
        target_hash: &'a crate::target::TargetHash,
        compiler_build_hash: &'a CompilerBuildHash,
        device_fingerprint_hash: &'a DeviceFingerprintHash,
        device: &'a DeviceFingerprint,
        config: &'a crate::backend_ir::HardwareBenchmarkConfig,
        min_ns: u64,
        median_ns: u64,
        p95_ns: u64,
        max_ns: u64,
        guard_outcomes: &'a BTreeMap<String, u64>,
        validation_status: &'a str,
        runtime_version: &'a str,
    }
    let bytes = serde_json::to_vec(&Model {
        format_version: record.format_version,
        artifact_hash: &record.artifact_hash,
        target_hash: &record.target_hash,
        compiler_build_hash: &record.compiler_build_hash,
        device_fingerprint_hash: &record.device_fingerprint_hash,
        device: &record.device,
        config: &record.config,
        min_ns: record.min_ns,
        median_ns: record.median_ns,
        p95_ns: record.p95_ns,
        max_ns: record.max_ns,
        guard_outcomes: &record.guard_outcomes,
        validation_status: &record.validation_status,
        runtime_version: &record.runtime_version,
    })
    .map_err(|error| {
        backend_error(
            ErrorCode::CanonicalizationFailed,
            format!("measurement canonicalization failed: {error}"),
        )
    })?;
    Ok(MeasurementHash(digest(MEASUREMENT_HASH_DOMAIN, &bytes)))
}

/// Replayable completed measurement event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementEvent {
    /// Compiler-assigned measurement identity.
    pub measurement: MeasurementId,
    /// Exact completed record.
    pub record: HardwareMeasurementRecord,
    /// Artifact dependency cursor.
    pub artifact_event_cursor: u64,
}

/// Measurement event with independent replay semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedMeasurementEvent {
    /// Measurement event semantics version.
    pub semantics_version: u32,
    /// Replayable event.
    pub event: MeasurementEvent,
}

/// Persistent confidence-only hardware measurement records.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementStore {
    /// Completed records by compiler identity.
    pub records: BTreeMap<MeasurementId, HardwareMeasurementRecord>,
    /// Ordered measurement event log.
    pub events: Vec<VersionedMeasurementEvent>,
}

impl MeasurementStore {
    /// Publishes one runtime-created completed record after structural verification.
    pub fn publish(
        &mut self,
        allocator: &mut BackendAllocator,
        artifacts: &ArtifactStore,
        mut record: HardwareMeasurementRecord,
    ) -> AgentResult<MeasurementId> {
        if !artifacts
            .packages
            .values()
            .any(|package| package.artifact_hash == record.artifact_hash)
            || record.compiler_build_hash != compiler_build_hash()
            || device_fingerprint_hash(&record.device)? != record.device_fingerprint_hash
        {
            return Err(backend_error(
                ErrorCode::MeasurementEventOrderInvalid,
                "measurement provenance does not match a retained artifact/build/device",
            ));
        }
        record.measurement_hash = measurement_hash(&record)?;
        let id = allocator.measurement();
        self.records.insert(id.clone(), record.clone());
        self.events.push(VersionedMeasurementEvent {
            semantics_version: MEASUREMENT_EVENT_SEMANTICS_VERSION,
            event: MeasurementEvent {
                measurement: id.clone(),
                record,
                artifact_event_cursor: u64::try_from(artifacts.events.len()).unwrap_or(u64::MAX),
            },
        });
        Ok(id)
    }
}
