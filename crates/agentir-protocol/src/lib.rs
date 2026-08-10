//! Transport-neutral JSON request engine for AgentIR.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod request;
pub mod response;

use agentir_core::{
    actions::{Action, Transaction},
    candidate::{CandidateTransaction, SpeculativeRewriteProposal},
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{CandidateId, CandidateRevisionId, RevisionId, WorkspaceId},
    memory::MemoryTransaction,
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
    workspace::Workspace,
};
use request::{QueryView, Request};
use response::Response;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Instant;

fn runtime_inputs(
    values: &BTreeMap<String, Value>,
) -> AgentResult<BTreeMap<String, agentir_runtime_wgpu::RuntimeInput>> {
    values
        .iter()
        .map(|(name, value)| {
            let runtime = match value {
                Value::Number(number) if number.is_i64() => {
                    let value = number
                        .as_i64()
                        .and_then(|value| i32::try_from(value).ok())
                        .ok_or_else(|| {
                            AgentError::new(
                                ErrorCode::EvaluationInputMismatch,
                                format!("input `{name}` exceeds i32"),
                            )
                        })?;
                    agentir_runtime_wgpu::RuntimeInput::I32(value)
                }
                Value::Number(number) => {
                    let value = number.as_f64().ok_or_else(|| {
                        AgentError::new(
                            ErrorCode::EvaluationInputMismatch,
                            format!("input `{name}` is not finite"),
                        )
                    })? as f32;
                    agentir_runtime_wgpu::RuntimeInput::F32(value)
                }
                Value::Array(items) => {
                    let values = items
                        .iter()
                        .map(|item| {
                            item.as_f64().map(|value| value as f32).ok_or_else(|| {
                                AgentError::new(
                                    ErrorCode::EvaluationInputMismatch,
                                    format!("tensor input `{name}` must contain only numbers"),
                                )
                            })
                        })
                        .collect::<AgentResult<Vec<_>>>()?;
                    agentir_runtime_wgpu::RuntimeInput::F32Tensor(values)
                }
                _ => {
                    return Err(AgentError::new(
                        ErrorCode::EvaluationInputMismatch,
                        format!("input `{name}` is not a supported runtime scalar or tensor"),
                    ));
                }
            };
            Ok((name.clone(), runtime))
        })
        .collect()
}

fn check_runtime_limits(
    limits: &ResourceLimits,
    package: &agentir_core::backend_ir::ArtifactPackage,
    inputs: &BTreeMap<String, agentir_runtime_wgpu::RuntimeInput>,
) -> AgentResult<()> {
    let buffers = package
        .manifest
        .binding_layouts
        .iter()
        .flat_map(|layout| {
            layout
                .storage_bindings
                .iter()
                .map(|binding| &binding.buffer)
        })
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let elements = inputs.values().fold(0_u64, |total, input| {
        total.saturating_add(match input {
            agentir_runtime_wgpu::RuntimeInput::F32Tensor(values) => {
                u64::try_from(values.len()).unwrap_or(u64::MAX)
            }
            agentir_runtime_wgpu::RuntimeInput::F32(_)
            | agentir_runtime_wgpu::RuntimeInput::I32(_) => 1,
        })
    });
    for (kind, actual) in [
        (
            ResourceKind::ExecutionBuffers,
            u64::try_from(buffers).unwrap_or(u64::MAX),
        ),
        (ResourceKind::ExecutionElements, elements),
        (ResourceKind::ExecutionBytes, elements.saturating_mul(4)),
    ] {
        BudgetCheck::against(limits, kind, actual, "artifact device execution")?;
    }
    Ok(())
}

/// Stateful in-memory request engine shared by CLI and future transports.
#[derive(Debug, Default)]
pub struct Engine {
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    next_workspace: u64,
    limits: ResourceLimits,
    benchmark_tasks: BTreeMap<String, Value>,
    next_benchmark_task: u64,
}

impl Engine {
    /// Creates an empty protocol engine.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an engine with explicit interactive resource limits.
    #[must_use]
    pub fn with_limits(limits: ResourceLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    /// Returns the request byte limit used by bounded frontends.
    #[must_use]
    pub const fn max_request_bytes(&self) -> u64 {
        self.limits.jsonl_request_bytes
    }

    fn workspace(&self, id: &WorkspaceId) -> AgentResult<&Workspace> {
        self.workspaces.get(id).ok_or_else(|| {
            AgentError::new(
                ErrorCode::WorkspaceNotFound,
                format!("workspace `{id}` does not exist"),
            )
        })
    }

    fn workspace_mut(&mut self, id: &WorkspaceId) -> AgentResult<&mut Workspace> {
        self.workspaces.get_mut(id).ok_or_else(|| {
            AgentError::new(
                ErrorCode::WorkspaceNotFound,
                format!("workspace `{id}` does not exist"),
            )
        })
    }

    fn selected_revision(
        &self,
        workspace: &WorkspaceId,
        revision: Option<RevisionId>,
    ) -> AgentResult<RevisionId> {
        let workspace = self.workspace(workspace)?;
        Ok(revision.unwrap_or_else(|| workspace.head().clone()))
    }

    fn selected_candidate_revision(
        &self,
        workspace: &WorkspaceId,
        candidate: &CandidateId,
        revision: Option<CandidateRevisionId>,
    ) -> AgentResult<CandidateRevisionId> {
        let workspace = self.workspace(workspace)?;
        let revision = match revision {
            Some(revision) => revision,
            None => workspace.candidate_query(candidate)?.head.clone(),
        };
        workspace.candidate_revision(candidate, &revision)?;
        Ok(revision)
    }

    fn apply(
        &mut self,
        workspace: &WorkspaceId,
        base_revision: RevisionId,
        actions: Vec<Action>,
        client_transaction_id: Option<String>,
        allow_branch: bool,
    ) -> AgentResult<Value> {
        let transaction = Transaction {
            workspace: workspace.clone(),
            base_revision,
            actions,
            client_transaction_id,
            allow_branch,
        };
        serde_json::to_value(self.workspace_mut(workspace)?.apply(&transaction)?).map_err(|error| {
            AgentError::new(
                ErrorCode::TransactionRejected,
                format!("commit response serialization failed: {error}"),
            )
        })
    }

    /// Executes a decoded request against the in-memory workspace registry.
    pub fn handle(&mut self, request: Request) -> Response {
        let request_id = request.request_id().to_owned();
        let result = self.handle_inner(request);
        match result {
            Ok(value) => Response::success(request_id, value),
            Err(error) => Response::failure(request_id, error),
        }
    }

    fn handle_inner(&mut self, request: Request) -> AgentResult<Value> {
        match request {
            Request::WorkspaceOpen { workspace, .. } => {
                let id = workspace.unwrap_or_else(|| {
                    self.next_workspace += 1;
                    WorkspaceId::new(format!("w{}", self.next_workspace))
                });
                if self.workspaces.contains_key(&id) {
                    return Err(AgentError::new(
                        ErrorCode::DuplicateBinding,
                        format!("workspace `{id}` already exists"),
                    ));
                }
                let workspace = Workspace::with_limits(id.clone(), self.limits.clone())?;
                let head = workspace.head().clone();
                let hash = workspace.revision(&head)?.content_hash.clone();
                self.workspaces.insert(id.clone(), workspace);
                Ok(json!({"workspace": id, "revision": head, "content_hash": hash}))
            }
            Request::WorkspaceSave {
                workspace, path, ..
            } => serde_json::to_value(agentir_store::save_workspace(
                &path,
                self.workspace(&workspace)?,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::PersistenceFormat, error.to_string())),
            Request::WorkspaceLoad { path, replace, .. } => {
                let loaded = agentir_store::load_workspace(&path)?;
                let workspace_id = loaded.workspace.id().clone();
                if self.workspaces.contains_key(&workspace_id) && !replace {
                    return Err(AgentError::new(
                        ErrorCode::DuplicateBinding,
                        format!("workspace `{workspace_id}` is already open"),
                    ));
                }
                let result = json!({
                    "metadata": loaded.metadata,
                    "migration": loaded.migration,
                    "replay": loaded.replay,
                });
                let mut workspace = loaded.workspace;
                workspace.set_resource_limits(self.limits.clone());
                self.workspaces.insert(workspace_id, workspace);
                Ok(result)
            }
            Request::WorkspaceVerifyArchive { path, .. } => {
                let (metadata, replay) = agentir_store::verify_archive(&path)?;
                Ok(json!({"metadata": metadata, "replay": replay}))
            }
            Request::WorkspaceMigrateArchive {
                source_path,
                destination_path,
                overwrite,
                ..
            } => serde_json::to_value(agentir_store::migrate_archive(
                &source_path,
                &destination_path,
                overwrite,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::PersistenceFormat, error.to_string())),
            Request::SpecApply {
                workspace,
                base_revision,
                actions,
                client_transaction_id,
                allow_branch,
                ..
            }
            | Request::TransactionApply {
                workspace,
                base_revision,
                actions,
                client_transaction_id,
                allow_branch,
                ..
            } => self.apply(
                &workspace,
                base_revision,
                actions,
                client_transaction_id,
                allow_branch,
            ),
            Request::SpecCheck {
                workspace,
                revision,
                ..
            } => {
                let revision = self.selected_revision(&workspace, revision)?;
                let report = self.workspace(&workspace)?.check(&revision)?;
                serde_json::to_value(report)
                    .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::SpecFreeze {
                workspace,
                base_revision,
                client_transaction_id,
                ..
            } => self.apply(
                &workspace,
                base_revision,
                vec![Action::FreezeSpec],
                client_transaction_id,
                false,
            ),
            Request::ProgramQuery {
                workspace,
                revision,
                view,
                ..
            } => {
                let revision = self.selected_revision(&workspace, revision)?;
                let snapshot = self.workspace(&workspace)?.revision(&revision)?;
                match view {
                    QueryView::Canonical => serde_json::to_value(snapshot).map_err(|error| {
                        AgentError::new(ErrorCode::InvalidRequest, error.to_string())
                    }),
                    QueryView::SemanticCanonical => {
                        let canonical =
                            self.workspace(&workspace)?.semantic_canonical(&revision)?;
                        Ok(json!({
                            "semantic_canonical_version": canonical.canonical.version,
                            "canonical": canonical.canonical,
                            "canonical_byte_length": canonical.bytes.len(),
                            "spec_hash": canonical.spec_hash,
                        }))
                    }
                    QueryView::Summary => {
                        let mut summary = json!({
                            "workspace": workspace,
                            "revision": snapshot.id,
                            "parents": snapshot.parents,
                            "content_hash": snapshot.content_hash,
                            "status": snapshot.status,
                            "parameters": snapshot.program.parameters,
                            "outputs": snapshot.program.outputs,
                        });
                        let object = summary
                            .as_object_mut()
                            .expect("summary literal is a JSON object");
                        if let Some(spec_hash) = &snapshot.spec_hash {
                            object.insert("spec_hash".to_owned(), json!(spec_hash));
                        }
                        if let Some(version) = snapshot.semantic_canonical_version {
                            object.insert("semantic_canonical_version".to_owned(), json!(version));
                        }
                        Ok(summary)
                    }
                }
            }
            Request::ProgramEvaluate {
                workspace,
                revision,
                inputs,
                ..
            } => {
                let revision = self.selected_revision(&workspace, revision)?;
                let program = &self.workspace(&workspace)?.revision(&revision)?.program;
                serde_json::to_value(agentir_eval::evaluate_with_limits(
                    program,
                    &inputs,
                    &self.limits,
                )?)
                .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::RevisionFork {
                workspace,
                base_revision,
                ..
            } => {
                let revision = self.workspace_mut(&workspace)?.fork(&base_revision)?;
                Ok(json!({"workspace": workspace, "revision": revision, "parent": base_revision}))
            }
            Request::RevisionDiff {
                workspace,
                from,
                to,
                ..
            } => serde_json::to_value(self.workspace(&workspace)?.diff(&from, &to)?)
                .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::ContinuationGet {
                workspace,
                revision,
                hole,
                mode,
                ..
            } => serde_json::to_value(
                self.workspace_mut(&workspace)?
                    .continuation(&revision, &hole, mode)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::CandidateCreate {
                workspace,
                spec_revision,
                relation,
                ..
            } => serde_json::to_value(
                self.workspace_mut(&workspace)?
                    .candidate_create(&spec_revision, relation)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::CandidateQuery {
                workspace,
                candidate,
                candidate_revision,
                ..
            } => {
                if let Some(revision) = candidate_revision {
                    serde_json::to_value(
                        self.workspace(&workspace)?
                            .candidate_revision(&candidate, &revision)?,
                    )
                    .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
                } else {
                    serde_json::to_value(self.workspace(&workspace)?.candidate_query(&candidate)?)
                        .map_err(|error| {
                            AgentError::new(ErrorCode::InvalidRequest, error.to_string())
                        })
                }
            }
            Request::CandidateCheck {
                workspace,
                candidate,
                candidate_revision,
                ..
            } => {
                let revision =
                    self.selected_candidate_revision(&workspace, &candidate, candidate_revision)?;
                serde_json::to_value(
                    self.workspace(&workspace)?
                        .candidate_check(&candidate, &revision)?,
                )
                .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::CandidateApply {
                workspace,
                candidate,
                base_candidate_revision,
                actions,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.candidate_apply(
                &CandidateTransaction {
                    candidate,
                    base_revision: base_candidate_revision,
                    actions,
                },
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::CandidateFork {
                workspace,
                candidate,
                base_candidate_revision,
                ..
            } => serde_json::to_value(
                self.workspace_mut(&workspace)?
                    .candidate_fork(&candidate, &base_candidate_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::CandidateValidate {
                workspace,
                candidate,
                base_candidate_revision,
                seed,
                cases,
                ..
            } => {
                let validation = {
                    let workspace_data = self.workspace(&workspace)?;
                    let candidate_data = workspace_data.candidate_query(&candidate)?;
                    let spec = workspace_data
                        .revision(&candidate_data.spec_revision)?
                        .program
                        .clone();
                    workspace_data.candidate_revision(&candidate, &base_candidate_revision)?;
                    agentir_eval::differential_validate_candidate(
                        &spec,
                        workspace_data.candidate_forest(),
                        &candidate,
                        &base_candidate_revision,
                        seed,
                        cases,
                        &self.limits,
                    )?
                };
                serde_json::to_value(
                    self.workspace_mut(&workspace)?
                        .candidate_record_validation(
                            &candidate,
                            &base_candidate_revision,
                            validation,
                        )?,
                )
                .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::CandidateSeal {
                workspace,
                candidate,
                base_candidate_revision,
                ..
            } => serde_json::to_value(
                self.workspace_mut(&workspace)?
                    .candidate_seal(&candidate, &base_candidate_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::CandidateContinuation {
                workspace,
                candidate,
                candidate_revision,
                ..
            } => {
                let revision =
                    self.selected_candidate_revision(&workspace, &candidate, candidate_revision)?;
                serde_json::to_value(
                    self.workspace(&workspace)?
                        .candidate_continuation(&candidate, &revision)?,
                )
                .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::CandidatePropose {
                workspace,
                candidate,
                base_candidate_revision,
                target,
                replacement,
                expected_before_impl_hash,
                allow_speculative,
                claimed_rule,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.candidate_propose(
                &candidate,
                &base_candidate_revision,
                &SpeculativeRewriteProposal {
                    target,
                    replacement,
                    expected_before_impl_hash,
                    allow_speculative,
                    claimed_rule,
                },
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::CandidateProposalQuery {
                workspace,
                proposal,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .candidate_proposal_query(&proposal)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::CandidateTranslationCheck {
                workspace,
                candidate,
                base_candidate_revision,
                proposal,
                ..
            } => serde_json::to_value(
                self.workspace_mut(&workspace)?
                    .candidate_translation_check(&candidate, &base_candidate_revision, &proposal)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::CandidateEvaluate {
                workspace,
                candidate,
                candidate_revision,
                inputs,
                ..
            } => {
                let revision =
                    self.selected_candidate_revision(&workspace, &candidate, candidate_revision)?;
                serde_json::to_value(agentir_eval::evaluate_candidate_with_limits(
                    self.workspace(&workspace)?.candidate_forest(),
                    &candidate,
                    &revision,
                    &inputs,
                    &self.limits,
                )?)
                .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::EqualityCreate {
                workspace,
                candidate,
                candidate_revision,
                ..
            } => serde_json::to_value(
                self.workspace_mut(&workspace)?
                    .equality_create(&candidate, &candidate_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::EqualityQuery {
                workspace,
                equality_space,
                equality_revision,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .equality_query(&equality_space, &equality_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::EqualityExpand {
                workspace,
                equality_space,
                base_equality_revision,
                expected_equality_hash,
                fuel,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.equality_expand(
                &equality_space,
                &base_equality_revision,
                &expected_equality_hash,
                fuel,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::EqualitySaturate {
                workspace,
                equality_space,
                base_equality_revision,
                expected_equality_hash,
                fuel,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.equality_saturate(
                &equality_space,
                &base_equality_revision,
                &expected_equality_hash,
                fuel,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::EqualityExplain {
                workspace,
                equality_space,
                equality_revision,
                node,
                ..
            } => serde_json::to_value(self.workspace(&workspace)?.equality_explain(
                &equality_space,
                &equality_revision,
                &node,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::EqualityEvaluate {
                workspace,
                equality_space,
                equality_revision,
                node,
                inputs,
                ..
            } => {
                BudgetCheck::against(
                    &self.limits,
                    ResourceKind::EqualityEvaluationCases,
                    1,
                    "equality reference evaluation",
                )?;
                let mut evaluation_limits = self.limits.clone();
                evaluation_limits.total_evaluation_elements = evaluation_limits
                    .total_evaluation_elements
                    .min(evaluation_limits.equality_evaluation_elements);
                let program = self.workspace(&workspace)?.equality_node_program(
                    &equality_space,
                    &equality_revision,
                    &node,
                )?;
                serde_json::to_value(agentir_eval::evaluate_impl_with_limits(
                    program,
                    &inputs,
                    &evaluation_limits,
                )?)
                .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::EqualityMaterialize {
                workspace,
                equality_space,
                equality_revision,
                expected_equality_hash,
                node,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.equality_materialize(
                &equality_space,
                &equality_revision,
                &expected_equality_hash,
                &node,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::EqualityContinuation {
                workspace,
                equality_space,
                equality_revision,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .equality_continuation(&equality_space, &equality_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::CandidateEqualityCheck {
                workspace,
                candidate,
                base_candidate_revision,
                proposal,
                equality_space,
                equality_revision,
                expected_equality_hash,
                target_node,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.candidate_equality_check(
                &candidate,
                &base_candidate_revision,
                &proposal,
                &equality_space,
                &equality_revision,
                &expected_equality_hash,
                &target_node,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::MemoryCreate {
                workspace,
                candidate,
                candidate_revision,
                ..
            } => serde_json::to_value(
                self.workspace_mut(&workspace)?
                    .memory_create(&candidate, &candidate_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::MemoryQuery {
                workspace,
                memory_plan,
                memory_revision,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .memory_query(&memory_plan, &memory_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::MemoryCheck {
                workspace,
                memory_plan,
                memory_revision,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .memory_check(&memory_plan, &memory_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::MemoryApply {
                workspace,
                memory_plan,
                base_memory_revision,
                expected_memory_hash,
                expected_impl_hash,
                actions,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.memory_apply(
                &MemoryTransaction {
                    memory_plan,
                    base_memory_revision,
                    expected_memory_hash,
                    expected_impl_hash,
                    actions,
                },
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::MemoryFork {
                workspace,
                memory_plan,
                memory_revision,
                expected_memory_hash,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.memory_fork(
                &memory_plan,
                &memory_revision,
                &expected_memory_hash,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::MemorySeal {
                workspace,
                memory_plan,
                memory_revision,
                expected_memory_hash,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.memory_seal(
                &memory_plan,
                &memory_revision,
                &expected_memory_hash,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::MemoryEvaluate {
                workspace,
                memory_plan,
                memory_revision,
                inputs,
                guard_outcomes,
                ..
            } => {
                let workspace_data = self.workspace(&workspace)?;
                workspace_data.memory_check(&memory_plan, &memory_revision)?;
                serde_json::to_value(agentir_eval::evaluate_memory_with_limits(
                    workspace_data
                        .memory_store()
                        .revision(&memory_plan, &memory_revision)?,
                    workspace_data.memory_impl_program(&memory_plan)?,
                    &inputs,
                    &guard_outcomes,
                    &self.limits,
                )?)
                .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::MemoryAliasQuery {
                workspace,
                memory_plan,
                memory_revision,
                first,
                second,
                ..
            } => serde_json::to_value(self.workspace(&workspace)?.memory_alias_query(
                &memory_plan,
                &memory_revision,
                &first,
                &second,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::MemoryBufferQuery {
                workspace,
                memory_plan,
                memory_revision,
                buffer,
                ..
            } => serde_json::to_value(self.workspace(&workspace)?.memory_buffer_query(
                &memory_plan,
                &memory_revision,
                &buffer,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::MemoryContinuation {
                workspace,
                memory_plan,
                memory_revision,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .memory_continuation(&memory_plan, &memory_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::TargetList { workspace, .. } => {
                serde_json::to_value(self.workspace(&workspace)?.target_list())
                    .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::TargetCreate {
                workspace, profile, ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.target_create(profile)?)
                .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::TargetQuery {
                workspace,
                target_manifest,
                target_revision,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .target_query(&target_manifest, &target_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::TargetCheck {
                workspace,
                target_manifest,
                target_revision,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .target_check(&target_manifest, &target_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::ScheduleCreate {
                workspace,
                memory_plan,
                memory_revision,
                target_manifest,
                target_revision,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.schedule_create(
                &memory_plan,
                &memory_revision,
                &target_manifest,
                &target_revision,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::ScheduleQuery {
                workspace,
                schedule_plan,
                schedule_revision,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .schedule_query(&schedule_plan, &schedule_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::ScheduleCheck {
                workspace,
                schedule_plan,
                schedule_revision,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .schedule_check(&schedule_plan, &schedule_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::ScheduleApply {
                workspace,
                schedule_plan,
                base_schedule_revision,
                expected_schedule_hash,
                expected_memory_hash,
                expected_target_hash,
                actions,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.schedule_apply(
                &agentir_core::schedule::ScheduleTransaction {
                    schedule_plan,
                    base_schedule_revision,
                    expected_schedule_hash,
                    expected_memory_hash,
                    expected_target_hash,
                    actions,
                },
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::ScheduleFork {
                workspace,
                schedule_plan,
                schedule_revision,
                expected_schedule_hash,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.schedule_fork(
                &schedule_plan,
                &schedule_revision,
                &expected_schedule_hash,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::ScheduleSeal {
                workspace,
                schedule_plan,
                schedule_revision,
                expected_schedule_hash,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.schedule_seal(
                &schedule_plan,
                &schedule_revision,
                &expected_schedule_hash,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::ScheduleEvaluate {
                workspace,
                schedule_plan,
                schedule_revision,
                inputs,
                guard_outcomes,
                ..
            } => {
                let workspace_data = self.workspace(&workspace)?;
                workspace_data.schedule_check(&schedule_plan, &schedule_revision)?;
                serde_json::to_value(agentir_eval::evaluate_schedule_with_limits(
                    workspace_data
                        .schedule_store()
                        .revision(&schedule_plan, &schedule_revision)?,
                    workspace_data.scheduled_memory_revision(&schedule_plan)?,
                    workspace_data.scheduled_impl_program(&schedule_plan)?,
                    &inputs,
                    &guard_outcomes,
                    &self.limits,
                )?)
                .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::ScheduleResourceQuery {
                workspace,
                schedule_plan,
                schedule_revision,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .schedule_resource_query(&schedule_plan, &schedule_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::ScheduleAxisQuery {
                workspace,
                schedule_plan,
                schedule_revision,
                axis,
                ..
            } => serde_json::to_value(self.workspace(&workspace)?.schedule_axis_query(
                &schedule_plan,
                &schedule_revision,
                &axis,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::ScheduleLegalityQuery {
                workspace,
                schedule_plan,
                schedule_revision,
                action,
                ..
            } => serde_json::to_value(self.workspace(&workspace)?.schedule_legality_query(
                &schedule_plan,
                &schedule_revision,
                &action,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::ScheduleContinuation {
                workspace,
                schedule_plan,
                schedule_revision,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .schedule_continuation(&schedule_plan, &schedule_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::BackendLower {
                workspace,
                schedule_plan,
                schedule_revision,
                expected_schedule_hash,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.backend_lower_with(
                &schedule_plan,
                &schedule_revision,
                &expected_schedule_hash,
                agentir_backend_wgsl::lower_schedule,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::BackendQuery {
                workspace,
                backend_plan,
                backend_revision,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .backend_query(&backend_plan, &backend_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::BackendCheck {
                workspace,
                backend_plan,
                backend_revision,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .backend_check(&backend_plan, &backend_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::BackendContinuation {
                workspace,
                backend_plan,
                backend_revision,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .backend_continuation(&backend_plan, &backend_revision)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::BackendFork {
                workspace,
                backend_plan,
                backend_revision,
                expected_backend_hash,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.backend_fork(
                &backend_plan,
                &backend_revision,
                &expected_backend_hash,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::BackendSeal {
                workspace,
                backend_plan,
                backend_revision,
                expected_backend_hash,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.backend_seal(
                &backend_plan,
                &backend_revision,
                &expected_backend_hash,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::ArtifactEmit {
                workspace,
                backend_plan,
                backend_revision,
                expected_backend_hash,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.artifact_emit_with(
                &backend_plan,
                &backend_revision,
                &expected_backend_hash,
                agentir_backend_wgsl::emit_artifact,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::ArtifactList { workspace, .. } => {
                serde_json::to_value(self.workspace(&workspace)?.artifact_list())
                    .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::ArtifactQuery {
                workspace,
                artifact,
                ..
            } => serde_json::to_value(self.workspace(&workspace)?.artifact_query(&artifact)?)
                .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::ArtifactCheck {
                workspace,
                artifact,
                expected_artifact_hash,
                ..
            } => {
                let data = self.workspace(&workspace)?.artifact_package(&artifact)?;
                if data.artifact_hash != expected_artifact_hash {
                    return Err(AgentError::new(
                        ErrorCode::ArtifactHashMismatch,
                        "artifact.check expected hash differs from the retained package",
                    )
                    .with_types(
                        expected_artifact_hash.to_string(),
                        data.artifact_hash.to_string(),
                    ));
                }
                serde_json::to_value(self.workspace(&workspace)?.artifact_check(&artifact)?)
                    .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::ArtifactReferenceEvaluate {
                workspace,
                artifact,
                inputs,
                guard_outcomes,
                ..
            } => {
                let workspace_data = self.workspace(&workspace)?;
                workspace_data.artifact_check(&artifact)?;
                let (backend_plan, _) = workspace_data.artifact_source_backend(&artifact)?;
                let (schedule_plan, schedule_revision) =
                    workspace_data.backend_source_schedule(backend_plan)?;
                let evaluation = agentir_eval::evaluate_schedule_with_limits(
                    workspace_data
                        .schedule_store()
                        .revision(schedule_plan, schedule_revision)?,
                    workspace_data.scheduled_memory_revision(schedule_plan)?,
                    workspace_data.scheduled_impl_program(schedule_plan)?,
                    &inputs,
                    &guard_outcomes,
                    &self.limits,
                )?;
                let package = workspace_data.artifact_package(&artifact)?;
                let guard_branch = package
                    .manifest
                    .guard
                    .as_ref()
                    .map(|_| guard_outcomes.values().copied().next().unwrap_or(false));
                let selected_orders = package.manifest.guard.as_ref().map(|guard| {
                    if guard_branch == Some(true) {
                        &guard.true_dispatches
                    } else {
                        &guard.false_dispatches
                    }
                });
                let mut events = Vec::new();
                let push_event =
                    |events: &mut Vec<agentir_core::backend_ir::ArtifactTraceEvent>,
                     kind: &str,
                     detail: String| {
                        events.push(agentir_core::backend_ir::ArtifactTraceEvent {
                            sequence: u64::try_from(events.len()).unwrap_or(u64::MAX),
                            kind: kind.to_owned(),
                            detail,
                        });
                    };
                if let Some(branch) = guard_branch {
                    push_event(&mut events, "guard", format!("no_overlap={branch}"));
                }
                for layout in &package.manifest.binding_layouts {
                    for binding in &layout.storage_bindings {
                        push_event(
                            &mut events,
                            "binding",
                            format!(
                                "{}:group={}:binding={}:buffer={}:access={:?}:offset={}",
                                layout.kernel,
                                binding.group,
                                binding.binding,
                                binding.buffer,
                                binding.access,
                                binding.offset_elements
                            ),
                        );
                    }
                }
                for dispatch in &package.manifest.dispatches {
                    if selected_orders.is_some_and(|orders| !orders.contains(&dispatch.order)) {
                        continue;
                    }
                    push_event(
                        &mut events,
                        "dispatch",
                        format!(
                            "{}:{}:grid={:?}:workgroup={:?}:bounds_checked={}",
                            dispatch.order,
                            dispatch.kernel,
                            dispatch.workgroups,
                            dispatch.workgroup_size,
                            dispatch.bounds_checked
                        ),
                    );
                }
                for output in &package.manifest.outputs {
                    push_event(
                        &mut events,
                        "output",
                        format!(
                            "{}:binding={}:buffer={}",
                            output.name, output.binding, output.buffer
                        ),
                    );
                }
                let trace = agentir_core::backend_ir::ArtifactTrace {
                    trace_codec_version: agentir_core::backend_ir::ARTIFACT_TRACE_CODEC_VERSION,
                    artifact,
                    guard_branch,
                    events,
                };
                Ok(json!({"evaluation": evaluation, "trace": trace}))
            }
            Request::ArtifactExecute {
                workspace,
                artifact,
                expected_artifact_hash,
                adapter,
                inputs,
                ..
            } => {
                let workspace_data = self.workspace(&workspace)?;
                workspace_data.artifact_check(&artifact)?;
                let package = workspace_data.artifact_package(&artifact)?;
                if package.artifact_hash != expected_artifact_hash {
                    return Err(AgentError::new(
                        ErrorCode::ArtifactHashMismatch,
                        "artifact.execute expected hash differs from the retained package",
                    ));
                }
                let target = workspace_data.target_manifest(
                    &package.manifest.anchor.target_manifest,
                    &package.manifest.anchor.target_revision,
                )?;
                let runtime = runtime_inputs(&inputs)?;
                check_runtime_limits(&self.limits, package, &runtime)?;
                let record = agentir_runtime_wgpu::execute(package, target, adapter, &runtime)?;
                serde_json::to_value(record)
                    .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::CpuArtifactEmit {
                workspace,
                schedule_plan,
                schedule_revision,
                expected_schedule_hash,
                ..
            } => serde_json::to_value(self.workspace_mut(&workspace)?.cpu_artifact_emit_with(
                &schedule_plan,
                &schedule_revision,
                &expected_schedule_hash,
                agentir_backend_cpu::lower_schedule,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::CpuArtifactList { workspace, .. } => {
                serde_json::to_value(self.workspace(&workspace)?.cpu_artifact_list()?)
                    .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::CpuArtifactQuery {
                workspace,
                cpu_artifact,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .cpu_artifact_query(&cpu_artifact)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::CpuArtifactCheck {
                workspace,
                cpu_artifact,
                expected_cpu_artifact_hash,
                ..
            } => {
                let package = self
                    .workspace(&workspace)?
                    .cpu_artifact_package(&cpu_artifact)?;
                if package.cpu_artifact_hash != expected_cpu_artifact_hash {
                    return Err(AgentError::new(
                        ErrorCode::CpuArtifactHashMismatch,
                        "cpu_artifact.check expected hash differs from the retained package",
                    )
                    .with_types(
                        expected_cpu_artifact_hash.to_string(),
                        package.cpu_artifact_hash.to_string(),
                    ));
                }
                serde_json::to_value(
                    self.workspace(&workspace)?
                        .cpu_artifact_check(&cpu_artifact)?,
                )
                .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::CpuArtifactExecute {
                workspace,
                cpu_artifact,
                expected_cpu_artifact_hash,
                inputs,
                ..
            } => {
                let workspace = self.workspace(&workspace)?;
                workspace.cpu_artifact_check(&cpu_artifact)?;
                let package = workspace.cpu_artifact_package(&cpu_artifact)?;
                if package.cpu_artifact_hash != expected_cpu_artifact_hash {
                    return Err(AgentError::new(
                        ErrorCode::CpuArtifactHashMismatch,
                        "cpu_artifact.execute expected hash differs from the retained package",
                    )
                    .with_types(
                        expected_cpu_artifact_hash.to_string(),
                        package.cpu_artifact_hash.to_string(),
                    ));
                }
                serde_json::to_value(agentir_backend_cpu::execute(
                    package,
                    &inputs,
                    &self.limits,
                )?)
                .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::CpuMeasurementAcquire {
                workspace,
                cpu_artifact,
                expected_cpu_artifact_hash,
                config,
                inputs,
                ..
            } => {
                agentir_core::cpu_measurement::validate_cpu_benchmark_config(
                    &config,
                    &self.limits,
                )?;
                let package = {
                    let retained = self.workspace(&workspace)?;
                    retained.cpu_artifact_check(&cpu_artifact)?;
                    let package = retained.cpu_artifact_package(&cpu_artifact)?;
                    if package.cpu_artifact_hash != expected_cpu_artifact_hash {
                        return Err(AgentError::new(ErrorCode::CpuArtifactHashMismatch, "cpu_measurement.acquire expected hash differs from the retained package")
                            .with_types(expected_cpu_artifact_hash.to_string(), package.cpu_artifact_hash.to_string()));
                    }
                    package.clone()
                };
                let draft = agentir_runtime_cpu::acquire(&package, config, &inputs, &self.limits)?;
                serde_json::to_value(
                    self.workspace_mut(&workspace)?
                        .cpu_measurement_publish(draft)?,
                )
                .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::CpuMeasurementList { workspace, .. } => {
                serde_json::to_value(self.workspace(&workspace)?.cpu_measurement_list())
                    .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::CpuMeasurementQuery {
                workspace,
                cpu_measurement,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .cpu_measurement_query(&cpu_measurement)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::CpuMeasurementCheck {
                workspace,
                cpu_measurement,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .cpu_measurement_check(&cpu_measurement)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::DeviceList {
                workspace,
                target_manifest,
                target_revision,
                ..
            } => serde_json::to_value(agentir_runtime_wgpu::list_devices(
                self.workspace(&workspace)?
                    .target_manifest(&target_manifest, &target_revision)?,
            )?)
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
            Request::DeviceQuery {
                workspace,
                target_manifest,
                target_revision,
                adapter,
                ..
            } => {
                let devices = agentir_runtime_wgpu::list_devices(
                    self.workspace(&workspace)?
                        .target_manifest(&target_manifest, &target_revision)?,
                )?;
                let record = devices
                    .into_iter()
                    .find(|record| record.index == adapter)
                    .ok_or_else(|| {
                        AgentError::new(
                            ErrorCode::DeviceUnavailable,
                            "WebGPU adapter is unavailable",
                        )
                    })?;
                serde_json::to_value(record)
                    .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string()))
            }
            Request::BenchmarkStart {
                workspace,
                artifact,
                expected_artifact_hash,
                adapter,
                config,
                inputs,
                ..
            } => {
                BudgetCheck::against(
                    &self.limits,
                    ResourceKind::ActiveDeviceTasks,
                    1,
                    "benchmark.start",
                )?;
                BudgetCheck::against(
                    &self.limits,
                    ResourceKind::BenchmarkWarmups,
                    u64::from(config.warmups),
                    "benchmark.start",
                )?;
                BudgetCheck::against(
                    &self.limits,
                    ResourceKind::BenchmarkIterations,
                    u64::from(config.iterations),
                    "benchmark.start",
                )?;
                if config.iterations == 0 {
                    return Err(AgentError::new(
                        ErrorCode::BenchmarkLimitExceeded,
                        "benchmark iterations must be positive",
                    ));
                }
                let runtime = runtime_inputs(&inputs)?;
                let (package, target) = {
                    let workspace_data = self.workspace(&workspace)?;
                    workspace_data.artifact_check(&artifact)?;
                    let package = workspace_data.artifact_package(&artifact)?.clone();
                    if package.artifact_hash != expected_artifact_hash {
                        return Err(AgentError::new(
                            ErrorCode::ArtifactHashMismatch,
                            "benchmark.start expected hash differs from the retained package",
                        ));
                    }
                    let target = workspace_data
                        .target_manifest(
                            &package.manifest.anchor.target_manifest,
                            &package.manifest.anchor.target_revision,
                        )?
                        .clone();
                    (package, target)
                };
                check_runtime_limits(&self.limits, &package, &runtime)?;
                for _ in 0..config.warmups {
                    agentir_runtime_wgpu::execute(&package, &target, adapter, &runtime)?;
                }
                let started = Instant::now();
                let mut samples = Vec::with_capacity(config.iterations as usize);
                let mut last = None;
                let mut guard_outcomes = BTreeMap::<String, u64>::new();
                for _ in 0..config.iterations {
                    let iteration = Instant::now();
                    let result =
                        agentir_runtime_wgpu::execute(&package, &target, adapter, &runtime)?;
                    samples.push(u64::try_from(iteration.elapsed().as_nanos()).unwrap_or(u64::MAX));
                    let branch = match result.guard_branch {
                        Some(true) => "true",
                        Some(false) => "false",
                        None => "unguarded",
                    };
                    *guard_outcomes.entry(branch.to_owned()).or_default() += 1;
                    last = Some(result);
                    BudgetCheck::against(
                        &self.limits,
                        ResourceKind::BenchmarkWallTimeMs,
                        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                        "benchmark.start",
                    )?;
                }
                samples.sort_unstable();
                let last = last.expect("positive iteration count checked above");
                let percentile = |numerator: usize| {
                    let index = samples.len().saturating_mul(numerator).saturating_add(99) / 100;
                    samples[index.saturating_sub(1).min(samples.len() - 1)]
                };
                let record = agentir_core::backend_ir::HardwareMeasurementRecord {
                    format_version: agentir_core::backend_ir::MEASUREMENT_FORMAT_VERSION,
                    artifact_hash: package.artifact_hash.clone(),
                    target_hash: package.manifest.anchor.target_hash.clone(),
                    compiler_build_hash: package.manifest.compiler_build_hash.clone(),
                    device_fingerprint_hash: last.device_fingerprint_hash,
                    device: last.device,
                    config,
                    min_ns: samples[0],
                    median_ns: percentile(50),
                    p95_ns: percentile(95),
                    max_ns: samples[samples.len() - 1],
                    guard_outcomes,
                    validation_status: "offline_validated_and_device_executed".to_owned(),
                    runtime_version: agentir_runtime_wgpu::WGPU_RUNTIME_VERSION.to_owned(),
                    measurement_hash: agentir_core::backend::MeasurementHash::new("pending"),
                };
                BudgetCheck::against(
                    &self.limits,
                    ResourceKind::BenchmarkRecordBytes,
                    u64::try_from(
                        serde_json::to_vec(&record)
                            .map_err(|error| {
                                AgentError::new(
                                    ErrorCode::CanonicalizationFailed,
                                    error.to_string(),
                                )
                            })?
                            .len(),
                    )
                    .unwrap_or(u64::MAX),
                    "benchmark record",
                )?;
                let measurement = self
                    .workspace_mut(&workspace)?
                    .measurement_publish(record)?;
                self.next_benchmark_task = self.next_benchmark_task.saturating_add(1);
                let task = format!("bench{}", self.next_benchmark_task);
                let value = json!({
                    "task": task,
                    "status": "completed",
                    "measurement": measurement,
                });
                self.benchmark_tasks.insert(task, value.clone());
                Ok(value)
            }
            Request::BenchmarkStatus { task, .. } => {
                self.benchmark_tasks.get(&task).cloned().ok_or_else(|| {
                    AgentError::new(
                        ErrorCode::BenchmarkTaskNotFound,
                        format!("benchmark task `{task}` does not exist"),
                    )
                })
            }
            Request::BenchmarkCancel { task, .. } => {
                let Some(state) = self.benchmark_tasks.get(&task) else {
                    return Err(AgentError::new(
                        ErrorCode::BenchmarkTaskNotFound,
                        format!("benchmark task `{task}` does not exist"),
                    ));
                };
                Ok(json!({"task": task, "status": "already_completed", "result": state}))
            }
            Request::BenchmarkQuery {
                workspace,
                measurement,
                ..
            } => serde_json::to_value(
                self.workspace(&workspace)?
                    .measurement_query(&measurement)?,
            )
            .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, error.to_string())),
        }
    }

    /// Decodes one UTF-8 line and always returns exactly one response object.
    #[must_use]
    pub fn process_line(&mut self, line: &str) -> String {
        self.process_bytes(line.as_bytes())
    }

    /// Decodes one bounded byte line, including invalid UTF-8, into one response.
    #[must_use]
    pub fn process_bytes(&mut self, line: &[u8]) -> String {
        let request_id = extract_request_id(line).unwrap_or_else(|| "unknown".to_owned());
        let byte_check = BudgetCheck::against(
            &self.limits,
            ResourceKind::JsonlRequestBytes,
            u64::try_from(line.len()).unwrap_or(u64::MAX),
            "JSONL request before parse",
        );
        if let Err(error) = byte_check {
            return self.serialize_response(&Response::failure(request_id, error));
        }
        let line = match std::str::from_utf8(line) {
            Ok(line) => line,
            Err(error) => {
                return self.serialize_response(&Response::failure(
                    request_id,
                    AgentError::new(
                        ErrorCode::InvalidRequest,
                        format!("invalid UTF-8 in JSONL request: {error}"),
                    ),
                ));
            }
        };
        if let Err(error) = check_json_depth(line.as_bytes(), &self.limits) {
            return self.serialize_response(&Response::failure(request_id, error));
        }
        let response = match serde_json::from_str::<Request>(line) {
            Ok(request) => self.handle(request),
            Err(error) => Response::failure(
                request_id,
                AgentError::new(
                    ErrorCode::InvalidRequest,
                    format!("invalid JSONL request: {error}"),
                ),
            ),
        };
        self.serialize_response(&response)
    }

    /// Creates the single response for a line discarded by a bounded reader.
    #[must_use]
    pub fn oversized_line_response(&self, retained_prefix: &[u8], attempted: u64) -> String {
        let request_id =
            extract_request_id(retained_prefix).unwrap_or_else(|| "unknown".to_owned());
        let error = BudgetCheck::against(
            &self.limits,
            ResourceKind::JsonlRequestBytes,
            attempted,
            "bounded JSONL line reader",
        )
        .expect_err("oversized line exceeds configured limit");
        self.serialize_response(&Response::failure(request_id, error))
    }

    fn serialize_response(&self, response: &Response) -> String {
        let serialized = response.to_json_line();
        let serialized = serialized.and_then(|line| {
            BudgetCheck::against(
                &self.limits,
                ResourceKind::CanonicalOutputBytes,
                u64::try_from(line.len()).unwrap_or(u64::MAX),
                "protocol response encoding",
            )?;
            Ok(line)
        });
        serialized.unwrap_or_else(|error| {
            let fallback = Response::failure("unknown", error);
            fallback.to_json_line().unwrap_or_else(|error| {
            format!(
                "{{\"ok\":false,\"request_id\":\"unknown\",\"error\":{{\"code\":\"INVALID_REQUEST\",\"message\":{}}},\"diagnostics\":[]}}",
                serde_json::to_string(&error.message).unwrap_or_else(|_| "\"serialization failed\"".to_owned())
            )
            })
        })
    }
}

fn check_json_depth(bytes: &[u8], limits: &ResourceLimits) -> AgentResult<()> {
    let mut depth = 0_u64;
    let mut maximum = 0_u64;
    let mut in_string = false;
    let mut escaped = false;
    for byte in bytes {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                maximum = maximum.max(depth);
                BudgetCheck::against(
                    limits,
                    ResourceKind::JsonNestingDepth,
                    maximum,
                    "JSON structural scan before parse",
                )?;
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

fn extract_request_id(bytes: &[u8]) -> Option<String> {
    let valid_length =
        std::str::from_utf8(bytes).map_or_else(|error| error.valid_up_to(), str::len);
    let text = std::str::from_utf8(&bytes[..valid_length]).ok()?;
    let mut offset = 0;
    while let Some(relative_start) = text[offset..].find('"') {
        let start = offset + relative_start;
        let mut escaped = false;
        let mut end = None;
        for (relative, character) in text[start + 1..].char_indices() {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                end = Some(start + 1 + relative);
                break;
            }
        }
        let end = end?;
        let token = serde_json::from_str::<String>(&text[start..=end]).ok()?;
        offset = end + 1;
        if token != "request_id" {
            continue;
        }
        let colon = text[offset..].find(':')? + offset;
        let value_start = text[colon + 1..].find('"')? + colon + 1;
        let mut escaped = false;
        for (relative, character) in text[value_start + 1..].char_indices() {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                let value_end = value_start + 1 + relative;
                return serde_json::from_str::<String>(&text[value_start..=value_end]).ok();
            }
        }
        return None;
    }
    None
}
