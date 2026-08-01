use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::agent::AgentState;
use crate::session::{collect_leaf_pane_ids, ResumeTabState, SshWriteAck, SyncMsg};
use crate::ssh::SshProfileConnectRequest;
use crate::tabs::{create_ssh_profile_tab, CreateSshTabError};
use crate::token::TokenInfo;

const RESUME_CAPABILITY: &str = "terminal:resume";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeSessionRequest {
    pub profile_id: String,
    pub session_id: String,
    #[serde(default)]
    pub initial_cwd: Option<String>,
    #[serde(default)]
    pub execute: bool,
}

#[derive(Debug, Serialize)]
pub struct ResumeSessionResponse {
    pub ok: bool,
    pub action: &'static str,
    pub tab_id: String,
    pub pane_id: String,
    pub profile_id: String,
    pub session_id: String,
    pub command: String,
    pub executed: bool,
    pub delivery: &'static str,
}

struct ResumeError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

pub async fn resume_session(
    axum::extract::State(state): axum::extract::State<AgentState>,
    axum::Extension(token_info): axum::Extension<TokenInfo>,
    Json(req): Json<ResumeSessionRequest>,
) -> axum::response::Response {
    let profile_id = req.profile_id.trim().to_string();
    let session_id = req.session_id.trim().to_string();
    match resume_session_inner(&state, &token_info, req).await {
        Ok((status, response)) => {
            state.audit.record(
                &token_info.token_id,
                "terminal:resume",
                &profile_id,
                serde_json::json!({
                    "session_id": session_id,
                    "pane_id": response.pane_id,
                    "action": response.action,
                    "executed": response.executed,
                    "delivery": response.delivery,
                    "ok": true,
                }),
            );
            (status, Json(response)).into_response()
        }
        Err(error) => {
            state.audit.record(
                &token_info.token_id,
                "terminal:resume",
                if profile_id.is_empty() { "unknown" } else { &profile_id },
                serde_json::json!({
                    "session_id": session_id,
                    "code": error.code,
                    "message": &error.message,
                    "ok": false,
                }),
            );
            error.into_response()
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn resume_session_inner(
    state: &AgentState,
    token_info: &TokenInfo,
    req: ResumeSessionRequest,
) -> Result<(StatusCode, ResumeSessionResponse), ResumeError> {
    {
        let settings = state.settings.read().await;
        if !settings.open_api.enabled {
            return Err(resume_error(
                StatusCode::FORBIDDEN,
                "CAPABILITY_DENIED",
                "Agent API is disabled",
            ));
        }
    }

    let profile_id = req.profile_id.trim();
    if !valid_identifier(profile_id) {
        return Err(resume_error(StatusCode::BAD_REQUEST, "INVALID_REQUEST", "Invalid profile_id"));
    }
    if !token_info.check_required_scope(RESUME_CAPABILITY, profile_id) {
        return Err(resume_error(
            StatusCode::FORBIDDEN,
            "CAPABILITY_DENIED",
            "Token requires an explicit scope for this SSH profile",
        ));
    }

    let session_id = req.session_id.trim();
    if !valid_identifier(session_id) {
        return Err(resume_error(StatusCode::BAD_REQUEST, "INVALID_REQUEST", "Invalid session_id"));
    }
    let initial_cwd = normalize_cwd(req.initial_cwd.as_deref())
        .map_err(|message| resume_error(StatusCode::BAD_REQUEST, "INVALID_REQUEST", message))?;

    {
        let settings = state.settings.read().await;
        let profile = settings
            .ssh_profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| {
                resume_error(StatusCode::NOT_FOUND, "PROFILE_NOT_FOUND", "SSH profile not found")
            })?;
        if profile.default_command.as_deref().is_some_and(|command| !command.trim().is_empty()) {
            return Err(resume_error(
                StatusCode::CONFLICT,
                "PROFILE_NOT_INTERACTIVE",
                "SSH profile uses a default command and cannot host a resume shell",
            ));
        }
    }

    let key = resume_key(profile_id, session_id);
    prune_resume_tabs(state);
    let lock_index = resume_lock_index(&key, state.manager.resume_locks.len());
    let _guard = state.manager.resume_locks[lock_index].lock().await;
    prune_resume_tabs(state);
    let mut recreated = false;

    if let Some(existing) = state.manager.resume_tabs.get(&key).map(|entry| entry.value().clone()) {
        if resume_tab_exists(state, &existing) {
            if existing.executed || !req.execute {
                if !activate_tab(state, &existing.tab_id, &existing.pane_id) {
                    state.manager.resume_tabs.remove(&key);
                    return Err(resume_error(
                        StatusCode::CONFLICT,
                        "RESUME_TAB_CLOSED",
                        "Resume tab closed while it was being activated",
                    ));
                }
                return Ok((
                    StatusCode::OK,
                    response_for(
                        "reused",
                        existing,
                        profile_id,
                        session_id,
                        resume_command(session_id),
                        "confirmed",
                    ),
                ));
            }
            close_resume_tab(state, &existing);
            recreated = true;
        }
        state.manager.resume_tabs.remove(&key);
    }

    let capacity_permit = Arc::new(
        tokio::time::timeout(
            std::time::Duration::from_millis(250),
            state.manager.resume_capacity.clone().acquire_owned(),
        )
        .await
        .map_err(|_| {
            resume_error(
                StatusCode::TOO_MANY_REQUESTS,
                "RESUME_TAB_LIMIT",
                "Too many active resume tabs",
            )
        })?
        .map_err(|_| {
            resume_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "RESUME_CAPACITY_CLOSED",
                "Resume capacity manager is unavailable",
            )
        })?,
    );
    let action = if recreated { "recreated" } else { "created" };
    let connect = SshProfileConnectRequest { profile_id: profile_id.to_string(), initial_cwd };
    let created = create_ssh_profile_tab(&state.manager, &state.settings, &connect).await.map_err(
        |error| match error {
            CreateSshTabError::ProfileNotFound => {
                resume_error(StatusCode::NOT_FOUND, "PROFILE_NOT_FOUND", "SSH profile not found")
            }
            CreateSshTabError::Connection(message) => resume_error(
                StatusCode::BAD_GATEWAY,
                "SSH_CONNECT_FAILED",
                format!("SSH connection failed: {message}"),
            ),
        },
    )?;

    let command = resume_command(session_id);
    let input = if req.execute { format!("{command}\r") } else { command.clone() };
    let (status, delivery) = match created.session.write_input_confirmed(input.as_bytes()).await {
        Ok(SshWriteAck::Delivered) => (StatusCode::CREATED, "confirmed"),
        Ok(SshWriteAck::Unknown(_)) => (StatusCode::ACCEPTED, "unknown"),
        Ok(SshWriteAck::Failed(error)) | Err(error) => {
            close_created_tab(state, &created.tab_id, &created.pane_id);
            return Err(resume_error(
                StatusCode::BAD_GATEWAY,
                "WRITE_FAILED",
                format!("Failed to write resume command: {error}"),
            ));
        }
    };

    let resume_tab = ResumeTabState {
        tab_id: created.tab_id,
        pane_id: created.pane_id,
        executed: req.execute,
        capacity_permit,
    };
    let resume_lifecycle_guard =
        state.manager.resume_tabs_guard.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if !resume_tab_exists(state, &resume_tab) {
        drop(resume_lifecycle_guard);
        return Ok((
            status,
            response_for(
                "committed_tab_closed",
                resume_tab,
                profile_id,
                session_id,
                command,
                delivery,
            ),
        ));
    }
    state.manager.resume_tabs.insert(key, resume_tab.clone());
    drop(resume_lifecycle_guard);
    Ok((status, response_for(action, resume_tab, profile_id, session_id, command, delivery)))
}

fn response_for(
    action: &'static str,
    tab: ResumeTabState,
    profile_id: &str,
    session_id: &str,
    command: String,
    delivery: &'static str,
) -> ResumeSessionResponse {
    ResumeSessionResponse {
        ok: true,
        action,
        tab_id: tab.tab_id,
        pane_id: tab.pane_id,
        profile_id: profile_id.to_string(),
        session_id: session_id.to_string(),
        command,
        executed: tab.executed,
        delivery,
    }
}

fn close_resume_tab(state: &AgentState, tab: &ResumeTabState) {
    close_created_tab(state, &tab.tab_id, &tab.pane_id);
}

fn close_created_tab(state: &AgentState, tab_id: &str, pane_id: &str) {
    state.manager.kill_and_remove(pane_id);
    state.manager.remove_tab(tab_id);
    state.manager.broadcast_sync(&SyncMsg::TabClosed { pane_id: tab_id.to_string() });
}

fn activate_tab(state: &AgentState, tab_id: &str, pane_id: &str) -> bool {
    if !state.manager.sessions.contains_key(pane_id) {
        return false;
    }
    let Some(mut tab) = state.manager.tab_layouts.get_mut(tab_id) else {
        return false;
    };
    if !tab
        .get("layout")
        .is_some_and(|layout| collect_leaf_pane_ids(layout).iter().any(|id| id == pane_id))
    {
        return false;
    }
    tab["active_pane_id"] = serde_json::Value::String(pane_id.to_string());
    drop(tab);
    *state.manager.active_pane_id.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(pane_id.to_string());
    state.manager.broadcast_sync(&SyncMsg::TabActivated { pane_id: pane_id.to_string() });
    true
}

fn resume_tab_exists(state: &AgentState, tab: &ResumeTabState) -> bool {
    if !state.manager.sessions.contains_key(&tab.pane_id) {
        return false;
    }
    state
        .manager
        .tab_layouts
        .get(&tab.tab_id)
        .and_then(|value| value.get("layout").cloned())
        .is_some_and(|layout| {
            collect_leaf_pane_ids(&layout).iter().any(|pane_id| pane_id == &tab.pane_id)
        })
}

fn prune_resume_tabs(state: &AgentState) {
    let _guard =
        state.manager.resume_tabs_guard.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state.manager.resume_tabs.retain(|_, tab| resume_tab_exists(state, tab));
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value.chars().all(|ch| ch.is_ascii_alphanumeric() || "._:-".contains(ch))
}

fn normalize_cwd(value: Option<&str>) -> Result<Option<String>, &'static str> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.len() > 1024 || !value.starts_with('/') || value.contains(['\0', '\r', '\n']) {
        return Err("initial_cwd must be an absolute path without control characters");
    }
    Ok(Some(value.to_string()))
}

fn resume_command(session_id: &str) -> String {
    format!("siyuan resume {session_id}")
}

fn resume_key(profile_id: &str, session_id: &str) -> String {
    format!("{profile_id}\0{session_id}")
}

fn resume_lock_index(key: &str, lock_count: usize) -> usize {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    usize::try_from(hasher.finish()).unwrap_or(0) % lock_count.max(1)
}

fn resume_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> ResumeError {
    ResumeError { status, code, message: message.into() }
}

impl IntoResponse for ResumeError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(serde_json::json!({
                "error": {
                    "code": self.code,
                    "message": self.message,
                }
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_identifiers_without_allowing_shell_syntax() {
        assert!(valid_identifier("019f79d3-f18b-72d1-aab3-b687b40f10d4"));
        assert!(valid_identifier("dinotty-tailscale-10"));
        assert!(!valid_identifier("abc; shutdown"));
        assert!(!valid_identifier(""));
    }

    #[test]
    fn validates_remote_cwd() {
        assert_eq!(
            normalize_cwd(Some(" /home/ivan/project ")).unwrap(),
            Some("/home/ivan/project".into())
        );
        assert_eq!(normalize_cwd(None).unwrap(), None);
        assert!(normalize_cwd(Some("relative/path")).is_err());
        assert!(normalize_cwd(Some("/tmp/a\ncommand")).is_err());
    }

    #[test]
    fn builds_only_the_fixed_resume_command() {
        assert_eq!(
            resume_command("019f79d3-f18b-72d1-aab3-b687b40f10d4"),
            "siyuan resume 019f79d3-f18b-72d1-aab3-b687b40f10d4"
        );
    }

    #[test]
    fn resume_keys_are_scoped_by_profile_and_session() {
        assert_ne!(resume_key("profile-1", "session-1"), resume_key("profile-2", "session-1"));
        assert_ne!(resume_key("profile-1", "session-1"), resume_key("profile-1", "session-2"));
    }

    #[test]
    fn resume_lock_index_stays_inside_the_fixed_lock_pool() {
        assert!(resume_lock_index("profile\0session", 64) < 64);
    }
}
