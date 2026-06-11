use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
};

use crate::{
    app_config::{AppType, MultiAppConfig},
    cli::{
        commands::{
            config_common::canonical_common_snippet,
            provider_input::{
                build_provider_from_add_template, common_snippet_has_effective_config,
                current_timestamp, provider_add_template_choices, set_provider_common_config_meta,
                supports_common_config, validate_provider_add_template, ProviderAddTemplate,
            },
            provider_usage_query::{
                default_code_for_template_for_provider, default_usage_script_for_provider,
                normalize_usage_interval, validate_usage_script_for_save, UsageQueryTemplate,
            },
            update,
        },
        provider_quota::{
            provider_display_name, query_quota, quota_target_for_provider, ProviderUsageQuota,
        },
        tui::{fetch_provider_models_for_tui, ModelFetchStrategy},
    },
    config::{sanitize_provider_name, write_json_file},
    error::AppError,
    provider::{Provider, ProviderMeta, UsageResult, UsageScript},
    services::{
        CodexOAuthService, ConfigService, CredentialStatus, ProviderService, SpeedtestService,
        StreamCheckService,
    },
    store::AppState as CcSwitchState,
};

#[derive(Subcommand, Debug, Clone)]
pub enum WebCommand {
    /// Serve the web provider management UI and JSON API
    Serve {
        /// Address to bind
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Port to bind
        #[arg(long, default_value_t = 3088)]
        port: u16,
    },
}

pub fn execute(cmd: WebCommand) -> Result<(), AppError> {
    match cmd {
        WebCommand::Serve { host, port } => serve(host, port),
    }
}

fn serve(host: String, port: u16) -> Result<(), AppError> {
    let state = CcSwitchState::try_new_with_startup_recovery()?;
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|err| AppError::Message(format!("invalid bind address {host}:{port}: {err}")))?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| AppError::Message(format!("failed to create web runtime: {err}")))?;

    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|err| AppError::Message(format!("failed to bind {addr}: {err}")))?;
        println!("cc-switch web UI: http://{addr}");
        axum::serve(
            listener,
            router(Arc::new(WebAppState {
                state: Arc::new(state),
            })),
        )
        .await
        .map_err(|err| AppError::Message(format!("web server stopped: {err}")))
    })
}

#[derive(Clone)]
struct WebAppState {
    state: Arc<CcSwitchState>,
}

fn router(state: Arc<WebAppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/static/app.js", get(static_app_js))
        .route("/static/style.css", get(static_style_css))
        .route("/api/health", get(health))
        .route("/api/apps", get(apps))
        .route("/api/config", get(config_overview))
        .route("/api/config/snapshot", post(save_config_snapshot))
        .route(
            "/api/config/common/:app",
            get(get_common_config)
                .put(set_common_config)
                .delete(clear_common_config),
        )
        .route(
            "/api/config/backups",
            get(list_config_backups).post(create_config_backup),
        )
        .route("/api/config/restore", post(restore_config_backup))
        .route("/api/config/export-sql", get(export_config_sql))
        .route("/api/config/import-sql", post(import_config_sql))
        .route("/api/config/sync-live", post(sync_live_config))
        .route("/api/update/check", get(check_update))
        .route("/api/update/apply", post(apply_update))
        .route("/api/providers", get(list_providers).post(create_provider))
        .route("/api/providers/templates", get(provider_templates))
        .route(
            "/api/providers/:app/:id",
            get(get_provider)
                .put(update_provider)
                .delete(delete_provider),
        )
        .route("/api/providers/:app/:id/switch", post(switch_provider))
        .route(
            "/api/providers/:app/:id/duplicate",
            post(duplicate_provider),
        )
        .route(
            "/api/providers/:app/:id/remove-from-config",
            post(remove_from_config),
        )
        .route("/api/providers/:app/import-live", post(import_live_config))
        .route(
            "/api/providers/:app/set-default",
            post(set_default_provider),
        )
        .route("/api/providers/:app/sort", post(update_sort_order))
        .route(
            "/api/providers/:app/:id/speedtest",
            post(speedtest_provider),
        )
        .route(
            "/api/providers/:app/:id/stream-check",
            post(stream_check_provider),
        )
        .route(
            "/api/providers/:app/:id/fetch-models",
            post(fetch_models_provider),
        )
        .route("/api/providers/:app/fetch-models", post(fetch_models_once))
        .route("/api/providers/:app/:id/quota", post(quota_provider))
        .route(
            "/api/providers/:app/:id/usage-query",
            get(get_usage_query)
                .put(set_usage_query)
                .delete(clear_usage_query),
        )
        .route("/api/providers/:app/:id/export", post(export_provider))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../../../web/index.html"))
}

async fn static_app_js() -> impl IntoResponse {
    static_file(
        "application/javascript; charset=utf-8",
        include_str!("../../../web/app.js"),
    )
}

async fn static_style_css() -> impl IntoResponse {
    static_file(
        "text/css; charset=utf-8",
        include_str!("../../../web/style.css"),
    )
}

fn static_file(content_type: &'static str, body: &'static str) -> Response {
    let mut response = body.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "service": "cc-switch-web" }))
}

async fn apps() -> Json<Value> {
    Json(json!({
        "apps": AppType::all()
            .map(|app| json!({
                "id": app.as_str(),
                "label": app_label(&app),
                "additiveMode": app.is_additive_mode(),
                "supportsFailover": app.supports_failover(),
                "supportsCommonConfig": supports_common_config(&app),
            }))
            .collect::<Vec<_>>()
    }))
}

async fn config_overview(
    State(app_state): State<Arc<WebAppState>>,
) -> Result<Json<Value>, ApiError> {
    let config = app_state
        .state
        .config
        .read()
        .map_err(AppError::from)?
        .clone();
    Ok(Json(json!({
        "ok": true,
        "paths": config_paths_json(),
        "validation": config_validation_json(&app_state.state)?,
        "backups": config_backups_json()?,
        "config": config,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigSnapshotSaveRequest {
    config: MultiAppConfig,
    confirm: bool,
    backup_name: Option<String>,
    sync_live: Option<bool>,
}

async fn save_config_snapshot(
    State(app_state): State<Arc<WebAppState>>,
    Json(request): Json<ConfigSnapshotSaveRequest>,
) -> Result<Json<Value>, ApiError> {
    if !request.confirm {
        return Err(AppError::InvalidInput(
            "Saving the raw configuration snapshot requires confirm=true.".to_string(),
        )
        .into());
    }
    validate_config_snapshot(&request.config)?;

    let backup_id = create_named_backup(request.backup_name.as_deref())?;
    {
        let mut config = app_state.state.config.write().map_err(AppError::from)?;
        *config = request.config;
    }
    app_state.state.save()?;
    app_state.state.refresh_config_from_db()?;

    let sync_warning =
        sync_current_to_live_if_requested(&app_state.state, request.sync_live.unwrap_or(false));

    Ok(Json(json!({
        "ok": true,
        "backupId": backup_id,
        "syncWarning": sync_warning,
        "validation": config_validation_json(&app_state.state)?,
        "backups": config_backups_json()?,
    })))
}

async fn get_common_config(
    State(app_state): State<Arc<WebAppState>>,
    Path(app): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    Ok(Json(common_config_json(&app_state.state, &app)?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommonConfigSetRequest {
    snippet: String,
}

async fn set_common_config(
    State(app_state): State<Arc<WebAppState>>,
    Path(app): Path<String>,
    Json(request): Json<CommonConfigSetRequest>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    ensure_common_config_supported(&app)?;
    match canonical_common_snippet(app.clone(), &request.snippet)? {
        Some(snippet) => ProviderService::set_common_config_snippet(
            &app_state.state,
            app.clone(),
            Some(snippet),
        )?,
        None => ProviderService::clear_common_config_snippet(&app_state.state, app.clone())?,
    }
    Ok(Json(common_config_json(&app_state.state, &app)?))
}

async fn clear_common_config(
    State(app_state): State<Arc<WebAppState>>,
    Path(app): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    ensure_common_config_supported(&app)?;
    ProviderService::clear_common_config_snippet(&app_state.state, app.clone())?;
    Ok(Json(common_config_json(&app_state.state, &app)?))
}

async fn list_config_backups() -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({
        "ok": true,
        "backups": config_backups_json()?,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateConfigBackupRequest {
    name: Option<String>,
}

async fn create_config_backup(
    Json(request): Json<CreateConfigBackupRequest>,
) -> Result<Json<Value>, ApiError> {
    let backup_id = create_named_backup(request.name.as_deref())?;
    Ok(Json(json!({
        "ok": true,
        "backupId": backup_id,
        "backups": config_backups_json()?,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoreConfigRequest {
    backup_id: String,
    confirm: bool,
    sync_live: Option<bool>,
}

async fn restore_config_backup(
    State(app_state): State<Arc<WebAppState>>,
    Json(request): Json<RestoreConfigRequest>,
) -> Result<Json<Value>, ApiError> {
    if !request.confirm {
        return Err(AppError::InvalidInput(
            "Restoring configuration requires confirm=true.".to_string(),
        )
        .into());
    }
    let backup_id = request.backup_id.trim();
    validate_backup_token(backup_id, "backup id")?;

    let pre_restore_backup = ConfigService::restore_from_backup_id(backup_id, &app_state.state)?;
    app_state.state.refresh_config_from_db()?;
    let sync_warning =
        sync_current_to_live_if_requested(&app_state.state, request.sync_live.unwrap_or(true));

    Ok(Json(json!({
        "ok": true,
        "restored": backup_id,
        "preRestoreBackup": pre_restore_backup,
        "syncWarning": sync_warning,
        "validation": config_validation_json(&app_state.state)?,
        "backups": config_backups_json()?,
    })))
}

async fn export_config_sql(
    State(app_state): State<Arc<WebAppState>>,
) -> Result<Response, ApiError> {
    let sql = app_state.state.db.export_sql_string()?;
    let filename = format!(
        "cc-switch-config-{}.sql",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    );
    let mut response = sql.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/sql; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")).map_err(|err| {
            AppError::Message(format!("failed to build export response header: {err}"))
        })?,
    );
    Ok(response)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportConfigSqlRequest {
    sql: String,
    confirm: bool,
    sync_live: Option<bool>,
}

async fn import_config_sql(
    State(app_state): State<Arc<WebAppState>>,
    Json(request): Json<ImportConfigSqlRequest>,
) -> Result<Json<Value>, ApiError> {
    if !request.confirm {
        return Err(AppError::InvalidInput(
            "Importing configuration requires confirm=true.".to_string(),
        )
        .into());
    }
    if request.sql.trim().is_empty() {
        return Err(AppError::InvalidInput("SQL import body cannot be empty.".to_string()).into());
    }

    let backup_id = app_state.state.db.import_sql_string(&request.sql)?;
    app_state.state.refresh_config_from_db()?;
    let sync_warning =
        sync_current_to_live_if_requested(&app_state.state, request.sync_live.unwrap_or(true));

    Ok(Json(json!({
        "ok": true,
        "backupId": backup_id,
        "syncWarning": sync_warning,
        "validation": config_validation_json(&app_state.state)?,
        "backups": config_backups_json()?,
    })))
}

async fn sync_live_config(
    State(app_state): State<Arc<WebAppState>>,
) -> Result<Json<Value>, ApiError> {
    ProviderService::sync_current_to_live(&app_state.state)?;
    Ok(Json(json!({
        "ok": true,
        "validation": config_validation_json(&app_state.state)?,
    })))
}

async fn check_update() -> Result<Json<Value>, ApiError> {
    let info = update::check_for_update().await?;
    Ok(Json(json!({ "ok": true, "update": info })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyUpdateRequest {
    target_tag: Option<String>,
}

async fn apply_update(Json(request): Json<ApplyUpdateRequest>) -> Result<Json<Value>, ApiError> {
    let target_tag = match request
        .target_tag
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(target) => target.to_string(),
        None => {
            let info = update::check_for_update().await?;
            if info.is_already_latest {
                return Ok(Json(json!({
                    "ok": true,
                    "updated": false,
                    "message": "Already on latest version.",
                    "update": info,
                })));
            }
            if info.is_homebrew_managed {
                return Err(AppError::InvalidInput(
                    "Homebrew-managed installations must be updated with brew upgrade cc-switch."
                        .to_string(),
                )
                .into());
            }
            if info.is_downgrade {
                return Err(AppError::InvalidInput(format!(
                    "Current version is newer than {}; automatic downgrade is not allowed.",
                    info.target_tag
                ))
                .into());
            }
            info.target_tag
        }
    };

    let progress = Arc::new(Mutex::new((0_u64, None::<u64>)));
    let progress_cb = Arc::clone(&progress);
    update::download_and_apply(&target_tag, move |downloaded, total| {
        if let Ok(mut guard) = progress_cb.lock() {
            *guard = (downloaded, total);
        }
    })
    .await?;
    let (downloaded, total) = progress.lock().map(|guard| *guard).unwrap_or((0, None));

    Ok(Json(json!({
        "ok": true,
        "updated": true,
        "targetTag": target_tag,
        "downloadedBytes": downloaded,
        "totalBytes": total,
    })))
}

fn config_paths_json() -> Value {
    let config_dir = crate::config::get_app_config_dir();
    let db_path = config_dir.join("cc-switch.db");
    let legacy_config_path = config_dir.join("config.json");
    let backup_dir = config_dir.join("backups");

    json!({
        "configDir": config_dir.display().to_string(),
        "database": db_path.display().to_string(),
        "legacyJson": legacy_config_path.display().to_string(),
        "backupDir": backup_dir.display().to_string(),
    })
}

fn config_validation_json(state: &CcSwitchState) -> Result<Value, AppError> {
    let config_dir = crate::config::get_app_config_dir();
    let db_path = config_dir.join("cc-switch.db");
    let legacy_config_path = config_dir.join("config.json");
    let backup_dir = config_dir.join("backups");

    let mut provider_counts = serde_json::Map::new();
    let mut prompt_counts = serde_json::Map::new();
    for app in AppType::all() {
        provider_counts.insert(
            app.as_str().to_string(),
            json!(state.db.get_all_providers(app.as_str())?.len()),
        );
        prompt_counts.insert(
            app.as_str().to_string(),
            json!(state.db.get_prompts(app.as_str())?.len()),
        );
    }

    Ok(json!({
        "databaseExists": db_path.exists(),
        "databaseBytes": file_size(&db_path),
        "legacyJsonExists": legacy_config_path.exists(),
        "backupDirExists": backup_dir.exists(),
        "providerCounts": provider_counts,
        "promptCounts": prompt_counts,
        "mcpServers": state.db.get_all_mcp_servers()?.len(),
        "skillsInstalled": state.db.get_all_installed_skills()?.len(),
        "schemaReadable": true,
    }))
}

fn config_backups_json() -> Result<Vec<Value>, AppError> {
    let config_path = crate::config::get_app_config_path();
    ConfigService::list_backups(&config_path).map(|backups| {
        backups
            .into_iter()
            .map(|backup| {
                let bytes = file_size(&backup.path);
                json!({
                    "id": backup.id,
                    "path": backup.path.display().to_string(),
                    "timestamp": backup.timestamp,
                    "displayName": backup.display_name,
                    "bytes": bytes,
                })
            })
            .collect()
    })
}

fn common_config_json(state: &CcSwitchState, app: &AppType) -> Result<Value, AppError> {
    ensure_common_config_supported(app)?;
    let snippet = {
        let config = state.config.read().map_err(AppError::from)?;
        config.common_config_snippets.get(app).cloned()
    };
    Ok(json!({
        "ok": true,
        "app": app.as_str(),
        "format": if matches!(app, AppType::Codex) { "toml" } else { "json" },
        "snippet": snippet,
        "configured": common_snippet_has_effective_config(app, snippet.as_deref()),
        "supported": true,
    }))
}

fn ensure_common_config_supported(app: &AppType) -> Result<(), AppError> {
    if supports_common_config(app) {
        Ok(())
    } else {
        Err(AppError::InvalidInput(format!(
            "{} does not support common config snippets.",
            app.as_str()
        )))
    }
}

fn create_named_backup(name: Option<&str>) -> Result<String, AppError> {
    let name = name.map(str::trim).filter(|value| !value.is_empty());
    if let Some(name) = name {
        validate_backup_token(name, "backup name")?;
    }
    ConfigService::create_backup(
        &crate::config::get_app_config_path(),
        name.map(str::to_string),
    )
}

fn validate_backup_token(value: &str, label: &str) -> Result<(), AppError> {
    if value.trim().is_empty() {
        return Err(AppError::InvalidInput(format!("{label} cannot be empty.")));
    }
    if value.contains("..") || value.contains('/') || value.contains('\\') {
        return Err(AppError::InvalidInput(format!(
            "{label} cannot contain path separators or '..'."
        )));
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
    {
        return Err(AppError::InvalidInput(format!(
            "{label} can contain only ASCII letters, numbers, '.', '-' and '_'."
        )));
    }
    Ok(())
}

fn validate_config_snapshot(config: &MultiAppConfig) -> Result<(), AppError> {
    for app in AppType::all() {
        let manager = config.get_manager(&app).ok_or_else(|| {
            AppError::InvalidInput(format!("Missing manager for app '{}'.", app.as_str()))
        })?;
        if !manager.current.trim().is_empty() && !manager.providers.contains_key(&manager.current) {
            return Err(AppError::InvalidInput(format!(
                "{} current provider '{}' does not exist in providers.",
                app.as_str(),
                manager.current
            )));
        }
        for (id, provider) in &manager.providers {
            if id != &provider.id {
                return Err(AppError::InvalidInput(format!(
                    "{} provider map key '{}' does not match provider.id '{}'.",
                    app.as_str(),
                    id,
                    provider.id
                )));
            }
            validate_provider_identity(&app, provider)?;
        }
    }
    Ok(())
}

fn sync_current_to_live_if_requested(state: &CcSwitchState, sync_live: bool) -> Option<String> {
    if !sync_live {
        return None;
    }
    ProviderService::sync_current_to_live(state)
        .err()
        .map(|err| err.to_string())
}

fn file_size(path: &PathBuf) -> Option<u64> {
    std::fs::metadata(path).ok().map(|metadata| metadata.len())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderListQuery {
    app: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProvidersResponse {
    apps: Vec<AppProviders>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppProviders {
    app: String,
    label: &'static str,
    additive_mode: bool,
    current: String,
    common_config: CommonConfigInfo,
    providers: Vec<ProviderSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommonConfigInfo {
    supported: bool,
    configured: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderSummary {
    id: String,
    name: String,
    api_url: Option<String>,
    website_url: Option<String>,
    category: Option<String>,
    sort_index: Option<usize>,
    notes: Option<String>,
    icon: Option<String>,
    icon_color: Option<String>,
    in_failover_queue: bool,
    current: bool,
    live_config_managed: Option<bool>,
    common_config_enabled: Option<bool>,
    usage_query_enabled: bool,
    api_format: Option<String>,
}

async fn list_providers(
    State(app_state): State<Arc<WebAppState>>,
    Query(query): Query<ProviderListQuery>,
) -> Result<Json<ProvidersResponse>, ApiError> {
    let app_filter = parse_optional_app(query.app.as_deref())?;
    let mut apps_out = Vec::new();

    for app in AppType::all() {
        if app_filter.as_ref().is_some_and(|filter| filter != &app) {
            continue;
        }
        let providers = ProviderService::list(&app_state.state, app.clone())?;
        let current = ProviderService::current(&app_state.state, app.clone()).unwrap_or_default();
        let common_snippet = {
            let cfg = app_state.state.config.read().map_err(AppError::from)?;
            cfg.common_config_snippets.get(&app).cloned()
        };
        let mut provider_entries = providers.into_iter().collect::<Vec<_>>();
        provider_entries.sort_by(|(_, a), (_, b)| match (a.sort_index, b.sort_index) {
            (Some(a_idx), Some(b_idx)) => a_idx.cmp(&b_idx),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.created_at.cmp(&b.created_at),
        });
        let summaries = provider_entries
            .into_iter()
            .map(|(id, provider)| {
                provider_summary(&app, &current, id, provider, common_snippet.as_deref())
            })
            .collect::<Vec<_>>();

        apps_out.push(AppProviders {
            app: app.as_str().to_string(),
            label: app_label(&app),
            additive_mode: app.is_additive_mode(),
            current,
            common_config: CommonConfigInfo {
                supported: supports_common_config(&app),
                configured: common_snippet_has_effective_config(&app, common_snippet.as_deref()),
            },
            providers: summaries,
        });
    }

    Ok(Json(ProvidersResponse { apps: apps_out }))
}

fn provider_summary(
    app: &AppType,
    current: &str,
    id: String,
    provider: Provider,
    common_snippet: Option<&str>,
) -> ProviderSummary {
    ProviderSummary {
        current: id == current,
        api_url: extract_api_url(&provider, app),
        website_url: provider.website_url.clone(),
        category: provider.category.clone(),
        sort_index: provider.sort_index,
        notes: provider.notes.clone(),
        icon: provider.icon.clone(),
        icon_color: provider.icon_color.clone(),
        in_failover_queue: provider.in_failover_queue,
        live_config_managed: provider
            .meta
            .as_ref()
            .and_then(|meta| meta.live_config_managed),
        common_config_enabled: provider
            .meta
            .as_ref()
            .and_then(|meta| meta.apply_common_config)
            .or_else(|| {
                supports_common_config(app).then(|| {
                    ProviderService::provider_uses_common_config_for_app(
                        app,
                        &provider,
                        common_snippet,
                    )
                })
            }),
        usage_query_enabled: provider
            .meta
            .as_ref()
            .and_then(|meta| meta.usage_script.as_ref())
            .is_some_and(|script| script.enabled),
        api_format: provider
            .meta
            .as_ref()
            .and_then(|meta| meta.api_format.clone()),
        id,
        name: provider.name,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderDetailResponse {
    app: String,
    current: bool,
    additive_mode: bool,
    common_config: CommonConfigInfo,
    provider: Provider,
    derived: ProviderDerived,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderDerived {
    api_url: Option<String>,
    common_config_enabled: Option<bool>,
    usage_query_enabled: bool,
    usage_template: Option<String>,
}

async fn get_provider(
    State(app_state): State<Arc<WebAppState>>,
    Path((app, id)): Path<(String, String)>,
) -> Result<Json<ProviderDetailResponse>, ApiError> {
    let app = parse_app(&app)?;
    let provider = find_provider(&app_state.state, &app, &id)?;
    let current = ProviderService::current(&app_state.state, app.clone()).unwrap_or_default();
    let common_snippet = {
        let cfg = app_state.state.config.read().map_err(AppError::from)?;
        cfg.common_config_snippets.get(&app).cloned()
    };
    let common_config_enabled = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.apply_common_config)
        .or_else(|| {
            supports_common_config(&app).then(|| {
                ProviderService::provider_uses_common_config_for_app(
                    &app,
                    &provider,
                    common_snippet.as_deref(),
                )
            })
        });

    Ok(Json(ProviderDetailResponse {
        app: app.as_str().to_string(),
        current: id == current,
        additive_mode: app.is_additive_mode(),
        common_config: CommonConfigInfo {
            supported: supports_common_config(&app),
            configured: common_snippet_has_effective_config(&app, common_snippet.as_deref()),
        },
        derived: ProviderDerived {
            api_url: extract_api_url(&provider, &app),
            common_config_enabled,
            usage_query_enabled: provider
                .meta
                .as_ref()
                .and_then(|meta| meta.usage_script.as_ref())
                .is_some_and(|script| script.enabled),
            usage_template: provider
                .meta
                .as_ref()
                .and_then(|meta| meta.usage_script.as_ref())
                .and_then(|script| effective_usage_template_type(script, &provider)),
        },
        provider,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderCreateRequest {
    app: String,
    template: Option<ProviderAddTemplate>,
    provider: Option<Provider>,
    common_config_enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderUpdateRequest {
    provider: Provider,
    common_config_enabled: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderMutationResponse {
    ok: bool,
    app: String,
    provider: Provider,
}

async fn create_provider(
    State(app_state): State<Arc<WebAppState>>,
    Json(request): Json<ProviderCreateRequest>,
) -> Result<Json<ProviderMutationResponse>, ApiError> {
    let app = parse_app(&request.app)?;
    let providers = ProviderService::list(&app_state.state, app.clone())?;
    let existing_ids = providers.keys().cloned().collect::<Vec<_>>();

    let mut provider = match (request.provider, request.template) {
        (Some(provider), _) => provider,
        (None, Some(template)) => {
            validate_provider_add_template(&app, template)?;
            if template.is_custom() {
                return Err(AppError::InvalidInput(
                    "Custom template requires a provider payload".to_string(),
                )
                .into());
            }
            build_provider_from_add_template(&app, template, &existing_ids)?
        }
        (None, None) => {
            return Err(AppError::InvalidInput(
                "provider payload or template is required".to_string(),
            )
            .into())
        }
    };
    if provider.created_at.is_none() {
        provider.created_at = Some(current_timestamp());
    }
    if let Some(enabled) = request
        .common_config_enabled
        .filter(|_| common_config_available(&app_state.state, &app).unwrap_or(false))
    {
        set_provider_common_config_meta(&mut provider, enabled);
    }
    validate_provider_identity(&app, &provider)?;

    ProviderService::add(&app_state.state, app.clone(), provider.clone())?;
    let provider = find_provider(&app_state.state, &app, &provider.id)?;
    Ok(Json(ProviderMutationResponse {
        ok: true,
        app: app.as_str().to_string(),
        provider,
    }))
}

async fn update_provider(
    State(app_state): State<Arc<WebAppState>>,
    Path((app, id)): Path<(String, String)>,
    Json(request): Json<ProviderUpdateRequest>,
) -> Result<Json<ProviderMutationResponse>, ApiError> {
    let app = parse_app(&app)?;
    if request.provider.id != id {
        return Err(AppError::InvalidInput(format!(
            "provider id mismatch: path '{id}' vs payload '{}'",
            request.provider.id
        ))
        .into());
    }
    let mut provider = request.provider;
    if let Some(enabled) = request
        .common_config_enabled
        .filter(|_| common_config_available(&app_state.state, &app).unwrap_or(false))
    {
        set_provider_common_config_meta(&mut provider, enabled);
    }
    validate_provider_identity(&app, &provider)?;
    ProviderService::update(&app_state.state, app.clone(), provider.clone())?;
    let provider = find_provider(&app_state.state, &app, &id)?;
    Ok(Json(ProviderMutationResponse {
        ok: true,
        app: app.as_str().to_string(),
        provider,
    }))
}

async fn delete_provider(
    State(app_state): State<Arc<WebAppState>>,
    Path((app, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    ProviderService::delete(&app_state.state, app.clone(), &id)?;
    Ok(Json(json!({ "ok": true, "app": app.as_str(), "id": id })))
}

async fn switch_provider(
    State(app_state): State<Arc<WebAppState>>,
    Path((app, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    let provider = find_provider(&app_state.state, &app, &id)?;
    ProviderService::switch(&app_state.state, app.clone(), &id)?;
    if let Err(err) = crate::claude_plugin::sync_claude_plugin_on_provider_switch(&app, &provider) {
        return Ok(Json(json!({
            "ok": true,
            "app": app.as_str(),
            "id": id,
            "warning": format!("Claude plugin sync failed: {err}")
        })));
    }
    Ok(Json(json!({ "ok": true, "app": app.as_str(), "id": id })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DuplicateProviderRequest {
    provider: Option<Provider>,
}

async fn duplicate_provider(
    State(app_state): State<Arc<WebAppState>>,
    Path((app, id)): Path<(String, String)>,
    Json(request): Json<DuplicateProviderRequest>,
) -> Result<Json<ProviderMutationResponse>, ApiError> {
    let app = parse_app(&app)?;
    let provider =
        ProviderService::duplicate(&app_state.state, app.clone(), &id, request.provider)?;
    Ok(Json(ProviderMutationResponse {
        ok: true,
        app: app.as_str().to_string(),
        provider,
    }))
}

async fn import_live_config(
    State(app_state): State<Arc<WebAppState>>,
    Path(app): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    let imported = ProviderService::import_live_config(&app_state.state, app.clone())?;
    Ok(Json(
        json!({ "ok": true, "app": app.as_str(), "imported": imported }),
    ))
}

async fn remove_from_config(
    State(app_state): State<Arc<WebAppState>>,
    Path((app, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    ProviderService::remove_from_live_config(&app_state.state, app.clone(), &id)?;
    Ok(Json(json!({ "ok": true, "app": app.as_str(), "id": id })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetDefaultRequest {
    provider_id: String,
    model: Option<String>,
}

async fn set_default_provider(
    State(app_state): State<Arc<WebAppState>>,
    Path(app): Path<String>,
    Json(request): Json<SetDefaultRequest>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    let default = ProviderService::set_default_model(
        &app_state.state,
        app.clone(),
        &request.provider_id,
        request.model.as_deref(),
    )?;
    Ok(Json(
        json!({ "ok": true, "app": app.as_str(), "default": default }),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SortRequest {
    updates: Vec<crate::services::provider::ProviderSortUpdate>,
}

async fn update_sort_order(
    State(app_state): State<Arc<WebAppState>>,
    Path(app): Path<String>,
    Json(request): Json<SortRequest>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    ProviderService::update_sort_order(&app_state.state, app.clone(), request.updates)?;
    Ok(Json(json!({ "ok": true, "app": app.as_str() })))
}

async fn speedtest_provider(
    State(app_state): State<Arc<WebAppState>>,
    Path((app, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    let provider = find_provider(&app_state.state, &app, &id)?;
    let api_url = extract_api_url(&provider, &app)
        .ok_or_else(|| AppError::Message(format!("No API URL configured for provider '{id}'")))?;
    let results = SpeedtestService::test_endpoints(vec![api_url], None).await?;
    Ok(Json(
        json!({ "ok": true, "app": app.as_str(), "id": id, "results": results }),
    ))
}

async fn stream_check_provider(
    State(app_state): State<Arc<WebAppState>>,
    Path((app, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    let provider = find_provider(&app_state.state, &app, &id)?;
    let config = app_state.state.db.get_stream_check_config()?;
    let result = StreamCheckService::check_with_retry(&app, &provider, &config).await?;
    let _ = app_state
        .state
        .db
        .save_stream_check_log(&id, &provider.name, app.as_str(), &result);
    Ok(Json(
        json!({ "ok": true, "app": app.as_str(), "id": id, "result": result }),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchModelsOnceRequest {
    base_url: String,
    api_key: Option<String>,
    auth: Option<ModelFetchAuth>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ModelFetchAuth {
    Bearer,
    Anthropic,
    GoogleApiKey,
}

async fn fetch_models_provider(
    State(app_state): State<Arc<WebAppState>>,
    Path((app, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    let provider = find_provider(&app_state.state, &app, &id)?;
    let models = fetch_models_for_provider(&app, &provider).await?;
    Ok(Json(
        json!({ "ok": true, "app": app.as_str(), "id": id, "models": models }),
    ))
}

async fn fetch_models_once(
    Path(app): Path<String>,
    Json(request): Json<FetchModelsOnceRequest>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    let base_url = request.base_url.trim().trim_end_matches('/').to_string();
    if base_url.is_empty() {
        return Err(
            AppError::Message("No API URL configured for one-off model fetch".into()).into(),
        );
    }
    let api_key = request
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let strategy = request
        .auth
        .map(model_fetch_auth_to_strategy)
        .unwrap_or_else(|| default_model_fetch_strategy(&app));
    let models = fetch_provider_models_for_tui(&base_url, api_key, strategy)
        .await
        .map_err(AppError::Message)?;
    Ok(Json(
        json!({ "ok": true, "app": app.as_str(), "models": models }),
    ))
}

async fn quota_provider(
    State(app_state): State<Arc<WebAppState>>,
    Path((app, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    let provider = find_provider(&app_state.state, &app, &id)?;
    let provider_name = provider_display_name(&app, &id, &provider);
    let target = quota_target_for_provider(&app, &id, &provider);
    let queried_at = chrono::Utc::now().timestamp_millis();

    let output = match target {
        Some(target) => match query_quota(&target).await {
            Ok(result) => {
                let (status, available, error) = quota_result_summary(&result);
                json!({
                    "app": app.as_str(),
                    "providerId": id,
                    "providerName": provider_name,
                    "target": target,
                    "status": status,
                    "available": available,
                    "queriedAt": queried_at,
                    "result": result,
                    "error": error
                })
            }
            Err(error) => json!({
                "app": app.as_str(),
                "providerId": id,
                "providerName": provider_name,
                "target": target,
                "status": "query_failed",
                "available": false,
                "queriedAt": queried_at,
                "result": null,
                "error": error
            }),
        },
        None => json!({
            "app": app.as_str(),
            "providerId": id,
            "providerName": provider_name,
            "target": null,
            "status": "not_available",
            "available": false,
            "queriedAt": queried_at,
            "result": null,
            "error": null
        }),
    };

    Ok(Json(json!({ "ok": true, "quota": output })))
}

async fn get_usage_query(
    State(app_state): State<Arc<WebAppState>>,
    Path((app, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    let provider = find_provider(&app_state.state, &app, &id)?;
    let script = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.usage_script.clone());
    let effective_template = script
        .as_ref()
        .and_then(|script| effective_usage_template_type(script, &provider))
        .or_else(|| default_usage_script_for_provider(&provider).template_type);
    Ok(Json(json!({
        "ok": true,
        "app": app.as_str(),
        "id": id,
        "usageQuery": script,
        "effectiveTemplate": effective_template
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageQuerySetRequest {
    enabled: Option<bool>,
    template: Option<UsageQueryTemplate>,
    code: Option<String>,
    timeout: Option<u64>,
    auto_query_interval: Option<u64>,
    api_key: Option<String>,
    base_url: Option<String>,
    access_token: Option<String>,
    user_id: Option<String>,
}

async fn set_usage_query(
    State(app_state): State<Arc<WebAppState>>,
    Path((app, id)): Path<(String, String)>,
    Json(request): Json<UsageQuerySetRequest>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    let mut provider = find_provider(&app_state.state, &app, &id)?;
    let existing = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.usage_script.as_ref())
        .cloned();
    let existing_template = existing
        .as_ref()
        .and_then(|script| effective_usage_template_type(script, &provider));
    let mut script = existing.unwrap_or_else(|| default_usage_script_for_provider(&provider));

    if let Some(enabled) = request.enabled {
        script.enabled = enabled;
    }
    if let Some(template) = request.template {
        script.template_type = Some(template.as_str().to_string());
        if request.code.is_none() {
            script.code = default_code_for_template_for_provider(template, &app, &provider);
        }
    } else if script
        .template_type
        .as_ref()
        .is_none_or(|value| value.trim().is_empty())
    {
        script.template_type = existing_template.or_else(|| {
            default_usage_script_for_provider(&provider)
                .template_type
                .filter(|value| !value.trim().is_empty())
        });
    }
    if let Some(code) = request.code.as_ref() {
        script.code = code.clone();
    }
    if let Some(timeout) = request.timeout {
        script.timeout = Some(timeout);
    }
    if let Some(interval) = request.auto_query_interval {
        script.auto_query_interval = Some(normalize_usage_interval(interval));
    }
    script.language = "javascript".to_string();
    apply_usage_credentials(&mut script, &request);
    validate_usage_script_for_save(&script)?;

    provider
        .meta
        .get_or_insert_with(ProviderMeta::default)
        .usage_script = Some(script.clone());
    ProviderService::update(&app_state.state, app.clone(), provider)?;
    Ok(Json(
        json!({ "ok": true, "app": app.as_str(), "id": id, "usageQuery": script }),
    ))
}

async fn clear_usage_query(
    State(app_state): State<Arc<WebAppState>>,
    Path((app, id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    let mut provider = find_provider(&app_state.state, &app, &id)?;
    if let Some(meta) = provider.meta.as_mut() {
        meta.usage_script = None;
    }
    ProviderService::update(&app_state.state, app.clone(), provider)?;
    Ok(Json(json!({ "ok": true, "app": app.as_str(), "id": id })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportProviderRequest {
    output: Option<PathBuf>,
    write: Option<bool>,
}

async fn export_provider(
    State(app_state): State<Arc<WebAppState>>,
    Path((app, id)): Path<(String, String)>,
    Json(request): Json<ExportProviderRequest>,
) -> Result<Json<Value>, ApiError> {
    let app = parse_app(&app)?;
    if !matches!(app, AppType::Claude) {
        return Err(AppError::Message(format!(
            "Provider export currently supports only Claude standalone settings files. Current app: {}.",
            app.as_str()
        ))
        .into());
    }
    let (provider, common_config_snippet) = {
        let config = app_state.state.config.read().map_err(AppError::from)?;
        let manager = config
            .get_manager(&app)
            .ok_or_else(|| AppError::Message(format!("{} config not found", app.as_str())))?;
        let provider = manager.providers.get(&id).cloned().ok_or_else(|| {
            AppError::localized(
                "provider.not_found",
                format!("供应商不存在: {id}"),
                format!("Provider not found: {id}"),
            )
        })?;
        (provider, config.common_config_snippets.get(&app).cloned())
    };
    let apply_common_config = ProviderService::provider_uses_common_config_for_app(
        &app,
        &provider,
        common_config_snippet.as_deref(),
    );
    let settings_content = ProviderService::build_live_backup_snapshot(
        &app,
        &provider,
        common_config_snippet.as_deref(),
        apply_common_config,
    )?;
    let output_path = resolve_export_path(request.output, &provider)?;
    if request.write.unwrap_or(false) {
        write_json_file(&output_path, &settings_content)?;
    }
    Ok(Json(json!({
        "ok": true,
        "app": app.as_str(),
        "id": id,
        "path": output_path,
        "written": request.write.unwrap_or(false),
        "settings": settings_content
    })))
}

async fn provider_templates(
    State(app_state): State<Arc<WebAppState>>,
    Query(query): Query<ProviderListQuery>,
) -> Result<Json<Value>, ApiError> {
    let app_filter = parse_optional_app(query.app.as_deref())?;
    let mut apps_json = Vec::new();
    for app in AppType::all() {
        if app_filter.as_ref().is_some_and(|filter| filter != &app) {
            continue;
        }
        let existing_ids = ProviderService::list(&app_state.state, app.clone())?
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let templates = provider_add_template_choices(&app)
            .iter()
            .map(|choice| {
                let seed = if choice.template.is_custom() {
                    None
                } else {
                    build_provider_from_add_template(&app, choice.template, &existing_ids).ok()
                };
                json!({
                    "id": choice.template.cli_name(),
                    "label": choice.label,
                    "custom": choice.template.is_custom(),
                    "requiresSettingsPrompt": choice.template.requires_settings_prompt(),
                    "seed": seed
                })
            })
            .collect::<Vec<_>>();
        apps_json.push(json!({
            "app": app.as_str(),
            "label": app_label(&app),
            "templates": templates
        }));
    }
    Ok(Json(json!({ "apps": apps_json })))
}

fn find_provider(state: &CcSwitchState, app: &AppType, id: &str) -> Result<Provider, AppError> {
    ProviderService::get_provider(state, app.clone(), id)
}

fn validate_provider_identity(app: &AppType, provider: &Provider) -> Result<(), AppError> {
    if provider.id.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "Provider ID cannot be empty".to_string(),
        ));
    }
    if provider.name.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "Provider name cannot be empty".to_string(),
        ));
    }
    ProviderService::validate_provider_key_for_add(app, provider.id.trim())
}

fn common_config_available(state: &CcSwitchState, app: &AppType) -> Result<bool, AppError> {
    if !supports_common_config(app) {
        return Ok(false);
    }
    let common_snippet = {
        let cfg = state.config.read().map_err(AppError::from)?;
        cfg.common_config_snippets.get(app).cloned()
    };
    Ok(common_snippet_has_effective_config(
        app,
        common_snippet.as_deref(),
    ))
}

async fn fetch_models_for_provider(
    app: &AppType,
    provider: &Provider,
) -> Result<Vec<String>, AppError> {
    if matches!(app, AppType::Claude) && provider.is_codex_oauth() {
        return CodexOAuthService::get_models(
            provider
                .meta
                .as_ref()
                .and_then(|meta| meta.managed_account_id_for("codex_oauth"))
                .as_deref(),
        )
        .await
        .map(|models| models.into_iter().map(|model| model.id).collect())
        .map_err(AppError::Message);
    }

    let base_url = StreamCheckService::extract_base_url(provider, app)?
        .trim_end_matches('/')
        .to_string();
    if base_url.is_empty() {
        return Err(AppError::Message(format!(
            "No API URL configured for provider '{}'",
            provider.id
        )));
    }
    let (auth_value, strategy) = model_fetch_auth_for_provider(app, provider, &base_url)?;
    fetch_provider_models_for_tui(&base_url, auth_value.as_deref(), strategy)
        .await
        .map_err(AppError::Message)
}

fn model_fetch_auth_for_provider(
    app: &AppType,
    provider: &Provider,
    base_url: &str,
) -> Result<(Option<String>, ModelFetchStrategy), AppError> {
    match app {
        AppType::Claude => {
            let key = StreamCheckService::extract_claude_key(provider).ok_or_else(|| {
                AppError::Message(format!("Missing API key for provider '{}'", provider.id))
            })?;
            let strategy = if base_url.contains("openrouter.ai")
                || provider
                    .settings_config
                    .get("auth_mode")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        provider
                            .settings_config
                            .get("env")
                            .and_then(|env| env.get("AUTH_MODE"))
                            .and_then(Value::as_str)
                    })
                    .is_some_and(|value| value == "bearer_only")
            {
                ModelFetchStrategy::Bearer
            } else {
                ModelFetchStrategy::Anthropic
            };
            Ok((Some(key), strategy))
        }
        AppType::Codex => Ok((
            Some(
                StreamCheckService::extract_codex_key(provider).ok_or_else(|| {
                    AppError::Message(format!("Missing API key for provider '{}'", provider.id))
                })?,
            ),
            ModelFetchStrategy::Bearer,
        )),
        AppType::Gemini => {
            let (auth_value, strategy) = extract_gemini_model_fetch_auth(provider)?;
            Ok((Some(auth_value), strategy))
        }
        AppType::OpenCode => Ok((
            Some(
                provider
                    .settings_config
                    .get("options")
                    .and_then(|options| options.get("apiKey"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        AppError::Message(format!("Missing API key for provider '{}'", provider.id))
                    })?,
            ),
            ModelFetchStrategy::Bearer,
        )),
        AppType::Hermes => Ok((
            Some(
                provider
                    .settings_config
                    .get("apiKey")
                    .or_else(|| provider.settings_config.get("api_key"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        AppError::Message(format!("Missing API key for provider '{}'", provider.id))
                    })?,
            ),
            ModelFetchStrategy::Bearer,
        )),
        AppType::OpenClaw => Ok((
            Some(
                provider
                    .settings_config
                    .get("apiKey")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        AppError::Message(format!("Missing API key for provider '{}'", provider.id))
                    })?,
            ),
            ModelFetchStrategy::Bearer,
        )),
    }
}

fn extract_gemini_model_fetch_auth(
    provider: &Provider,
) -> Result<(String, ModelFetchStrategy), AppError> {
    let env_map = crate::gemini_config::json_to_env(&provider.settings_config)?;

    if let Some(token) = env_map
        .get("GOOGLE_ACCESS_TOKEN")
        .or_else(|| env_map.get("GEMINI_ACCESS_TOKEN"))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok((token.to_string(), ModelFetchStrategy::Bearer));
    }

    let key = env_map
        .get("GEMINI_API_KEY")
        .or_else(|| env_map.get("GOOGLE_API_KEY"))
        .or_else(|| env_map.get("API_KEY"))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::Message(format!("Missing API key for provider '{}'", provider.id))
        })?;

    if key.starts_with("ya29.") {
        return Ok((key.to_string(), ModelFetchStrategy::Bearer));
    }

    if let Some(access_token) = parse_access_token_blob(key) {
        return Ok((access_token, ModelFetchStrategy::Bearer));
    }

    Ok((key.to_string(), ModelFetchStrategy::GoogleApiKey))
}

fn parse_access_token_blob(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw.trim()).ok()?;
    value
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn quota_result_summary(result: &ProviderUsageQuota) -> (String, bool, Option<String>) {
    match result {
        ProviderUsageQuota::Subscription(quota) => subscription_quota_summary(quota),
        ProviderUsageQuota::Script(result) => script_usage_summary(result),
    }
}

fn subscription_quota_summary(
    quota: &crate::services::SubscriptionQuota,
) -> (String, bool, Option<String>) {
    match &quota.credential_status {
        CredentialStatus::NotFound => return ("not_available".to_string(), false, None),
        CredentialStatus::ParseError => {
            return (
                "credential_parse_failed".to_string(),
                false,
                quota
                    .credential_message
                    .clone()
                    .or_else(|| quota.error.clone()),
            );
        }
        CredentialStatus::Expired if !quota.success => {
            return (
                "login_expired".to_string(),
                false,
                quota
                    .credential_message
                    .clone()
                    .or_else(|| quota.error.clone()),
            );
        }
        _ => {}
    }

    if !quota.success {
        return ("query_failed".to_string(), false, quota.error.clone());
    }

    if quota.tiers.is_empty() {
        return ("not_available".to_string(), false, None);
    }

    ("ok".to_string(), true, None)
}

fn script_usage_summary(result: &UsageResult) -> (String, bool, Option<String>) {
    if !result.success {
        return ("query_failed".to_string(), false, result.error.clone());
    }

    if result.data.as_ref().is_none_or(|items| items.is_empty()) {
        return ("not_available".to_string(), false, None);
    }

    ("ok".to_string(), true, None)
}

fn default_model_fetch_strategy(app: &AppType) -> ModelFetchStrategy {
    match app {
        AppType::Claude => ModelFetchStrategy::Anthropic,
        AppType::Gemini => ModelFetchStrategy::GoogleApiKey,
        AppType::Codex | AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => {
            ModelFetchStrategy::Bearer
        }
    }
}

fn model_fetch_auth_to_strategy(auth: ModelFetchAuth) -> ModelFetchStrategy {
    match auth {
        ModelFetchAuth::Bearer => ModelFetchStrategy::Bearer,
        ModelFetchAuth::Anthropic => ModelFetchStrategy::Anthropic,
        ModelFetchAuth::GoogleApiKey => ModelFetchStrategy::GoogleApiKey,
    }
}

fn effective_usage_template_type(script: &UsageScript, provider: &Provider) -> Option<String> {
    script
        .template_type
        .as_deref()
        .map(str::trim)
        .filter(|template| !template.is_empty())
        .map(str::to_string)
        .or_else(|| infer_usage_template_type(script))
        .or_else(|| {
            default_usage_script_for_provider(provider)
                .template_type
                .filter(|template| !template.trim().is_empty())
        })
}

fn infer_usage_template_type(script: &UsageScript) -> Option<String> {
    if script
        .access_token
        .as_ref()
        .is_some_and(|value| !value.is_empty())
        || script
            .user_id
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    {
        Some("newapi".to_string())
    } else if script
        .api_key
        .as_ref()
        .is_some_and(|value| !value.is_empty())
        || script
            .base_url
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    {
        Some("general".to_string())
    } else {
        None
    }
}

fn apply_usage_credentials(script: &mut UsageScript, request: &UsageQuerySetRequest) {
    let template = script.template_type.as_deref().unwrap_or("custom");
    match template {
        "general" => {
            set_trimmed_option(&mut script.api_key, request.api_key.as_deref());
            set_trimmed_option(&mut script.base_url, request.base_url.as_deref());
            script.access_token = None;
            script.user_id = None;
            script.coding_plan_provider = None;
        }
        "newapi" => {
            set_trimmed_option(&mut script.base_url, request.base_url.as_deref());
            set_trimmed_option(&mut script.access_token, request.access_token.as_deref());
            set_trimmed_option(&mut script.user_id, request.user_id.as_deref());
            script.api_key = None;
            script.coding_plan_provider = None;
        }
        "custom" | "balance" => {
            script.api_key = None;
            script.base_url = None;
            script.access_token = None;
            script.user_id = None;
            script.coding_plan_provider = None;
        }
        _ => {}
    }
}

fn set_trimmed_option(target: &mut Option<String>, value: Option<&str>) {
    if let Some(value) = value {
        let trimmed = value.trim();
        *target = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }
}

fn resolve_export_path(output: Option<PathBuf>, provider: &Provider) -> Result<PathBuf, AppError> {
    match output {
        None => Ok(std::env::current_dir()
            .map_err(|err| AppError::Message(format!("无法获取当前工作目录: {err}")))?
            .join(".claude")
            .join("settings.local.json")),
        Some(path) => {
            let path_str = path.to_string_lossy();
            if path_str.ends_with('/') || path_str.ends_with('\\') || !path_str.ends_with(".json") {
                Ok(path.join(format!(
                    "settings-{}.json",
                    sanitize_provider_name(&provider.name)
                )))
            } else {
                Ok(path)
            }
        }
    }
}

fn extract_api_url(provider: &Provider, app: &AppType) -> Option<String> {
    match app {
        AppType::Claude => provider
            .settings_config
            .pointer("/env/ANTHROPIC_BASE_URL")
            .and_then(Value::as_str)
            .or_else(|| {
                provider
                    .settings_config
                    .get("base_url")
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                provider
                    .settings_config
                    .get("baseURL")
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                provider
                    .settings_config
                    .get("apiEndpoint")
                    .and_then(Value::as_str)
            })
            .map(str::to_string),
        AppType::Codex => {
            if let Some(url) = provider
                .settings_config
                .get("base_url")
                .and_then(Value::as_str)
                .or_else(|| {
                    provider
                        .settings_config
                        .get("baseURL")
                        .and_then(Value::as_str)
                })
            {
                return Some(url.to_string());
            }
            provider
                .settings_config
                .get("config")
                .and_then(Value::as_str)
                .and_then(extract_base_url_from_toml_text)
        }
        AppType::Gemini => provider
            .settings_config
            .pointer("/env/GOOGLE_GEMINI_BASE_URL")
            .and_then(Value::as_str)
            .or_else(|| {
                provider
                    .settings_config
                    .pointer("/env/GEMINI_BASE_URL")
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                provider
                    .settings_config
                    .pointer("/env/BASE_URL")
                    .and_then(Value::as_str)
            })
            .map(str::to_string),
        AppType::OpenCode => provider
            .settings_config
            .pointer("/options/baseURL")
            .and_then(Value::as_str)
            .map(str::to_string),
        AppType::Hermes => provider
            .settings_config
            .get("base_url")
            .or_else(|| provider.settings_config.get("baseUrl"))
            .or_else(|| provider.settings_config.get("baseURL"))
            .or_else(|| provider.settings_config.get("endpoint"))
            .and_then(Value::as_str)
            .map(str::to_string),
        AppType::OpenClaw => provider
            .settings_config
            .get("baseUrl")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
    .map(|value| value.trim_end_matches('/').to_string())
    .filter(|value| !value.is_empty())
}

fn extract_base_url_from_toml_text(config_text: &str) -> Option<String> {
    for line in config_text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("base_url") {
            continue;
        }
        let (_, value) = trimmed.split_once('=')?;
        let value = value.trim().trim_matches('"').trim_matches('\'').trim();
        if !value.is_empty() {
            return Some(value.trim_end_matches('/').to_string());
        }
    }
    None
}

fn parse_optional_app(value: Option<&str>) -> Result<Option<AppType>, ApiError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(parse_app)
        .transpose()
}

fn parse_app(value: &str) -> Result<AppType, ApiError> {
    AppType::from_str(value).map_err(ApiError::from)
}

fn app_label(app: &AppType) -> &'static str {
    match app {
        AppType::Claude => "Claude Code",
        AppType::Codex => "Codex",
        AppType::Gemini => "Gemini",
        AppType::OpenCode => "OpenCode",
        AppType::Hermes => "Hermes",
        AppType::OpenClaw => "OpenClaw",
    }
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    error: String,
}

impl From<AppError> for ApiError {
    fn from(error: AppError) -> Self {
        let status = match &error {
            AppError::InvalidInput(_) => StatusCode::BAD_REQUEST,
            AppError::Localized { key, .. } if key.contains("not_found") => StatusCode::NOT_FOUND,
            AppError::Message(message)
                if message.to_ascii_lowercase().contains("not found")
                    || message.contains("不存在") =>
            {
                StatusCode::NOT_FOUND
            }
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            error: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "ok": false,
                "error": self.error
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::UsageData;
    use crate::services::{QuotaTier, SubscriptionQuota};

    #[test]
    fn extract_codex_toml_base_url() {
        let text = r#"
model_provider = "custom"

[model_providers.custom]
base_url = "https://api.example.com/v1"
"#;

        assert_eq!(
            extract_base_url_from_toml_text(text).as_deref(),
            Some("https://api.example.com/v1")
        );
    }

    #[test]
    fn export_directory_path_appends_settings_file() {
        let provider =
            Provider::with_id("p1".to_string(), "My Provider".to_string(), json!({}), None);

        let path = resolve_export_path(Some(PathBuf::from("/tmp/out")), &provider)
            .expect("resolve export path");

        assert_eq!(path, PathBuf::from("/tmp/out/settings-My-Provider.json"));
    }

    #[test]
    fn provider_identity_rejects_empty_values() {
        let provider = Provider::with_id("".to_string(), "".to_string(), json!({}), None);

        assert!(validate_provider_identity(&AppType::Claude, &provider).is_err());
    }

    #[test]
    fn backup_token_rejects_path_segments() {
        assert!(validate_backup_token("../outside", "backup id").is_err());
        assert!(validate_backup_token("nested/path", "backup id").is_err());
        assert!(validate_backup_token("backup_20260610_120000", "backup id").is_ok());
    }

    #[test]
    fn gemini_model_fetch_auth_accepts_google_api_key_alias() {
        let provider = Provider::with_id(
            "gemini".to_string(),
            "Gemini".to_string(),
            json!({ "env": { "GOOGLE_API_KEY": " google-key " } }),
            None,
        );

        let (auth, strategy) =
            extract_gemini_model_fetch_auth(&provider).expect("extract gemini auth");

        assert_eq!(auth, "google-key");
        assert_eq!(strategy, ModelFetchStrategy::GoogleApiKey);
    }

    #[test]
    fn gemini_model_fetch_auth_treats_access_token_blob_as_bearer() {
        let provider = Provider::with_id(
            "gemini".to_string(),
            "Gemini".to_string(),
            json!({ "env": { "API_KEY": r#"{"access_token":" token-value "}"# } }),
            None,
        );

        let (auth, strategy) =
            extract_gemini_model_fetch_auth(&provider).expect("extract gemini auth");

        assert_eq!(auth, "token-value");
        assert_eq!(strategy, ModelFetchStrategy::Bearer);
    }

    #[test]
    fn provider_model_fetch_auth_requires_open_code_key() {
        let provider = Provider::with_id(
            "opencode".to_string(),
            "OpenCode".to_string(),
            json!({ "options": { "baseURL": "https://api.example.com/v1" } }),
            None,
        );

        assert!(model_fetch_auth_for_provider(
            &AppType::OpenCode,
            &provider,
            "https://api.example.com/v1"
        )
        .is_err());
    }

    #[test]
    fn quota_summary_matches_cli_subscription_states() {
        let mut quota = SubscriptionQuota {
            tool: "claude".to_string(),
            credential_status: CredentialStatus::Expired,
            credential_message: Some("expired".to_string()),
            success: false,
            tiers: vec![],
            extra_usage: None,
            error: None,
            queried_at: None,
        };

        assert_eq!(
            quota_result_summary(&ProviderUsageQuota::Subscription(quota.clone())),
            (
                "login_expired".to_string(),
                false,
                Some("expired".to_string())
            )
        );

        quota.credential_status = CredentialStatus::Valid;
        quota.success = true;
        quota.credential_message = None;
        assert_eq!(
            quota_result_summary(&ProviderUsageQuota::Subscription(quota.clone())),
            ("not_available".to_string(), false, None)
        );

        quota.tiers.push(QuotaTier {
            name: "five_hour".to_string(),
            utilization: 42.0,
            resets_at: None,
        });
        assert_eq!(
            quota_result_summary(&ProviderUsageQuota::Subscription(quota)),
            ("ok".to_string(), true, None)
        );
    }

    #[test]
    fn quota_summary_matches_cli_script_states() {
        assert_eq!(
            quota_result_summary(&ProviderUsageQuota::Script(UsageResult {
                success: false,
                data: None,
                error: Some("boom".to_string()),
            })),
            ("query_failed".to_string(), false, Some("boom".to_string()))
        );

        assert_eq!(
            quota_result_summary(&ProviderUsageQuota::Script(UsageResult {
                success: true,
                data: Some(vec![]),
                error: None,
            })),
            ("not_available".to_string(), false, None)
        );

        assert_eq!(
            quota_result_summary(&ProviderUsageQuota::Script(UsageResult {
                success: true,
                data: Some(vec![UsageData {
                    plan_name: None,
                    extra: None,
                    is_valid: None,
                    invalid_message: None,
                    total: Some(100.0),
                    used: Some(10.0),
                    remaining: Some(90.0),
                    unit: Some("credits".to_string()),
                }]),
                error: None,
            })),
            ("ok".to_string(), true, None)
        );
    }
}
