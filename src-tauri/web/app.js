const state = {
  apps: [],
  providersByApp: new Map(),
  templatesByApp: new Map(),
  currentApp: "claude",
  selectedId: null,
  detail: null,
  usageBaseline: null,
  section: "providers",
  configData: null,
  currentCommonApp: "claude",
  updateInfo: null,
};

const $ = (id) => document.getElementById(id);

async function api(path, options = {}) {
  const init = {
    headers: { "Content-Type": "application/json" },
    ...options,
  };
  const response = await fetch(path, init);
  const text = await response.text();
  const data = text ? JSON.parse(text) : null;
  if (!response.ok || (data && data.ok === false)) {
    throw new Error((data && data.error) || response.statusText);
  }
  return data;
}

function jsonBody(value) {
  return { body: JSON.stringify(value) };
}

function pretty(value) {
  return JSON.stringify(value ?? null, null, 2);
}

function parseJsonField(id, fallback) {
  const raw = $(id).value.trim();
  if (!raw) return fallback;
  try {
    return JSON.parse(raw);
  } catch (error) {
    throw new Error(`${id}: ${error.message}`);
  }
}

function optionalText(id) {
  const value = $(id).value.trim();
  return value ? value : null;
}

function optionalNumber(id) {
  const raw = $(id).value.trim();
  return raw === "" ? null : Number(raw);
}

function toast(message) {
  const el = $("toast");
  el.textContent = message;
  el.classList.add("show");
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => el.classList.remove("show"), 2600);
}

function showError(error) {
  toast(error.message || String(error));
}

async function init() {
  bindEvents();
  await loadApps();
  await loadTemplates();
  await loadProviders();
}

async function loadApps() {
  const data = await api("/api/apps");
  state.apps = data.apps;
  if (!state.apps.some((app) => app.id === state.currentApp)) {
    state.currentApp = state.apps[0]?.id || "claude";
  }
  if (!state.apps.some((app) => app.id === state.currentCommonApp && app.supportsCommonConfig)) {
    state.currentCommonApp = state.apps.find((app) => app.supportsCommonConfig)?.id || "claude";
  }
  renderAppTabs();
  renderCommonAppSelect();
}

async function loadTemplates() {
  const data = await api("/api/providers/templates");
  state.templatesByApp.clear();
  for (const app of data.apps) {
    state.templatesByApp.set(app.app, app.templates);
  }
}

async function loadProviders(keepSelection = true) {
  const data = await api("/api/providers");
  state.providersByApp.clear();
  for (const appData of data.apps) {
    state.providersByApp.set(appData.app, appData);
  }
  const currentData = currentAppData();
  if (!keepSelection || !currentData?.providers.some((p) => p.id === state.selectedId)) {
    state.selectedId = currentData?.providers[0]?.id || null;
  }
  renderAll();
  if (state.selectedId) {
    await selectProvider(state.selectedId);
  } else {
    state.detail = null;
    renderDetail();
  }
}

function currentAppData() {
  return state.providersByApp.get(state.currentApp);
}

function currentAppMeta() {
  return state.apps.find((app) => app.id === state.currentApp);
}

function currentProviderList() {
  return currentAppData()?.providers || [];
}

function selectedSummary() {
  return currentProviderList().find((provider) => provider.id === state.selectedId) || null;
}

function renderAll() {
  renderSectionTabs();
  renderWorkspaceVisibility();
  renderAppTabs();
  renderProviderList();
  renderPaneMeta();
}

function renderSectionTabs() {
  document.querySelectorAll("[data-section]").forEach((button) => {
    button.classList.toggle("active", button.dataset.section === state.section);
  });
}

function renderWorkspaceVisibility() {
  $("providersWorkspace").hidden = state.section !== "providers";
  $("configWorkspace").hidden = state.section !== "config";
  $("updateWorkspace").hidden = state.section !== "update";
  $("appTabs").hidden = state.section !== "providers";
}

function renderAppTabs() {
  $("appTabs").innerHTML = state.apps
    .map(
      (app) => `
        <button type="button" class="app-tab ${app.id === state.currentApp ? "active" : ""}" data-app="${escapeHtml(app.id)}">
          ${escapeHtml(app.label)}
        </button>
      `,
    )
    .join("");
}

function renderPaneMeta() {
  const data = currentAppData();
  const meta = currentAppMeta();
  $("appTitle").textContent = meta?.label || state.currentApp;
  const count = data?.providers.length || 0;
  const mode = meta?.additiveMode ? "additive" : "current";
  $("appMeta").textContent = `${count} provider${count === 1 ? "" : "s"} · ${mode}`;
}

function renderProviderList() {
  const list = currentProviderList();
  if (!list.length) {
    $("providerList").innerHTML = `<div class="empty">No providers</div>`;
    return;
  }
  $("providerList").innerHTML = list
    .map((provider) => {
      const active = provider.id === state.selectedId;
      const current = provider.current ? `<span class="status-chip current">Current</span>` : `<span class="status-chip">Ready</span>`;
      return `
        <button type="button" class="provider-item ${active ? "active" : ""}" data-provider="${escapeHtml(provider.id)}">
          <span class="provider-name">${escapeHtml(provider.name || provider.id)}</span>
          ${current}
          <span class="provider-id">${escapeHtml(provider.id)}</span>
          <span class="provider-url">${escapeHtml(provider.apiUrl || "N/A")}</span>
        </button>
      `;
    })
    .join("");
}

async function selectProvider(id) {
  state.selectedId = id;
  renderProviderList();
  const data = await api(`/api/providers/${encodeURIComponent(state.currentApp)}/${encodeURIComponent(id)}`);
  state.detail = data;
  renderDetail();
}

function renderDetail() {
  const detail = state.detail;
  const hasProvider = Boolean(detail?.provider);
  setEditorDisabled(!hasProvider);
  if (!hasProvider) {
    $("providerTitle").textContent = "Select a provider";
    $("providerSubtitle").textContent = "";
    clearProviderForm();
    clearOutputs();
    return;
  }

  const provider = detail.provider;
  $("providerTitle").textContent = provider.name || provider.id;
  $("providerSubtitle").textContent = `${detail.app} · ${provider.id}${detail.current ? " · current" : ""}`;

  $("fieldId").value = provider.id || "";
  $("fieldName").value = provider.name || "";
  $("fieldWebsite").value = provider.websiteUrl || "";
  $("fieldCategory").value = provider.category || "";
  $("fieldSort").value = provider.sortIndex ?? "";
  $("fieldIcon").value = provider.icon || "";
  $("fieldIconColor").value = provider.iconColor || "";
  $("fieldFailover").checked = Boolean(provider.inFailoverQueue);
  $("fieldNotes").value = provider.notes || "";
  $("fieldCommonConfig").checked = Boolean(detail.derived.commonConfigEnabled);
  $("commonConfigRow").style.display = detail.commonConfig.configured ? "" : "none";
  $("settingsJson").value = pretty(provider.settingsConfig || {});
  $("metaJson").value = provider.meta ? pretty(provider.meta) : "";

  $("switchBtn").disabled = detail.additiveMode || detail.current;
  $("removeLiveBtn").disabled = !detail.additiveMode;
  $("setDefaultBtn").disabled = !["hermes", "openclaw"].includes(state.currentApp);
  $("exportPath").value = "";
  renderUsageForm(provider.meta?.usageScript || null);
}

function clearProviderForm() {
  for (const id of [
    "fieldId",
    "fieldName",
    "fieldWebsite",
    "fieldCategory",
    "fieldSort",
    "fieldIcon",
    "fieldIconColor",
    "fieldNotes",
    "settingsJson",
    "metaJson",
  ]) {
    $(id).value = "";
  }
  $("fieldFailover").checked = false;
  $("fieldCommonConfig").checked = false;
}

function clearOutputs() {
  $("actionOutput").textContent = "";
  $("exportOutput").textContent = "";
}

function setEditorDisabled(disabled) {
  for (const id of [
    "switchBtn",
    "duplicateBtn",
    "saveBtn",
    "deleteBtn",
    "speedtestBtn",
    "streamCheckBtn",
    "fetchModelsBtn",
    "quotaBtn",
    "removeLiveBtn",
    "setDefaultBtn",
    "previewExportBtn",
    "writeExportBtn",
    "saveUsageBtn",
    "clearUsageBtn",
  ]) {
    $(id).disabled = disabled;
  }
}

function collectProviderFromForm() {
  const current = state.detail?.provider || {};
  const provider = {
    id: $("fieldId").value.trim(),
    name: $("fieldName").value.trim(),
    settingsConfig: parseJsonField("settingsJson", {}),
    inFailoverQueue: $("fieldFailover").checked,
  };
  assignOptional(provider, "websiteUrl", optionalText("fieldWebsite"));
  assignOptional(provider, "category", optionalText("fieldCategory"));
  assignOptional(provider, "createdAt", current.createdAt ?? null);
  assignOptional(provider, "sortIndex", optionalNumber("fieldSort"));
  assignOptional(provider, "notes", optionalText("fieldNotes"));
  assignOptional(provider, "icon", optionalText("fieldIcon"));
  assignOptional(provider, "iconColor", optionalText("fieldIconColor"));
  assignOptional(provider, "meta", parseJsonField("metaJson", null));
  return provider;
}

function assignOptional(target, key, value) {
  if (value !== null && value !== undefined && value !== "") {
    target[key] = value;
  }
}

async function saveProvider() {
  const provider = collectProviderFromForm();
  const body = {
    provider,
  };
  if (state.detail?.commonConfig?.configured) {
    body.commonConfigEnabled = $("fieldCommonConfig").checked;
  }
  await api(`/api/providers/${encodeURIComponent(state.currentApp)}/${encodeURIComponent(provider.id)}`, {
    method: "PUT",
    ...jsonBody(body),
  });
  toast("Saved");
  await loadProviders(true);
}

async function switchProvider() {
  const id = state.selectedId;
  await api(`/api/providers/${encodeURIComponent(state.currentApp)}/${encodeURIComponent(id)}/switch`, { method: "POST" });
  toast("Switched");
  await loadProviders(true);
}

async function duplicateProvider() {
  const id = state.selectedId;
  const data = await api(`/api/providers/${encodeURIComponent(state.currentApp)}/${encodeURIComponent(id)}/duplicate`, {
    method: "POST",
    ...jsonBody({}),
  });
  toast("Duplicated");
  await loadProviders(false);
  await selectProvider(data.provider.id);
}

async function deleteProvider() {
  const id = state.selectedId;
  if (!confirm(`Delete provider ${id}?`)) return;
  await api(`/api/providers/${encodeURIComponent(state.currentApp)}/${encodeURIComponent(id)}`, { method: "DELETE" });
  toast("Deleted");
  await loadProviders(false);
}

async function importLive() {
  const data = await api(`/api/providers/${encodeURIComponent(state.currentApp)}/import-live`, { method: "POST" });
  toast(`Imported ${data.imported}`);
  await loadTemplates();
  await loadProviders(false);
}

async function removeFromConfig() {
  const id = state.selectedId;
  await api(`/api/providers/${encodeURIComponent(state.currentApp)}/${encodeURIComponent(id)}/remove-from-config`, {
    method: "POST",
  });
  toast("Removed from config");
  await loadProviders(true);
}

async function setDefault() {
  const id = state.selectedId;
  const model = optionalText("defaultModelInput");
  const data = await api(`/api/providers/${encodeURIComponent(state.currentApp)}/set-default`, {
    method: "POST",
    ...jsonBody({ providerId: id, model }),
  });
  $("actionOutput").textContent = pretty(data);
  toast("Default saved");
}

async function moveSelected(delta) {
  const list = currentProviderList();
  const index = list.findIndex((provider) => provider.id === state.selectedId);
  const next = index + delta;
  if (index < 0 || next < 0 || next >= list.length) return;
  const copy = list.slice();
  [copy[index], copy[next]] = [copy[next], copy[index]];
  const updates = copy.map((provider, sortIndex) => ({ id: provider.id, sortIndex }));
  await api(`/api/providers/${encodeURIComponent(state.currentApp)}/sort`, {
    method: "POST",
    ...jsonBody({ updates }),
  });
  await loadProviders(true);
}

async function providerAction(kind) {
  const id = state.selectedId;
  const path = `/api/providers/${encodeURIComponent(state.currentApp)}/${encodeURIComponent(id)}/${kind}`;
  const data = await api(path, { method: "POST" });
  $("actionOutput").textContent = pretty(data);
}

async function oneOffFetch() {
  const body = {
    baseUrl: $("oneOffBaseUrl").value.trim(),
    apiKey: optionalText("oneOffApiKey"),
    auth: optionalText("oneOffAuth"),
  };
  const data = await api(`/api/providers/${encodeURIComponent(state.currentApp)}/fetch-models`, {
    method: "POST",
    ...jsonBody(body),
  });
  $("actionOutput").textContent = pretty(data);
}

function renderUsageForm(script) {
  const template = script?.templateType || state.detail?.derived?.usageTemplate || "general";
  $("usageEnabled").checked = Boolean(script?.enabled);
  $("usageTemplate").value = template;
  $("usageTimeout").value = script?.timeout ?? 10;
  $("usageInterval").value = script?.autoQueryInterval ?? 5;
  $("usageApiKey").value = script?.apiKey || "";
  $("usageBaseUrl").value = script?.baseUrl || "";
  $("usageAccessToken").value = script?.accessToken || "";
  $("usageUserId").value = script?.userId || "";
  $("usageCode").value = script?.code || "";
  state.usageBaseline = {
    template,
    code: script?.code || "",
  };
}

async function saveUsage() {
  const template = $("usageTemplate").value;
  const code = $("usageCode").value;
  const body = {
    enabled: $("usageEnabled").checked,
    timeout: optionalNumber("usageTimeout"),
    autoQueryInterval: optionalNumber("usageInterval"),
    apiKey: optionalText("usageApiKey"),
    baseUrl: optionalText("usageBaseUrl"),
    accessToken: optionalText("usageAccessToken"),
    userId: optionalText("usageUserId"),
  };
  if (!state.usageBaseline || template !== state.usageBaseline.template) {
    body.template = template;
  }
  if (!state.usageBaseline || code !== state.usageBaseline.code) {
    body.code = code;
  }
  await api(`/api/providers/${encodeURIComponent(state.currentApp)}/${encodeURIComponent(state.selectedId)}/usage-query`, {
    method: "PUT",
    ...jsonBody(body),
  });
  toast("Usage query saved");
  await selectProvider(state.selectedId);
}

async function clearUsage() {
  await api(`/api/providers/${encodeURIComponent(state.currentApp)}/${encodeURIComponent(state.selectedId)}/usage-query`, {
    method: "DELETE",
  });
  toast("Usage query cleared");
  await selectProvider(state.selectedId);
}

async function exportProvider(write) {
  const output = optionalText("exportPath");
  const data = await api(`/api/providers/${encodeURIComponent(state.currentApp)}/${encodeURIComponent(state.selectedId)}/export`, {
    method: "POST",
    ...jsonBody({ output, write }),
  });
  $("exportOutput").textContent = pretty(data);
  if (write) toast("Export written");
}

async function setSection(section) {
  state.section = section;
  renderAll();
  if (section === "config") {
    await loadConfig();
  } else if (section === "update") {
    renderUpdate();
  }
}

async function loadConfig() {
  const data = await api("/api/config");
  state.configData = data;
  renderConfig();
  await loadCommonConfig();
}

function renderConfig() {
  const data = state.configData;
  if (!data) return;
  const validation = data.validation || {};
  const providerTotal = sumObjectValues(validation.providerCounts);
  const promptTotal = sumObjectValues(validation.promptCounts);
  $("configSubtitle").textContent = data.paths?.database || "";
  $("configStatus").innerHTML = statusRows([
    ["Database", validation.databaseExists ? `${formatBytes(validation.databaseBytes)} · readable` : "Missing"],
    ["Providers", providerTotal],
    ["Prompts", promptTotal],
    ["MCP", validation.mcpServers ?? 0],
    ["Skills", validation.skillsInstalled ?? 0],
    ["Backups", data.backups?.length ?? 0],
    ["Config dir", data.paths?.configDir || ""],
  ]);
  $("configJson").value = pretty(data.config || {});
  renderCommonAppSelect();
  renderBackups();
}

function renderCommonAppSelect() {
  const supported = state.apps.filter((app) => app.supportsCommonConfig);
  if (!supported.length) return;
  if (!supported.some((app) => app.id === state.currentCommonApp)) {
    state.currentCommonApp = supported[0].id;
  }
  $("commonAppSelect").innerHTML = supported
    .map((app) => `<option value="${escapeHtml(app.id)}">${escapeHtml(app.label)}</option>`)
    .join("");
  $("commonAppSelect").value = state.currentCommonApp;
}

function renderBackups() {
  const backups = state.configData?.backups || [];
  if (!backups.length) {
    $("backupList").innerHTML = `<div class="empty">No backups</div>`;
    return;
  }
  $("backupList").innerHTML = backups
    .map(
      (backup) => `
        <div class="backup-item">
          <div class="backup-name">${escapeHtml(backup.displayName || backup.id)}</div>
          <button type="button" class="danger-btn" data-restore-backup="${escapeHtml(backup.id)}">Restore</button>
          <div class="backup-meta">${escapeHtml(backup.id)} · ${escapeHtml(formatBytes(backup.bytes))}</div>
        </div>
      `,
    )
    .join("");
}

async function loadCommonConfig() {
  renderCommonAppSelect();
  const data = await api(`/api/config/common/${encodeURIComponent(state.currentCommonApp)}`);
  $("commonSnippet").value = data.snippet || "";
  $("commonSnippet").dataset.format = data.format || "json";
}

async function saveCommonConfig() {
  const data = await api(`/api/config/common/${encodeURIComponent(state.currentCommonApp)}`, {
    method: "PUT",
    ...jsonBody({ snippet: $("commonSnippet").value }),
  });
  $("commonSnippet").value = data.snippet || "";
  toast("Common config saved");
  await loadProviders(true);
  await loadConfig();
}

async function clearCommonConfig() {
  if (!confirm(`Clear common config for ${state.currentCommonApp}?`)) return;
  const data = await api(`/api/config/common/${encodeURIComponent(state.currentCommonApp)}`, {
    method: "DELETE",
  });
  $("commonSnippet").value = data.snippet || "";
  toast("Common config cleared");
  await loadProviders(true);
  await loadConfig();
}

async function createBackup() {
  const data = await api("/api/config/backups", {
    method: "POST",
    ...jsonBody({ name: optionalText("backupName") }),
  });
  state.configData.backups = data.backups || [];
  $("backupName").value = "";
  renderBackups();
  toast(data.backupId ? `Backup ${data.backupId} created` : "No database to back up");
}

async function restoreBackup(backupId) {
  if (!confirm(`Restore backup ${backupId}?`)) return;
  const data = await api("/api/config/restore", {
    method: "POST",
    ...jsonBody({
      backupId,
      confirm: true,
      syncLive: $("syncLiveAfterImport").checked,
    }),
  });
  $("configJson").value = pretty(data);
  toast("Backup restored");
  await refreshAfterConfigMutation();
}

async function downloadConfigExport() {
  const response = await fetch("/api/config/export-sql");
  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || response.statusText);
  }
  const blob = await response.blob();
  const disposition = response.headers.get("Content-Disposition") || "";
  const match = disposition.match(/filename="([^"]+)"/);
  const filename = match?.[1] || "cc-switch-config.sql";
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
  toast("Export downloaded");
}

async function importSql() {
  const file = $("importSqlFile").files[0];
  const sql = file ? await file.text() : $("importSqlText").value;
  if (!sql.trim()) throw new Error("SQL import body cannot be empty");
  if (!confirm("Import SQL configuration now?")) return;
  const data = await api("/api/config/import-sql", {
    method: "POST",
    ...jsonBody({
      sql,
      confirm: true,
      syncLive: $("syncLiveAfterImport").checked,
    }),
  });
  $("importSqlText").value = "";
  $("importSqlFile").value = "";
  $("configJson").value = pretty(data);
  toast("Configuration imported");
  await refreshAfterConfigMutation();
}

async function saveSnapshot() {
  const config = JSON.parse($("configJson").value);
  if (!confirm("Apply the edited JSON snapshot?")) return;
  const data = await api("/api/config/snapshot", {
    method: "POST",
    ...jsonBody({
      config,
      confirm: true,
      syncLive: $("syncLiveAfterImport").checked,
      backupName: optionalText("backupName"),
    }),
  });
  $("configJson").value = pretty(data);
  toast("Configuration snapshot applied");
  await refreshAfterConfigMutation();
}

async function syncLiveConfig() {
  const data = await api("/api/config/sync-live", { method: "POST" });
  $("configJson").value = pretty(data);
  toast("Live config synced");
}

async function refreshAfterConfigMutation() {
  await loadTemplates();
  await loadProviders(true);
  await loadConfig();
}

function renderUpdate() {
  const info = state.updateInfo;
  $("updateSubtitle").textContent = info ? `Current v${info.currentVersion}` : "";
  $("updateStatus").innerHTML = info
    ? statusRows([
        ["Current", `v${info.currentVersion}`],
        ["Target", info.targetTag],
        ["Status", updateStatusText(info)],
        ["Homebrew", info.isHomebrewManaged ? "Yes" : "No"],
      ])
    : statusRows([["Status", "Not checked"]]);
  $("applyUpdateBtn").disabled =
    !info || info.isAlreadyLatest || info.isDowngrade || info.isHomebrewManaged;
  if (info?.targetTag) {
    $("targetTagInput").value = info.targetTag;
  }
}

async function checkUpdate() {
  const data = await api("/api/update/check");
  state.updateInfo = data.update;
  renderUpdate();
  $("updateOutput").textContent = pretty(data);
}

async function applyUpdate() {
  const targetTag = optionalText("targetTagInput") || state.updateInfo?.targetTag;
  if (!targetTag) throw new Error("Version tag is required");
  if (!confirm(`Update cc-switch to ${targetTag}?`)) return;
  const data = await api("/api/update/apply", {
    method: "POST",
    ...jsonBody({ targetTag }),
  });
  $("updateOutput").textContent = pretty(data);
  toast("Update applied");
}

function updateStatusText(info) {
  if (info.isAlreadyLatest) return "Latest";
  if (info.isHomebrewManaged) return "Use Homebrew";
  if (info.isDowngrade) return "Downgrade blocked";
  return "Update available";
}

function statusRows(rows) {
  return rows
    .map(
      ([label, value]) => `
        <div class="status-label">${escapeHtml(label)}</div>
        <div class="status-value">${escapeHtml(value ?? "")}</div>
      `,
    )
    .join("");
}

function sumObjectValues(value) {
  return Object.values(value || {}).reduce((total, item) => total + Number(item || 0), 0);
}

function formatBytes(bytes) {
  if (bytes === null || bytes === undefined) return "0 B";
  const value = Number(bytes);
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MB`;
}

function openNewDialog() {
  populateTemplateSelect();
  seedNewProvider();
  $("newDialog").showModal();
}

function populateTemplateSelect() {
  const templates = state.templatesByApp.get(state.currentApp) || [];
  $("templateSelect").innerHTML = templates
    .map((template) => `<option value="${escapeHtml(template.id)}">${escapeHtml(template.label)}</option>`)
    .join("");
}

function seedNewProvider() {
  const templateId = $("templateSelect").value;
  const template = (state.templatesByApp.get(state.currentApp) || []).find((item) => item.id === templateId);
  const supportsCommonConfig = Boolean(currentAppData()?.commonConfig?.configured);
  const seed =
    template?.seed || {
      id: "",
      name: "",
      settingsConfig: {},
      inFailoverQueue: false,
    };
  $("newId").value = seed.id || "";
  $("newName").value = seed.name || "";
  $("newWebsite").value = seed.websiteUrl || "";
  $("newCategory").value = seed.category || "";
  $("newCommonConfig").checked = Boolean(seed.meta?.commonConfigEnabled);
  $("newCommonConfigRow").style.display = supportsCommonConfig ? "" : "none";
  $("newSettingsJson").value = pretty(seed.settingsConfig || {});
  $("newMetaJson").value = seed.meta ? pretty(seed.meta) : "";
}

async function createProvider() {
  const provider = {
    id: $("newId").value.trim(),
    name: $("newName").value.trim(),
    settingsConfig: parseJsonField("newSettingsJson", {}),
    inFailoverQueue: false,
  };
  assignOptional(provider, "websiteUrl", optionalText("newWebsite"));
  assignOptional(provider, "category", optionalText("newCategory"));
  assignOptional(provider, "meta", parseJsonField("newMetaJson", null));
  const template = optionalText("templateSelect");
  const data = await api("/api/providers", {
    method: "POST",
    ...jsonBody({
      app: state.currentApp,
      template,
      provider,
      commonConfigEnabled: currentAppData()?.commonConfig?.configured ? $("newCommonConfig").checked : null,
    }),
  });
  $("newDialog").close();
  toast("Created");
  await loadTemplates();
  await loadProviders(false);
  await selectProvider(data.provider.id);
}

function bindEvents() {
  $("reloadBtn").addEventListener("click", () => {
    const action =
      state.section === "config"
        ? loadConfig()
        : state.section === "update"
          ? checkUpdate()
          : loadProviders(true);
    Promise.resolve(action).catch(showError);
  });
  $("newBtn").addEventListener("click", openNewDialog);
  $("importLiveBtn").addEventListener("click", () => importLive().catch(showError));
  $("moveUpBtn").addEventListener("click", () => moveSelected(-1).catch(showError));
  $("moveDownBtn").addEventListener("click", () => moveSelected(1).catch(showError));

  $("sectionTabs").addEventListener("click", (event) => {
    const button = event.target.closest("[data-section]");
    if (!button) return;
    setSection(button.dataset.section).catch(showError);
  });

  $("appTabs").addEventListener("click", (event) => {
    const button = event.target.closest("[data-app]");
    if (!button) return;
    state.currentApp = button.dataset.app;
    state.selectedId = null;
    loadProviders(false).catch(showError);
  });

  $("providerList").addEventListener("click", (event) => {
    const button = event.target.closest("[data-provider]");
    if (!button) return;
    selectProvider(button.dataset.provider).catch(showError);
  });

  document.querySelector(".segmented").addEventListener("click", (event) => {
    const button = event.target.closest("[data-panel]");
    if (!button) return;
    document.querySelectorAll(".segment").forEach((item) => item.classList.remove("active"));
    document.querySelectorAll(".panel").forEach((item) => item.classList.remove("active"));
    button.classList.add("active");
    $(button.dataset.panel).classList.add("active");
  });

  $("saveBtn").addEventListener("click", () => saveProvider().catch(showError));
  $("switchBtn").addEventListener("click", () => switchProvider().catch(showError));
  $("duplicateBtn").addEventListener("click", () => duplicateProvider().catch(showError));
  $("deleteBtn").addEventListener("click", () => deleteProvider().catch(showError));
  $("removeLiveBtn").addEventListener("click", () => removeFromConfig().catch(showError));
  $("setDefaultBtn").addEventListener("click", () => setDefault().catch(showError));
  $("speedtestBtn").addEventListener("click", () => providerAction("speedtest").catch(showError));
  $("streamCheckBtn").addEventListener("click", () => providerAction("stream-check").catch(showError));
  $("fetchModelsBtn").addEventListener("click", () => providerAction("fetch-models").catch(showError));
  $("quotaBtn").addEventListener("click", () => providerAction("quota").catch(showError));
  $("oneOffFetchBtn").addEventListener("click", () => oneOffFetch().catch(showError));
  $("saveUsageBtn").addEventListener("click", () => saveUsage().catch(showError));
  $("clearUsageBtn").addEventListener("click", () => clearUsage().catch(showError));
  $("previewExportBtn").addEventListener("click", () => exportProvider(false).catch(showError));
  $("writeExportBtn").addEventListener("click", () => exportProvider(true).catch(showError));
  $("closeDialogBtn").addEventListener("click", () => $("newDialog").close());
  $("templateSelect").addEventListener("change", seedNewProvider);
  $("createProviderBtn").addEventListener("click", () => createProvider().catch(showError));
  $("refreshConfigBtn").addEventListener("click", () => loadConfig().catch(showError));
  $("syncLiveBtn").addEventListener("click", () => syncLiveConfig().catch(showError));
  $("commonAppSelect").addEventListener("change", () => {
    state.currentCommonApp = $("commonAppSelect").value;
    loadCommonConfig().catch(showError);
  });
  $("saveCommonBtn").addEventListener("click", () => saveCommonConfig().catch(showError));
  $("clearCommonBtn").addEventListener("click", () => clearCommonConfig().catch(showError));
  $("createBackupBtn").addEventListener("click", () => createBackup().catch(showError));
  $("backupList").addEventListener("click", (event) => {
    const button = event.target.closest("[data-restore-backup]");
    if (!button) return;
    restoreBackup(button.dataset.restoreBackup).catch(showError);
  });
  $("downloadExportBtn").addEventListener("click", () => downloadConfigExport().catch(showError));
  $("importSqlBtn").addEventListener("click", () => importSql().catch(showError));
  $("saveSnapshotBtn").addEventListener("click", () => saveSnapshot().catch(showError));
  $("checkUpdateBtn").addEventListener("click", () => checkUpdate().catch(showError));
  $("applyUpdateBtn").addEventListener("click", () => applyUpdate().catch(showError));
}

function escapeHtml(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

init().catch(showError);
