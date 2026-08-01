use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};

use crate::session::{Session, SessionManager, SshSessionParams, SyncMsg};
use crate::settings::SettingsState;
use crate::ssh;

pub struct CreatedSshTab {
    pub tab_id: String,
    pub pane_id: String,
    pub layout: serde_json::Value,
    pub session: Arc<Session>,
}

pub enum CreateSshTabError {
    ProfileNotFound,
    Connection(String),
}

/// Creates an SSH-backed tab from a stored profile.
///
/// # Errors
/// Returns `ProfileNotFound` when the profile does not exist, or `Connection` when the SSH
/// session cannot be established.
pub async fn create_ssh_profile_tab(
    manager: &Arc<SessionManager>,
    settings: &SettingsState,
    req: &ssh::SshProfileConnectRequest,
) -> Result<CreatedSshTab, CreateSshTabError> {
    let tab_id = uuid::Uuid::new_v4().to_string();
    let pane_id = uuid::Uuid::new_v4().to_string();

    let params = {
        let settings = settings.read().await;
        let profile = settings.ssh_profiles.iter().find(|p| p.id == req.profile_id);
        match profile {
            Some(profile) => SshSessionParams {
                host: profile.host.clone(),
                port: profile.port,
                username: profile.username.clone(),
                auth_method: profile.auth_method.clone(),
                default_command: profile.default_command.clone(),
                profile_id: Some(profile.id.clone()),
                initial_cwd: req.initial_cwd.clone(),
            },
            None => return Err(CreateSshTabError::ProfileNotFound),
        }
    };

    let tab_title = format!("{}@{}", params.username, params.host);
    let (session, _shell_type) =
        ssh::create_ssh_session(manager, &pane_id, params, None).await.map_err(|error| {
            tracing::error!("Failed to create SSH session: {}", error);
            CreateSshTabError::Connection(error)
        })?;

    let layout = serde_json::json!({
        "type": "leaf",
        "paneId": pane_id,
        "title": tab_title,
        "shell_type": "ssh",
        "ratio": 1,
        "zoomed": false,
    });

    manager.insert_tab(
        tab_id.clone(),
        serde_json::json!({
            "layout": layout,
            "active_pane_id": pane_id,
        }),
    );
    *manager.active_pane_id.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(pane_id.clone());
    manager.broadcast_sync(&SyncMsg::TabCreated {
        tab_id: tab_id.clone(),
        pane_id: pane_id.clone(),
        layout: Some(layout.clone()),
        cwd: req.initial_cwd.clone(),
        connection_id: Some(req.profile_id.clone()),
    });

    Ok(CreatedSshTab { tab_id, pane_id, layout, session })
}

// ─── POST /api/tabs/ssh/quick ────────────────────────────────────

pub async fn create_ssh_quick_tab(
    State(manager): State<Arc<SessionManager>>,
    Json(req): Json<ssh::SshConnectRequest>,
) -> impl IntoResponse {
    let tab_id = uuid::Uuid::new_v4().to_string();
    let pane_id = uuid::Uuid::new_v4().to_string();

    let params = req.to_params();

    // 创建 SSH 会话
    let (_session, _shell_type) = match ssh::create_ssh_session(&manager, &pane_id, params, None)
        .await
    {
        Ok(x) => x,
        Err(e) => {
            tracing::error!("Failed to create SSH session: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e })))
                .into_response();
        }
    };

    // 创建初始布局
    let layout = serde_json::json!({
        "type": "leaf",
        "paneId": pane_id,
        "title": format!("{}@{}", req.username, req.host),
        "shell_type": "ssh",
        "ratio": 1,
        "zoomed": false,
    });

    // 存储 tab
    manager.insert_tab(
        tab_id.clone(),
        serde_json::json!({
            "layout": layout,
            "active_pane_id": pane_id,
        }),
    );

    // 设为活动 tab
    *manager.active_pane_id.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(pane_id.clone());

    // 广播
    manager.broadcast_sync(&SyncMsg::TabCreated {
        tab_id: tab_id.clone(),
        pane_id: pane_id.clone(),
        layout: Some(layout.clone()),
        cwd: None,
        connection_id: req.profile_id.clone(),
    });

    Json(serde_json::json!({
        "tab_id": tab_id,
        "pane_id": pane_id,
        "layout": layout,
        "connection_id": req.profile_id,
    }))
    .into_response()
}

// ─── POST /api/tabs/ssh ──────────────────────────────────────────

pub async fn create_ssh_tab(
    State((manager, settings)): State<(Arc<SessionManager>, SettingsState)>,
    Json(req): Json<ssh::SshProfileConnectRequest>,
) -> impl IntoResponse {
    match create_ssh_profile_tab(&manager, &settings, &req).await {
        Ok(created) => Json(serde_json::json!({
            "tab_id": created.tab_id,
            "pane_id": created.pane_id,
            "layout": created.layout,
            "connection_id": req.profile_id,
        }))
        .into_response(),
        Err(CreateSshTabError::ProfileNotFound) => {
            (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "profile not found" })))
                .into_response()
        }
        Err(CreateSshTabError::Connection(error)) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": error })))
                .into_response()
        }
    }
}
