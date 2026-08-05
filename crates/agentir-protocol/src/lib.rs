//! Transport-neutral JSON request engine for AgentIR.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod request;
pub mod response;

use agentir_core::{
    actions::{Action, Transaction},
    diagnostics::{AgentError, AgentResult, ErrorCode},
    ids::{RevisionId, WorkspaceId},
    resources::{BudgetCheck, ResourceKind, ResourceLimits},
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
    limits: ResourceLimits,
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
