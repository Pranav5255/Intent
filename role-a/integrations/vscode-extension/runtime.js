const {
  documentChangePayload,
  event: buildEvent,
  isExcludedPath,
  normaliseContentChange,
  splitDocumentChanges,
  workspaceForPath,
} = require("./events");

const EVENT_ENDPOINT = process.env.INTENT_OS_EVENT_ENDPOINT || "http://127.0.0.1:9477/v1/event";
const CONFIG_ENDPOINT = process.env.INTENT_OS_CONFIG_ENDPOINT || "http://127.0.0.1:9477/v1/detailed-capture/config";
const CONFIG_CACHE_MS = 30_000;
const DETAILED_CAPTURE_SETTING = "detailedCapture";

function boundedText(value, maximum) {
  return typeof value === "string" ? value.replace(/\s+/g, " ").trim().slice(0, maximum) : "";
}

function createExtensionRuntime(vscode, options = {}) {
  const eventEndpoint = options.eventEndpoint || EVENT_ENDPOINT;
  const configEndpoint = options.configEndpoint || CONFIG_ENDPOINT;
  const fetchImpl = options.fetchImpl || fetch;
  const setTimer = options.setTimeoutImpl || setTimeout;
  const clearTimer = options.clearTimeoutImpl || clearTimeout;
  const now = options.now || Date.now;
  let configCache = null;
  let configFetchedAt = 0;
  let configRequest = null;
  const editTimers = new Map();
  const openedWorkspaces = new Set();

  async function fetchConfig() {
    if (configCache && now() - configFetchedAt < CONFIG_CACHE_MS) {
      return configCache;
    }
    if (configRequest) {
      return configRequest;
    }
    configRequest = fetchImpl(configEndpoint, { method: "GET" })
      .then((response) => {
        if (!response.ok) {
          throw new Error("Intent detailed-capture config is unavailable");
        }
        return response.json();
      })
      .then((payload) => {
        configCache = payload;
        configFetchedAt = now();
        configRequest = null;
        return payload;
      })
      .catch(() => {
        configRequest = null;
        return configCache || { editor: { enabled: false, excluded_patterns: [] }, approved_workspaces: [] };
      });
    return configRequest;
  }

  async function postEvent(type, payload) {
    try {
      await fetchImpl(eventEndpoint, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(buildEvent(type, payload)),
      });
    } catch {
      // Capture must never interrupt editing.
    }
  }

  function emitWorkspaceOpen(folderPath) {
    if (!folderPath || openedWorkspaces.has(folderPath)) {
      return;
    }
    openedWorkspaces.add(folderPath);
    void postEvent("workspace_open", { folder: folderPath });
  }

  function scheduleFileEdit(pathValue) {
    const existing = editTimers.get(pathValue);
    if (existing) {
      clearTimer(existing);
    }
    editTimers.set(
      pathValue,
      setTimer(() => {
        editTimers.delete(pathValue);
        void postEvent("file_edit", { path: pathValue });
      }, 5000)
    );
  }

  function approvedWorkspacePath(document, config) {
    const approved = Array.isArray(config.approved_workspaces)
      ? config.approved_workspaces.filter((item) => typeof item === "string" && item)
      : [];
    return workspaceForPath(document.uri.fsPath, approved);
  }

  function detailedCaptureEnabled(document) {
    return vscode.workspace.getConfiguration("intentOS", document.uri).get(DETAILED_CAPTURE_SETTING, false) === true;
  }

  function activate(context) {
    for (const folder of vscode.workspace.workspaceFolders || []) {
      emitWorkspaceOpen(folder.uri.fsPath);
    }

    context.subscriptions.push(
      vscode.workspace.onDidChangeWorkspaceFolders((event) => {
        for (const folder of event.added) {
          emitWorkspaceOpen(folder.uri.fsPath);
        }
      }),
      vscode.workspace.onDidOpenTextDocument((document) => {
        if (document.uri.scheme === "file") {
          void postEvent("file_open", { path: document.uri.fsPath });
        }
      }),
      vscode.workspace.onDidSaveTextDocument((document) => {
        if (document.uri.scheme === "file") {
          void postEvent("file_save", { path: document.uri.fsPath });
        }
      }),
      vscode.workspace.onDidChangeTextDocument((event) => {
        const document = event.document;
        if (document.uri.scheme !== "file") {
          return;
        }
        const pathValue = document.uri.fsPath;
        scheduleFileEdit(pathValue);
        void fetchConfig().then((config) => {
          const workspace = approvedWorkspacePath(document, config);
          const excludedPatterns = Array.isArray(config.editor?.excluded_patterns) ? config.editor.excluded_patterns : [];
          if (
            !detailedCaptureEnabled(document) ||
            !config.editor?.enabled ||
            !workspace ||
            isExcludedPath(pathValue, excludedPatterns)
          ) {
            return;
          }
          for (const changes of splitDocumentChanges(event.contentChanges.map(normaliseContentChange))) {
            const payload = documentChangePayload(document, workspace, changes);
            payload.language = boundedText(document.languageId, 64);
            void postEvent("document_change", payload);
          }
        });
      })
    );
  }

  function deactivate() {
    for (const timer of editTimers.values()) {
      clearTimer(timer);
    }
    editTimers.clear();
  }

  return { activate, deactivate };
}

module.exports = { createExtensionRuntime };
