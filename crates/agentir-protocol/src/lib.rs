//! Transport-neutral JSON request engine for AgentIR.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod request;
pub mod response;

use agentir_core::{
    actions::{Action, Transaction},
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{RevisionId, WorkspaceId},
    workspace::Workspace,
};
use request::{QueryView, Request};
use response::Response;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Stateful in-memory request engine shared by CLI and future transports.
#[derive(Debug, Default)]
pub struct Engine {
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    next_workspace: u64,
}

impl Engine {
    /// Creates an empty protocol engine.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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
                let workspace = Workspace::new(id.clone())?;
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
                    "replay": loaded.replay,
                });
                self.workspaces.insert(workspace_id, loaded.workspace);
                Ok(result)
            }
            Request::WorkspaceVerifyArchive { path, .. } => {
                let (metadata, replay) = agentir_store::verify_archive(&path)?;
                Ok(json!({"metadata": metadata, "replay": replay}))
            }
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
                    QueryView::Summary => Ok(json!({
                        "workspace": workspace,
                        "revision": snapshot.id,
                        "parents": snapshot.parents,
                        "content_hash": snapshot.content_hash,
                        "status": snapshot.status,
                        "parameters": snapshot.program.parameters,
                        "outputs": snapshot.program.outputs,
                    })),
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
                serde_json::to_value(agentir_eval::evaluate(program, &inputs)?)
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
        }
    }

    /// Decodes one line and always returns exactly one response object.
    #[must_use]
    pub fn process_line(&mut self, line: &str) -> String {
        let parsed_json = serde_json::from_str::<Value>(line);
        let request_id = parsed_json
            .as_ref()
            .ok()
            .and_then(|value| value.get("request_id"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let response = match parsed_json.and_then(serde_json::from_value::<Request>) {
            Ok(request) => self.handle(request),
            Err(error) => Response::failure(
                request_id,
                AgentError::new(
                    ErrorCode::InvalidRequest,
                    format!("invalid JSONL request: {error}"),
                ),
            ),
        };
        response.to_json_line().unwrap_or_else(|error| {
            format!(
                "{{\"ok\":false,\"request_id\":\"unknown\",\"error\":{{\"code\":\"INVALID_REQUEST\",\"message\":{}}},\"diagnostics\":[]}}",
                serde_json::to_string(&error.message).unwrap_or_else(|_| "\"serialization failed\"".to_owned())
            )
        })
    }
}
