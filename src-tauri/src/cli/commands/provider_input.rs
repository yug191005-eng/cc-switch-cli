// Provider Add/Edit 命令的共享输入逻辑
// 提供可复用的交互式输入函数，供 add 和 edit 命令使用

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::cli::ui::info;
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta};
use crate::services::ProviderService;
use colored::Colorize;
use inquire::{Confirm, Select, Text};
use serde_json::{json, Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAddMode {
    Official,
    ThirdParty,
}

pub fn supports_common_config(app_type: &AppType) -> bool {
    matches!(app_type, AppType::Claude | AppType::Codex | AppType::Gemini)
}

pub fn common_snippet_has_effective_config(
    app_type: &AppType,
    common_snippet: Option<&str>,
) -> bool {
    if !supports_common_config(app_type) {
        return false;
    }

    let snippet = common_snippet.map(str::trim).unwrap_or_default();
    if snippet.is_empty() {
        return false;
    }

    match app_type {
        AppType::Codex => snippet
            .parse::<toml_edit::DocumentMut>()
            .ok()
            .is_some_and(|doc| doc.as_table().iter().next().is_some()),
        AppType::Claude | AppType::Gemini => serde_json::from_str::<Value>(snippet)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .is_some_and(|obj| !obj.is_empty()),
        AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => false,
    }
}

pub fn provider_uses_common_config(
    app_type: &AppType,
    provider: &Provider,
    common_snippet: Option<&str>,
) -> bool {
    if !supports_common_config(app_type) {
        return false;
    }

    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.apply_common_config)
        .unwrap_or_else(|| {
            ProviderService::provider_uses_common_config_for_app(app_type, provider, common_snippet)
        })
}

pub fn set_provider_common_config_meta(provider: &mut Provider, enabled: bool) {
    provider
        .meta
        .get_or_insert_with(ProviderMeta::default)
        .apply_common_config = Some(enabled);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_official_settings_config_uses_upstream_seed_shape() {
        let cfg = build_codex_official_settings_config(None).expect("build official settings");
        assert!(
            cfg.get("auth").is_some(),
            "official Codex provider should carry auth like upstream snapshots"
        );
        assert_eq!(cfg.get("auth"), Some(&json!({})));
        assert_eq!(cfg.get("config"), Some(&json!("")));
    }

    #[test]
    fn codex_official_settings_config_preserves_auth_and_strips_provider_config() {
        let cfg = build_codex_official_settings_config(Some(&json!({
            "auth": {
                "access_token": "oauth-token",
                "refresh_token": "refresh-token"
            },
            "config": "model_provider = \"openai\"\nmodel = \"gpt-5.4\"\nmodel_reasoning_effort = \"high\"\n\n[model_providers.openai]\nbase_url = \"https://api.openai.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
        })))
        .expect("build official settings");

        assert_eq!(
            cfg.get("auth"),
            Some(&json!({
                "access_token": "oauth-token",
                "refresh_token": "refresh-token"
            }))
        );
        assert_eq!(
            cfg.get("config").and_then(Value::as_str),
            Some("model_reasoning_effort = \"high\"")
        );
    }

    #[test]
    fn build_codex_settings_config_defaults_model_to_gpt_5_4() {
        let cfg = build_codex_settings_config(
            Some("sk-test"),
            "https://api.example.com/v1",
            "",
            "responses",
            "custom",
        );

        let config = cfg
            .get("config")
            .and_then(Value::as_str)
            .expect("config should be present");
        assert!(config.contains("model = \"gpt-5.4\""));
        assert!(config.contains("base_url = \"https://api.example.com/v1\""));
    }

    #[test]
    fn common_config_helpers_detect_and_mark_supported_provider() {
        assert!(common_snippet_has_effective_config(
            &AppType::Claude,
            Some(r#"{"env":{"CC_SWITCH_SHARED":"1"}}"#)
        ));
        assert!(common_snippet_has_effective_config(
            &AppType::Codex,
            Some("model_reasoning_effort = \"high\"")
        ));
        assert!(!common_snippet_has_effective_config(
            &AppType::OpenCode,
            Some(r#"{"options":{"theme":"dark"}}"#)
        ));

        let mut provider = Provider::with_id(
            "p1".to_string(),
            "Provider One".to_string(),
            json!({"env": {}}),
            None,
        );
        set_provider_common_config_meta(&mut provider, true);
        assert_eq!(
            provider.meta.and_then(|meta| meta.apply_common_config),
            Some(true)
        );
    }

    #[test]
    fn build_hermes_settings_config_writes_upstream_snake_case_shape() {
        let cfg = build_hermes_settings_config(
            None,
            "anthropic_messages",
            " https://openrouter.ai/api/v1/// ",
            " sk-hermes ",
            json!([
                {
                    "id": "anthropic/claude-opus-4-7",
                    "name": "Claude Opus 4.7",
                    "context_length": 1000000
                }
            ]),
            "0.5",
        )
        .expect("build Hermes settings");

        assert_eq!(cfg["api_mode"], "anthropic_messages");
        assert_eq!(cfg["base_url"], "https://openrouter.ai/api/v1");
        assert_eq!(cfg["api_key"], "sk-hermes");
        assert_eq!(cfg["rate_limit_delay"], 0.5);
        assert_eq!(cfg["models"][0]["id"], "anthropic/claude-opus-4-7");
    }

    #[test]
    fn build_hermes_settings_config_removes_legacy_aliases_and_preserves_unknown_fields() {
        let cfg = build_hermes_settings_config(
            Some(&json!({
                "api": "openai-completions",
                "apiMode": "bedrock_converse",
                "baseUrl": "https://legacy.example/v1",
                "baseURL": "https://legacy-upper.example/v1",
                "endpoint": "https://legacy-endpoint.example/v1",
                "apiKey": "sk-legacy",
                "auth_token": "sk-auth-token",
                "key_env": "HERMES_API_KEY",
                "models": [
                    { "id": "legacy-model" }
                ]
            })),
            "",
            "",
            "",
            json!([]),
            "",
        )
        .expect("build Hermes settings");
        let obj = cfg.as_object().expect("settings object");

        assert_eq!(
            obj.get("api_mode"),
            Some(&json!(crate::hermes_config::HERMES_DEFAULT_API_MODE))
        );
        assert_eq!(obj.get("auth_token"), Some(&json!("sk-auth-token")));
        assert_eq!(obj.get("key_env"), Some(&json!("HERMES_API_KEY")));
        assert!(obj.get("base_url").is_none());
        assert!(obj.get("api_key").is_none());
        assert!(obj.get("models").is_none());
        assert!(obj.get("rate_limit_delay").is_none());
        for legacy_key in ["api", "apiMode", "baseUrl", "baseURL", "endpoint", "apiKey"] {
            assert!(
                !obj.contains_key(legacy_key),
                "Hermes save should drop legacy alias {legacy_key}"
            );
        }
    }

    #[test]
    fn build_hermes_settings_config_omits_invalid_delay_and_rejects_non_array_models() {
        let cfg = build_hermes_settings_config(None, "codex_responses", "", "", json!([]), "-1")
            .expect("build Hermes settings");
        assert_eq!(cfg["api_mode"], "codex_responses");
        assert!(cfg.get("rate_limit_delay").is_none());

        let err = build_hermes_settings_config(
            None,
            "chat_completions",
            "",
            "",
            json!({"id": "model"}),
            "",
        )
        .expect_err("non-array models should fail");
        assert!(err.to_string().contains("models"));
    }

    #[test]
    fn hermes_edit_defaults_read_legacy_aliases() {
        let defaults = HermesPromptDefaults::from_settings(Some(&json!({
            "apiMode": "bedrock_converse",
            "baseUrl": "https://legacy.example/v1",
            "apiKey": "sk-legacy",
            "auth_token": "sk-auth-token",
            "models": [
                { "id": "legacy-model", "name": "Legacy Model" }
            ],
            "rate_limit_delay": 1.25
        })));

        assert_eq!(defaults.api_mode, "bedrock_converse");
        assert_eq!(defaults.base_url, "https://legacy.example/v1");
        assert_eq!(defaults.api_key, "sk-legacy");
        assert!(defaults.models_json.contains("legacy-model"));
        assert_eq!(defaults.rate_limit_delay, "1.25");
    }

    #[test]
    fn build_openclaw_settings_config_writes_canonical_shape() {
        let cfg = build_openclaw_settings_config(
            None,
            "",
            " sk-openclaw ",
            " https://api.openclaw.example/v1 ",
            true,
            json!([
                {
                    "id": "primary-model",
                    "name": "Primary Model",
                    "contextWindow": 128000
                }
            ]),
        )
        .expect("build OpenClaw settings");

        assert_eq!(
            cfg["api"],
            crate::openclaw_config::OPENCLAW_DEFAULT_API_PROTOCOL
        );
        assert_eq!(cfg["apiKey"], "sk-openclaw");
        assert_eq!(cfg["baseUrl"], "https://api.openclaw.example/v1");
        assert_eq!(
            cfg["headers"]["User-Agent"],
            crate::openclaw_config::OPENCLAW_DEFAULT_USER_AGENT
        );
        assert_eq!(cfg["models"][0]["id"], "primary-model");
    }

    #[test]
    fn build_openclaw_settings_config_removes_legacy_aliases_and_preserves_extra_headers() {
        let cfg = build_openclaw_settings_config(
            Some(&json!({
                "api_key": "legacy-key",
                "base_url": "https://legacy.example/v1",
                "npm": "@legacy/package",
                "options": {
                    "apiKey": "legacy-options-key"
                },
                "headers": {
                    "User-Agent": "Existing UA",
                    "X-Test": "1"
                },
                "authHeader": true,
                "models": [
                    {
                        "id": "old-model"
                    }
                ]
            })),
            "anthropic-messages",
            "",
            "",
            false,
            json!([
                {
                    "id": "new-model",
                    "name": "New Model",
                    "context_window": 128000
                }
            ]),
        )
        .expect("build OpenClaw settings");
        let obj = cfg.as_object().expect("settings object");

        assert_eq!(obj.get("api"), Some(&json!("anthropic-messages")));
        assert_eq!(obj.get("authHeader"), Some(&json!(true)));
        assert_eq!(cfg["headers"]["X-Test"], "1");
        assert!(cfg["headers"].get("User-Agent").is_none());
        assert!(obj.get("apiKey").is_none());
        assert!(obj.get("baseUrl").is_none());
        assert!(obj.get("api_key").is_none());
        assert!(obj.get("base_url").is_none());
        assert!(obj.get("npm").is_none());
        assert!(obj.get("options").is_none());
        assert_eq!(cfg["models"][0]["id"], "new-model");
        assert!(
            cfg["models"][0].get("context_window").is_none(),
            "CLI should remove legacy OpenClaw model aliases before saving"
        );
    }

    #[test]
    fn build_openclaw_settings_config_rejects_non_array_or_empty_models() {
        let non_array_err =
            build_openclaw_settings_config(None, "", "", "", false, json!({"id": "model"}))
                .expect_err("non-array models should fail");
        assert!(non_array_err.to_string().contains("models"));

        let empty_err = build_openclaw_settings_config(None, "", "", "", false, json!([]))
            .expect_err("empty models should fail");
        assert!(empty_err.to_string().contains("models"));
    }

    #[test]
    fn openclaw_edit_defaults_read_canonical_settings() {
        let defaults = OpenClawPromptDefaults::from_settings(Some(&json!({
            "api": "openai-responses",
            "apiKey": "sk-existing",
            "baseUrl": "https://api.existing.example/v1",
            "headers": {
                "User-Agent": "Existing UA"
            },
            "models": [
                {
                    "id": "existing-model",
                    "contextWindow": 200000
                }
            ]
        })));

        assert_eq!(defaults.api, "openai-responses");
        assert_eq!(defaults.api_key, "sk-existing");
        assert_eq!(defaults.base_url, "https://api.existing.example/v1");
        assert!(defaults.user_agent_enabled);
        assert!(defaults.models_json.contains("existing-model"));
    }
}

pub fn prompt_settings_config_for_add(
    app_type: &AppType,
    mode: ProviderAddMode,
) -> Result<Value, AppError> {
    match (app_type, mode) {
        (AppType::Claude, _) => prompt_claude_config(None),
        (AppType::Codex, ProviderAddMode::Official) => prompt_codex_official_config(None),
        (AppType::Codex, ProviderAddMode::ThirdParty) => prompt_codex_config(None),
        (AppType::Gemini, _) => prompt_gemini_config(None),
        (AppType::OpenCode, _) => Ok(json!({})),
        (AppType::Hermes, _) => prompt_hermes_config(None),
        (AppType::OpenClaw, _) => prompt_openclaw_config(None),
    }
}

/// Generate a clean TOML key from a provider name/id for use in model_provider and [model_providers.<key>].
fn clean_codex_provider_key(raw: &str) -> String {
    crate::codex_config::clean_codex_provider_key(raw)
}

fn build_codex_settings_config(
    api_key: Option<&str>,
    base_url: &str,
    model: &str,
    wire_api: &str,
    provider_key: &str,
) -> Value {
    let model = if model.trim().is_empty() {
        "gpt-5.4"
    } else {
        model.trim()
    };
    let base_url = base_url.trim();
    let provider_key = clean_codex_provider_key(provider_key);

    // Align with upstream: use full config.toml format with [model_providers.<key>]
    let config_toml = [
        format!("model_provider = \"{}\"", provider_key),
        format!("model = \"{}\"", model),
        "model_reasoning_effort = \"high\"".to_string(),
        "disable_response_storage = true".to_string(),
        String::new(),
        format!("[model_providers.{}]", provider_key),
        format!("name = \"{}\"", provider_key),
        format!("base_url = \"{}\"", base_url),
        format!("wire_api = \"{}\"", wire_api),
        "requires_openai_auth = true".to_string(),
        String::new(),
    ]
    .join("\n");

    match api_key {
        Some(key) => json!({
            "auth": { "OPENAI_API_KEY": key.trim() },
            "config": config_toml
        }),
        None => json!({
            "config": config_toml
        }),
    }
}

fn build_codex_official_settings_config(current: Option<&Value>) -> Result<Value, AppError> {
    let auth = current
        .and_then(|value| value.get("auth"))
        .and_then(Value::as_object)
        .map(|value| Value::Object(value.clone()))
        .unwrap_or_else(|| json!({}));
    let config = current
        .and_then(|value| value.get("config"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let cleaned_config = crate::codex_config::strip_codex_provider_config_text(config)?;

    Ok(json!({
        "auth": auth,
        "config": cleaned_config
    }))
}

struct HermesPromptDefaults {
    api_mode: String,
    api_key: String,
    base_url: String,
    models_json: String,
    rate_limit_delay: String,
}

impl HermesPromptDefaults {
    fn from_settings(current: Option<&Value>) -> Self {
        let settings = current.and_then(Value::as_object);
        let api_mode = settings
            .and_then(|obj| obj.get("api_mode").or_else(|| obj.get("apiMode")))
            .and_then(Value::as_str)
            .map(normalize_hermes_api_mode)
            .unwrap_or_else(|| crate::hermes_config::HERMES_DEFAULT_API_MODE.to_string());
        let api_key = settings
            .and_then(|obj| {
                obj.get("api_key")
                    .or_else(|| obj.get("apiKey"))
                    .or_else(|| obj.get("auth_token"))
            })
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let base_url = settings
            .and_then(|obj| {
                obj.get("base_url")
                    .or_else(|| obj.get("baseUrl"))
                    .or_else(|| obj.get("baseURL"))
                    .or_else(|| obj.get("endpoint"))
            })
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let models_json = settings
            .and_then(|obj| obj.get("models"))
            .and_then(Value::as_array)
            .map(|models| Value::Array(models.clone()))
            .and_then(|value| serde_json::to_string(&value).ok())
            .unwrap_or_else(|| "[]".to_string());
        let rate_limit_delay = settings
            .and_then(|obj| obj.get("rate_limit_delay"))
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(|value| value.to_string())
            .unwrap_or_default();

        Self {
            api_mode,
            api_key,
            base_url,
            models_json,
            rate_limit_delay,
        }
    }
}

fn prompt_hermes_config(current: Option<&Value>) -> Result<Value, AppError> {
    println!("\n{}", texts::tui_label_app_hermes().bright_cyan().bold());

    let defaults = HermesPromptDefaults::from_settings(current);
    let api_modes = crate::hermes_config::HERMES_API_MODES
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    let api_mode_index = api_modes
        .iter()
        .position(|candidate| candidate == &defaults.api_mode)
        .unwrap_or(0);

    let api_mode = Select::new(texts::tui_label_hermes_api_mode(), api_modes)
        .with_starting_cursor(api_mode_index)
        .with_help_message(hermes_api_mode_help())
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let base_url = if defaults.base_url.is_empty() {
        Text::new(texts::tui_label_hermes_base_url())
            .with_placeholder("https://api.example.com/v1")
            .with_help_message(texts::tui_hermes_base_url_scheme())
            .prompt()
    } else {
        Text::new(texts::tui_label_hermes_base_url())
            .with_initial_value(&defaults.base_url)
            .with_help_message(texts::tui_hermes_base_url_scheme())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let api_key = if defaults.api_key.is_empty() {
        Text::new(texts::api_key_label())
            .with_placeholder("sk-...")
            .with_help_message(texts::api_key_help())
            .prompt()
    } else {
        Text::new(texts::api_key_label())
            .with_initial_value(&defaults.api_key)
            .with_help_message(texts::api_key_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let models_json = if defaults.models_json == "[]" {
        Text::new(texts::tui_label_hermes_models())
            .with_placeholder(r#"[{"id":"gpt-4.1","name":"GPT 4.1"}]"#)
            .with_help_message(hermes_models_json_help())
            .prompt()
    } else {
        Text::new(texts::tui_label_hermes_models())
            .with_initial_value(&defaults.models_json)
            .with_help_message(hermes_models_json_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;
    let models_value = parse_hermes_models_json(&models_json)?;

    let rate_limit_delay = if defaults.rate_limit_delay.is_empty() {
        Text::new(texts::tui_label_hermes_rate_limit_delay())
            .with_placeholder("0.5")
            .with_help_message(texts::tui_hint_hermes_rate_limit_delay())
            .prompt()
    } else {
        Text::new(texts::tui_label_hermes_rate_limit_delay())
            .with_initial_value(&defaults.rate_limit_delay)
            .with_help_message(texts::tui_hint_hermes_rate_limit_delay())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    build_hermes_settings_config(
        current,
        &api_mode,
        &base_url,
        &api_key,
        models_value,
        &rate_limit_delay,
    )
}

fn build_hermes_settings_config(
    current: Option<&Value>,
    api_mode: &str,
    base_url: &str,
    api_key: &str,
    models_value: Value,
    rate_limit_delay: &str,
) -> Result<Value, AppError> {
    let mut settings_obj = current
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    for legacy_key in ["api", "apiKey", "apiMode", "baseUrl", "baseURL", "endpoint"] {
        settings_obj.remove(legacy_key);
    }

    settings_obj.insert(
        "api_mode".to_string(),
        json!(normalize_hermes_api_mode(api_mode)),
    );

    let base_url = base_url.trim().trim_end_matches('/').to_string();
    set_or_remove_trimmed(&mut settings_obj, "base_url", &base_url);
    set_or_remove_trimmed(&mut settings_obj, "api_key", api_key);

    let models_value = normalize_hermes_models_value(models_value)?;
    if models_value.as_array().is_some_and(Vec::is_empty) {
        settings_obj.remove("models");
    } else {
        settings_obj.insert("models".to_string(), models_value);
    }

    set_or_remove_f64(&mut settings_obj, "rate_limit_delay", rate_limit_delay);

    Ok(Value::Object(settings_obj))
}

fn normalize_hermes_api_mode(api_mode: &str) -> String {
    let api_mode = api_mode.trim();
    if crate::hermes_config::HERMES_API_MODES
        .iter()
        .any(|candidate| *candidate == api_mode)
    {
        api_mode.to_string()
    } else {
        crate::hermes_config::HERMES_DEFAULT_API_MODE.to_string()
    }
}

fn parse_hermes_models_json(raw: &str) -> Result<Value, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(json!([]));
    }
    let value = serde_json::from_str::<Value>(trimmed)
        .map_err(|err| AppError::InvalidInput(texts::tui_toast_invalid_json(&err.to_string())))?;
    normalize_hermes_models_value(value)
}

fn normalize_hermes_models_value(value: Value) -> Result<Value, AppError> {
    if value.is_array() {
        Ok(value)
    } else {
        Err(AppError::localized(
            "provider.hermes.models.invalid",
            "Hermes 模型列表必须是 JSON 数组",
            "Hermes models must be a JSON array",
        ))
    }
}

fn hermes_api_mode_help() -> &'static str {
    if crate::cli::i18n::is_chinese() {
        "供应商 API 协议。请选择与端点匹配的格式。"
    } else {
        "Provider API protocol. Choose the format that matches your endpoint."
    }
}

fn hermes_models_json_help() -> &'static str {
    if crate::cli::i18n::is_chinese() {
        "JSON 数组；留空或 [] 表示不写入模型。"
    } else {
        "JSON array; leave empty or [] to omit models."
    }
}

struct OpenClawPromptDefaults {
    api: String,
    api_key: String,
    base_url: String,
    user_agent_enabled: bool,
    models_json: String,
}

impl OpenClawPromptDefaults {
    fn from_settings(current: Option<&Value>) -> Self {
        let settings = current.and_then(Value::as_object);
        let api = settings
            .and_then(|obj| obj.get("api"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(crate::openclaw_config::OPENCLAW_DEFAULT_API_PROTOCOL)
            .to_string();
        let api_key = settings
            .and_then(|obj| obj.get("apiKey"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let base_url = settings
            .and_then(|obj| obj.get("baseUrl"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let user_agent_enabled = settings
            .and_then(|obj| obj.get("headers"))
            .and_then(Value::as_object)
            .is_some_and(|headers| headers.contains_key("User-Agent"));
        let models_json = settings
            .and_then(|obj| obj.get("models"))
            .and_then(Value::as_array)
            .map(|models| Value::Array(models.clone()))
            .and_then(|value| serde_json::to_string(&value).ok())
            .unwrap_or_else(|| "[]".to_string());

        Self {
            api,
            api_key,
            base_url,
            user_agent_enabled,
            models_json,
        }
    }
}

fn prompt_openclaw_config(current: Option<&Value>) -> Result<Value, AppError> {
    println!("\n{}", texts::config_openclaw_header().bright_cyan().bold());

    let defaults = OpenClawPromptDefaults::from_settings(current);
    let mut api_protocols = crate::openclaw_config::OPENCLAW_API_PROTOCOLS
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    if !api_protocols
        .iter()
        .any(|candidate| candidate == &defaults.api)
    {
        api_protocols.insert(0, defaults.api.clone());
    }
    let api_index = api_protocols
        .iter()
        .position(|candidate| candidate == &defaults.api)
        .unwrap_or(0);

    let api = Select::new(texts::openclaw_api_protocol_label(), api_protocols)
        .with_starting_cursor(api_index)
        .with_help_message(texts::openclaw_api_protocol_help())
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let api_key = if defaults.api_key.is_empty() {
        Text::new(texts::api_key_label())
            .with_placeholder("sk-...")
            .with_help_message(texts::api_key_help())
            .prompt()
    } else {
        Text::new(texts::api_key_label())
            .with_initial_value(&defaults.api_key)
            .with_help_message(texts::api_key_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let base_url = if defaults.base_url.is_empty() {
        Text::new(texts::base_url_label())
            .with_placeholder("https://api.example.com/v1")
            .with_help_message(texts::openclaw_base_url_help())
            .prompt()
    } else {
        Text::new(texts::base_url_label())
            .with_initial_value(&defaults.base_url)
            .with_help_message(texts::openclaw_base_url_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let user_agent_enabled = Confirm::new(texts::openclaw_user_agent_prompt())
        .with_default(defaults.user_agent_enabled)
        .with_help_message(texts::openclaw_user_agent_help())
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let models_json = if defaults.models_json == "[]" {
        Text::new(texts::openclaw_models_json_label())
            .with_placeholder(r#"[{"id":"gpt-4.1","name":"GPT 4.1"}]"#)
            .with_help_message(texts::openclaw_models_json_help())
            .prompt()
    } else {
        Text::new(texts::openclaw_models_json_label())
            .with_initial_value(&defaults.models_json)
            .with_help_message(texts::openclaw_models_json_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;
    let models_value = parse_openclaw_models_json(&models_json)?;

    build_openclaw_settings_config(
        current,
        &api,
        &api_key,
        &base_url,
        user_agent_enabled,
        models_value,
    )
}

fn build_openclaw_settings_config(
    current: Option<&Value>,
    api: &str,
    api_key: &str,
    base_url: &str,
    user_agent_enabled: bool,
    models_value: Value,
) -> Result<Value, AppError> {
    let mut settings_obj = current
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    for legacy_key in ["npm", "options", "api_key", "base_url"] {
        settings_obj.remove(legacy_key);
    }

    set_or_remove_trimmed(&mut settings_obj, "apiKey", api_key);
    set_or_remove_trimmed(&mut settings_obj, "baseUrl", base_url);

    let api = api.trim();
    settings_obj.insert(
        "api".to_string(),
        json!(if api.is_empty() {
            crate::openclaw_config::OPENCLAW_DEFAULT_API_PROTOCOL
        } else {
            api
        }),
    );

    let mut headers_obj = match settings_obj.remove("headers") {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    if user_agent_enabled {
        headers_obj
            .entry("User-Agent".to_string())
            .or_insert_with(|| json!(crate::openclaw_config::OPENCLAW_DEFAULT_USER_AGENT));
    } else {
        headers_obj.remove("User-Agent");
    }
    if !headers_obj.is_empty() {
        settings_obj.insert("headers".to_string(), Value::Object(headers_obj));
    }

    let models_value = normalize_openclaw_models_value(models_value)?;
    settings_obj.insert("models".to_string(), models_value);

    serde_json::from_value::<crate::provider::OpenClawProviderConfig>(Value::Object(
        settings_obj.clone(),
    ))
    .map_err(|err| {
        AppError::localized(
            "provider.openclaw.settings.invalid",
            format!("OpenClaw 配置格式无效: {err}"),
            format!("OpenClaw provider schema is invalid: {err}"),
        )
    })?;

    Ok(Value::Object(settings_obj))
}

fn parse_openclaw_models_json(raw: &str) -> Result<Value, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(openclaw_models_required_error());
    }
    let value = serde_json::from_str::<Value>(trimmed)
        .map_err(|err| AppError::InvalidInput(texts::tui_toast_invalid_json(&err.to_string())))?;
    normalize_openclaw_models_value(value)
}

fn normalize_openclaw_models_value(value: Value) -> Result<Value, AppError> {
    let Some(models) = value.as_array() else {
        return Err(openclaw_models_required_error());
    };
    if models.is_empty() {
        return Err(openclaw_models_required_error());
    }

    let normalized_models = models
        .iter()
        .cloned()
        .map(remove_openclaw_model_legacy_aliases)
        .collect::<Vec<_>>();
    let normalized_value = Value::Array(normalized_models);

    serde_json::from_value::<Vec<crate::provider::OpenClawModelEntry>>(normalized_value.clone())
        .map_err(|err| {
            AppError::InvalidInput(texts::openclaw_models_invalid_schema_error(
                &err.to_string(),
            ))
        })?;

    Ok(normalized_value)
}

fn remove_openclaw_model_legacy_aliases(model: Value) -> Value {
    let Value::Object(mut model_obj) = model else {
        return model;
    };
    model_obj.remove("context_window");
    Value::Object(model_obj)
}

fn openclaw_models_required_error() -> AppError {
    AppError::localized(
        "provider.openclaw.models.missing",
        "OpenClaw 模型列表必须是非空 JSON 数组",
        "OpenClaw models must be a non-empty JSON array",
    )
}

fn set_or_remove_trimmed(settings_obj: &mut Map<String, Value>, key: &str, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        settings_obj.remove(key);
    } else {
        settings_obj.insert(key.to_string(), json!(trimmed));
    }
}

fn set_or_remove_f64(settings_obj: &mut Map<String, Value>, key: &str, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        settings_obj.remove(key);
    } else if let Ok(value) = trimmed.parse::<f64>() {
        if value.is_finite() && value >= 0.0 {
            settings_obj.insert(key.to_string(), json!(value));
        } else {
            settings_obj.remove(key);
        }
    } else {
        settings_obj.remove(key);
    }
}

/// 可选字段集合
#[derive(Default)]
pub struct OptionalFields {
    pub notes: Option<String>,
    pub icon: Option<String>,
    pub icon_color: Option<String>,
    pub sort_index: Option<usize>,
}

impl OptionalFields {
    /// 从现有 Provider 提取可选字段
    pub fn from_provider(provider: &Provider) -> Self {
        Self {
            notes: provider.notes.clone(),
            icon: provider.icon.clone(),
            icon_color: provider.icon_color.clone(),
            sort_index: provider.sort_index,
        }
    }
}

/// 生成唯一的 Provider ID
/// 基于名称转换为 kebab-case，如有冲突则追加数字后缀
pub fn generate_provider_id(name: &str, existing_ids: &[String]) -> String {
    // 转换为 kebab-case
    let base_id = name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else if c.is_whitespace() {
                '-'
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    // 检查唯一性
    if !existing_ids.contains(&base_id) {
        return base_id;
    }

    // 追加数字后缀
    let mut counter = 1;
    loop {
        let candidate = format!("{}-{}", base_id, counter);
        if !existing_ids.contains(&candidate) {
            return candidate;
        }
        counter += 1;
    }
}

/// 收集基本字段：name, website_url
pub fn prompt_basic_fields(
    current: Option<&Provider>,
) -> Result<(String, Option<String>), AppError> {
    // 供应商名称：根据上下文选择方法
    let name = if let Some(provider) = current {
        // 编辑模式：预填充当前值
        Text::new(texts::provider_name_label())
            .with_initial_value(&provider.name)
            .with_help_message(texts::provider_name_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        // 新增模式：显示示例占位符
        Text::new(texts::provider_name_label())
            .with_placeholder("OpenAI")
            .with_help_message(texts::provider_name_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::InvalidInput(
            texts::provider_name_empty_error().to_string(),
        ));
    }

    // 官网 URL：同样处理
    let website_url = if let Some(provider) = current {
        let initial = provider.website_url.as_deref().unwrap_or("");
        Text::new(texts::website_url_label())
            .with_initial_value(initial)
            .with_help_message(texts::website_url_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(texts::website_url_label())
            .with_placeholder("https://openai.com")
            .with_help_message(texts::website_url_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    let website_url = if website_url.trim().is_empty() {
        None
    } else {
        Some(website_url.trim().to_string())
    };

    Ok((name, website_url))
}

/// 根据应用类型收集 settings_config
pub fn prompt_settings_config(
    app_type: &AppType,
    current: Option<&Value>,
    codex_official: bool,
) -> Result<Value, AppError> {
    match app_type {
        AppType::Claude => prompt_claude_config(current),
        AppType::Codex => {
            if codex_official {
                return prompt_codex_official_config(current);
            }

            let has_auth = current
                .and_then(|v| v.get("auth"))
                .and_then(|v| v.as_object())
                .map(|obj| !obj.is_empty())
                .unwrap_or(false);
            let current_config_str = current
                .and_then(|v| v.get("config"))
                .and_then(|c| c.as_str());
            let mut current_base_url: Option<String> = None;
            if let Some(cfg) = current_config_str {
                if let Ok(table) = toml::from_str::<toml::Table>(cfg) {
                    current_base_url = table
                        .get("base_url")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    if current_base_url.is_none() {
                        if let (Some(model_provider), Some(model_providers)) = (
                            table.get("model_provider").and_then(|v| v.as_str()),
                            table.get("model_providers").and_then(|v| v.as_table()),
                        ) {
                            current_base_url = model_providers
                                .get(model_provider)
                                .and_then(|v| v.as_table())
                                .and_then(|t| t.get("base_url"))
                                .and_then(|v| v.as_str())
                                .map(String::from);
                        }
                    }
                }
            }

            let is_openai_official_endpoint = current_base_url
                .as_deref()
                .map(|url| url.trim_start().starts_with("https://api.openai.com"))
                .unwrap_or(false);

            if !has_auth && is_openai_official_endpoint {
                prompt_codex_official_config(current)
            } else {
                prompt_codex_config(current)
            }
        }
        AppType::Gemini => prompt_gemini_config(current),
        AppType::OpenCode => Ok(current.cloned().unwrap_or_else(|| json!({}))),
        AppType::Hermes => prompt_hermes_config(current),
        AppType::OpenClaw => prompt_openclaw_config(current),
    }
}

/// 提示用户输入单个模型字段
///
/// # 参数
/// - `field_name`: 字段显示名称（如 "默认模型"）
/// - `env_key`: 环境变量键名（如 "ANTHROPIC_MODEL"）
/// - `placeholder`: 占位符示例值
/// - `current`: 当前配置（编辑模式）
///
/// # 返回
/// - `Some(value)`: 用户输入了值或需要保留现有值
/// - `None`: 用户留空且无现有值，不应写入配置
fn prompt_model_field(
    field_name: &str,
    env_key: &str,
    placeholder: &str,
    current: Option<&Value>,
) -> Result<Option<String>, AppError> {
    // 尝试提取现有值
    let existing_value = current
        .and_then(|v| v.get("env"))
        .and_then(|e| e.get(env_key))
        .and_then(|m| m.as_str());

    let input = if let Some(existing) = existing_value {
        // 编辑模式 - 有现有值：预填充
        Text::new(&format!("{}：", field_name))
            .with_initial_value(existing)
            .with_help_message(texts::model_default_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        // 新增模式或编辑模式无现有值：占位符
        Text::new(&format!("{}：", field_name))
            .with_placeholder(placeholder)
            .with_help_message(texts::model_default_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    let trimmed = input.trim();

    if trimmed.is_empty() {
        if existing_value.is_some() {
            // 编辑模式下清空 → 移除配置
            Ok(None)
        } else {
            // 新增模式或原本无值 → 不写入
            Ok(None)
        }
    } else {
        // 有输入值
        Ok(Some(trimmed.to_string()))
    }
}

/// Claude 配置输入
fn prompt_claude_config(current: Option<&Value>) -> Result<Value, AppError> {
    println!("\n{}", texts::config_claude_header().bright_cyan().bold());

    let api_key = if let Some(current_key) = current
        .and_then(|v| v.get("env"))
        .and_then(|e| e.get("ANTHROPIC_AUTH_TOKEN"))
        .and_then(|k| k.as_str())
        .filter(|s| !s.is_empty())
    {
        // 编辑模式：显示完整 API Key 供编辑
        Text::new(texts::api_key_label())
            .with_initial_value(current_key)
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        // 新增模式：占位符示例
        Text::new(texts::api_key_label())
            .with_placeholder("sk-ant-...")
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    let base_url = if let Some(current_url) = current
        .and_then(|v| v.get("env"))
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(|u| u.as_str())
        .filter(|s| !s.is_empty())
    {
        Text::new(texts::base_url_label())
            .with_initial_value(current_url)
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(texts::base_url_label())
            .with_placeholder(texts::base_url_placeholder())
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    // 询问是否配置模型
    let config_models = Confirm::new(texts::configure_model_names_prompt())
        .with_default(false)
        .with_help_message(texts::api_key_help())
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let mut env = serde_json::Map::new();
    env.insert("ANTHROPIC_AUTH_TOKEN".to_string(), json!(api_key.trim()));
    env.insert("ANTHROPIC_BASE_URL".to_string(), json!(base_url.trim()));

    if config_models {
        // 使用新的辅助函数处理四个模型字段
        let model = prompt_model_field(
            texts::model_default_label(),
            "ANTHROPIC_MODEL",
            texts::model_sonnet_placeholder(),
            current,
        )?;

        let haiku = prompt_model_field(
            texts::model_haiku_label(),
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            texts::model_haiku_placeholder(),
            current,
        )?;

        let sonnet = prompt_model_field(
            texts::model_sonnet_label(),
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            texts::model_sonnet_placeholder(),
            current,
        )?;

        let opus = prompt_model_field(
            texts::model_opus_label(),
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            texts::model_opus_placeholder(),
            current,
        )?;

        // 条件写入：只在值存在时写入配置
        if let Some(value) = model {
            env.insert("ANTHROPIC_MODEL".to_string(), json!(value));
        }
        if let Some(value) = haiku {
            env.insert("ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(), json!(value));
        }
        if let Some(value) = sonnet {
            env.insert("ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(), json!(value));
        }
        if let Some(value) = opus {
            env.insert("ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(), json!(value));
        }
    }

    Ok(json!({ "env": env }))
}

/// Codex 配置输入（第三方/自定义：需要 API Key）
fn prompt_codex_config(current: Option<&Value>) -> Result<Value, AppError> {
    println!("\n{}", texts::config_codex_header().bright_cyan().bold());

    // 从当前配置提取值
    let current_api_key = current
        .and_then(|v| v.get("auth"))
        .and_then(|a| a.get("OPENAI_API_KEY"))
        .and_then(|k| k.as_str())
        .filter(|s| !s.is_empty());

    let current_config_str = current
        .and_then(|v| v.get("config"))
        .and_then(|c| c.as_str());

    let mut current_base_url: Option<String> = None;
    let mut current_model: Option<String> = None;
    if let Some(cfg) = current_config_str {
        if let Ok(table) = toml::from_str::<toml::Table>(cfg) {
            current_base_url = table
                .get("base_url")
                .and_then(|v| v.as_str())
                .map(String::from);
            if current_base_url.is_none() {
                // Full upstream-style config: base_url lives under model_providers.<model_provider>.
                if let (Some(model_provider), Some(model_providers)) = (
                    table.get("model_provider").and_then(|v| v.as_str()),
                    table.get("model_providers").and_then(|v| v.as_table()),
                ) {
                    current_base_url = model_providers
                        .get(model_provider)
                        .and_then(|v| v.as_table())
                        .and_then(|t| t.get("base_url"))
                        .and_then(|v| v.as_str())
                        .map(String::from);
                }
            }
            current_model = table
                .get("model")
                .and_then(|v| v.as_str())
                .map(String::from);
        }
    }

    // 1. API Key（恢复：用于旧版本 Codex 兼容性）
    let api_key = if let Some(current_key) = current_api_key {
        Text::new(texts::openai_api_key_label())
            .with_initial_value(current_key)
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(texts::openai_api_key_label())
            .with_placeholder("sk-...")
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    // 2. Base URL
    let base_url = if let Some(current) = current_base_url.as_deref() {
        Text::new(&format!("{}:", texts::tui_label_base_url()))
            .with_initial_value(current)
            .with_help_message("API endpoint (e.g., https://api.openai.com/v1)")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(&format!("{}:", texts::tui_label_base_url()))
            .with_placeholder("https://api.openai.com/v1")
            .with_help_message("API endpoint")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };
    let base_url = base_url.trim().to_string();
    if base_url.is_empty() {
        return Err(AppError::InvalidInput(
            texts::base_url_empty_error().to_string(),
        ));
    }

    // 3. Model
    let model = if let Some(current) = current_model.as_deref() {
        Text::new(&format!("{}:", texts::model_label()))
            .with_initial_value(current)
            .with_help_message("Model name (e.g., gpt-5.4, o3)")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(&format!("{}:", texts::model_label()))
            .with_placeholder("gpt-5.4")
            .with_help_message("Model name")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    Ok(build_codex_settings_config(
        Some(api_key.trim()),
        &base_url,
        model.trim(),
        "responses",
        "custom",
    ))
}

/// Codex 配置输入（官方：仍写入 provider snapshot 的 auth/config）
fn prompt_codex_official_config(current: Option<&Value>) -> Result<Value, AppError> {
    println!("\n{}", texts::config_codex_header().bright_cyan().bold());
    println!(
        "{}",
        info("OpenAI Official keeps the stored auth snapshot and uses the upstream empty official config.")
    );
    build_codex_official_settings_config(current)
}

/// Gemini 配置输入（含认证类型选择）
fn prompt_gemini_config(current: Option<&Value>) -> Result<Value, AppError> {
    println!("\n{}", texts::config_gemini_header().bright_cyan().bold());

    // 检测当前认证类型
    let current_auth_type = detect_gemini_auth_type(current);
    let default_index = match current_auth_type.as_deref() {
        Some("oauth") => 0,
        _ => 1, // 默认 Generic API Key（包括 packycode 和 generic）
    };

    let auth_options = vec![texts::google_oauth_official(), texts::generic_api_key()];

    let auth_type = Select::new(texts::auth_type_label(), auth_options.clone())
        .with_starting_cursor(default_index)
        .with_help_message(texts::select_auth_method_help())
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    // Match using the translated strings
    let google_oauth = texts::google_oauth_official();

    if auth_type == google_oauth {
        println!("{}", texts::use_google_oauth_warning().yellow());
        Ok(json!({
            "env": {},
            "config": {}
        }))
    } else {
        // Generic API Key (统一处理所有 API Key 供应商，包括 PackyCode)
        let api_key = if let Some(current_key) = current
            .and_then(|v| v.get("env"))
            .and_then(|e| e.get("GEMINI_API_KEY"))
            .and_then(|k| k.as_str())
            .filter(|s| !s.is_empty())
        {
            Text::new(texts::gemini_api_key_label())
                .with_initial_value(current_key)
                .with_help_message(texts::generic_api_key_help())
                .prompt()
                .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
        } else {
            Text::new(texts::gemini_api_key_label())
                .with_placeholder("AIza... or pk-...")
                .with_help_message(texts::generic_api_key_help())
                .prompt()
                .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
        };

        let base_url = if let Some(current_url) = current
            .and_then(|v| v.get("env"))
            .and_then(|e| e.get("GOOGLE_GEMINI_BASE_URL"))
            .and_then(|u| u.as_str())
            .filter(|s| !s.is_empty())
        {
            Text::new(texts::gemini_base_url_label())
                .with_initial_value(current_url)
                .with_help_message(texts::gemini_base_url_help())
                .prompt()
                .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
        } else {
            Text::new(texts::gemini_base_url_label())
                .with_placeholder(texts::gemini_base_url_placeholder())
                .with_help_message(texts::gemini_base_url_help())
                .prompt()
                .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
        };

        Ok(json!({
            "env": {
                "GEMINI_API_KEY": api_key.trim(),
                "GOOGLE_GEMINI_BASE_URL": base_url.trim()
            },
            "config": {}
        }))
    }
}

/// 收集可选字段
pub fn prompt_optional_fields(current: Option<&Provider>) -> Result<OptionalFields, AppError> {
    println!("\n{}", texts::optional_fields_config().bright_cyan().bold());

    let notes = if let Some(provider) = current {
        let initial = provider.notes.as_deref().unwrap_or("");
        Text::new(texts::notes_label())
            .with_initial_value(initial)
            .with_help_message(texts::notes_help_edit())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(texts::notes_label())
            .with_placeholder(texts::notes_example_placeholder())
            .with_help_message(texts::notes_help_new())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };
    let notes = if notes.trim().is_empty() {
        None
    } else {
        Some(notes.trim().to_string())
    };

    let sort_index_str = if let Some(provider) = current {
        let initial = provider
            .sort_index
            .map(|i| i.to_string())
            .unwrap_or_default();
        Text::new(texts::sort_index_label())
            .with_initial_value(&initial)
            .with_help_message(texts::sort_index_help_edit())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(texts::sort_index_label())
            .with_placeholder(texts::sort_index_placeholder())
            .with_help_message(texts::sort_index_help_new())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };
    let sort_index =
        if sort_index_str.trim().is_empty() {
            None
        } else {
            Some(sort_index_str.trim().parse::<usize>().map_err(|_| {
                AppError::InvalidInput(texts::invalid_sort_index_number().to_string())
            })?)
        };

    Ok(OptionalFields {
        notes,
        icon: None,
        icon_color: None,
        sort_index,
    })
}

/// 显示供应商配置摘要
pub fn display_provider_summary(provider: &Provider, app_type: &AppType) {
    println!(
        "\n{}",
        texts::provider_config_summary().bright_green().bold()
    );
    println!("{}: {}", texts::id_label().bright_yellow(), provider.id);
    println!(
        "{}: {}",
        texts::provider_name_label().bright_yellow(),
        provider.name
    );

    if let Some(website) = &provider.website_url {
        println!("{}: {}", texts::website_label().bright_yellow(), website);
    }
    if supports_common_config(app_type) {
        if let Some(enabled) = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.apply_common_config)
        {
            println!(
                "{}: {}",
                texts::tui_form_attach_common_config().bright_yellow(),
                enabled
            );
        }
    }

    // 显示关键配置（不显示完整 API Key）
    println!("\n{}", texts::core_config_label().bright_cyan());
    match app_type {
        AppType::Claude => {
            if let Some(env) = provider.settings_config.get("env") {
                if let Some(api_key) = env.get("ANTHROPIC_AUTH_TOKEN").and_then(|v| v.as_str()) {
                    println!(
                        "  {}: {}",
                        texts::api_key_display_label(),
                        mask_api_key(api_key)
                    );
                }
                if let Some(base_url) = env.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str()) {
                    println!("  {}: {}", texts::base_url_display_label(), base_url);
                }
                if let Some(model) = env.get("ANTHROPIC_MODEL").and_then(|v| v.as_str()) {
                    println!("  {}: {}", texts::model_label(), model);
                }
            }
        }
        AppType::Codex => {
            if let Some(auth) = provider.settings_config.get("auth") {
                if let Some(api_key) = auth.get("OPENAI_API_KEY").and_then(|v| v.as_str()) {
                    println!(
                        "  {}: {}",
                        texts::api_key_display_label(),
                        mask_api_key(api_key)
                    );
                }
            }
            if let Some(config) = provider
                .settings_config
                .get("config")
                .and_then(|v| v.as_str())
            {
                println!("  {}", texts::config_toml_lines(config.lines().count()));
            }
        }
        AppType::Gemini => {
            if let Some(env) = provider.settings_config.get("env") {
                if let Some(api_key) = env.get("GEMINI_API_KEY").and_then(|v| v.as_str()) {
                    println!(
                        "  {}: {}",
                        texts::api_key_display_label(),
                        mask_api_key(api_key)
                    );
                }
                if let Some(base_url) = env
                    .get("GOOGLE_GEMINI_BASE_URL")
                    .or_else(|| env.get("BASE_URL"))
                    .and_then(|v| v.as_str())
                {
                    println!("  {}: {}", texts::base_url_display_label(), base_url);
                }
            }
        }
        AppType::OpenCode => {
            if let Some(options) = provider.settings_config.get("options") {
                if let Some(api_key) = options.get("apiKey").and_then(|v| v.as_str()) {
                    println!(
                        "  {}: {}",
                        texts::api_key_display_label(),
                        mask_api_key(api_key)
                    );
                }
                if let Some(base_url) = options.get("baseURL").and_then(|v| v.as_str()) {
                    println!("  {}: {}", texts::base_url_display_label(), base_url);
                }
            }
            if let Some(models) = provider
                .settings_config
                .get("models")
                .and_then(|v| v.as_object())
            {
                println!("  {}: {}", texts::model_label(), models.len());
            }
        }
        AppType::Hermes => {
            if let Some(api_key) = provider
                .settings_config
                .get("apiKey")
                .or_else(|| provider.settings_config.get("api_key"))
                .and_then(|v| v.as_str())
            {
                println!(
                    "  {}: {}",
                    texts::api_key_display_label(),
                    mask_api_key(api_key)
                );
            }
            if let Some(base_url) = provider
                .settings_config
                .get("base_url")
                .or_else(|| provider.settings_config.get("baseUrl"))
                .or_else(|| provider.settings_config.get("baseURL"))
                .or_else(|| provider.settings_config.get("endpoint"))
                .and_then(|v| v.as_str())
            {
                println!("  {}: {}", texts::base_url_display_label(), base_url);
            }
            if let Some(model) = provider
                .settings_config
                .get("model")
                .and_then(|v| v.as_str())
            {
                println!("  {}: {}", texts::model_label(), model);
            } else if let Some(models) = provider
                .settings_config
                .get("models")
                .and_then(|v| v.as_object())
            {
                println!("  {}: {}", texts::model_label(), models.len());
            } else if let Some(models) = provider
                .settings_config
                .get("models")
                .and_then(|v| v.as_array())
            {
                println!("  {}: {}", texts::model_label(), models.len());
            }
        }
        AppType::OpenClaw => {
            if let Some(api_key) = provider
                .settings_config
                .get("apiKey")
                .and_then(|v| v.as_str())
            {
                println!(
                    "  {}: {}",
                    texts::api_key_display_label(),
                    mask_api_key(api_key)
                );
            }
            if let Some(base_url) = provider
                .settings_config
                .get("baseUrl")
                .and_then(|v| v.as_str())
            {
                println!("  {}: {}", texts::base_url_display_label(), base_url);
            }
            if let Some(models) = provider
                .settings_config
                .get("models")
                .and_then(|v| v.as_array())
            {
                println!("  {}: {}", texts::model_label(), models.len());
            }
        }
    }

    // 可选字段
    if provider.notes.is_some() || provider.sort_index.is_some() {
        println!("\n{}", texts::optional_fields_label().bright_cyan());
        if let Some(notes) = &provider.notes {
            println!("  {}: {}", texts::notes_label_colon(), notes);
        }
        if let Some(idx) = provider.sort_index {
            println!("  {}: {}", texts::sort_index_label_colon(), idx);
        }
    }

    println!("{}", texts::summary_divider().bright_green().bold());
}

/// 获取当前时间戳（秒）
pub fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

// ========== 辅助函数 ==========
/// 检测 Gemini 当前的认证类型
fn detect_gemini_auth_type(value: Option<&Value>) -> Option<String> {
    if let Some(env) = value.and_then(|v| v.get("env")) {
        if env.get("GEMINI_API_KEY").is_some() {
            if env
                .get("GOOGLE_GEMINI_BASE_URL")
                .and_then(|v| v.as_str())
                .map(|s| s.contains("packycode"))
                .unwrap_or(false)
            {
                return Some("packycode".to_string());
            } else {
                return Some("generic".to_string());
            }
        }
    }
    // 如果没有 API Key，假设是 OAuth
    if value
        .and_then(|v| v.get("env"))
        .map(|v| v.as_object().map(|o| o.is_empty()).unwrap_or(true))
        .unwrap_or(true)
    {
        return Some("oauth".to_string());
    }
    None
}

/// 遮蔽 API Key 显示（用于摘要显示）
fn mask_api_key(key: &str) -> String {
    if key.len() <= 8 {
        return "***".to_string();
    }
    format!("{}...{}", &key[..4], &key[key.len() - 4..])
}
