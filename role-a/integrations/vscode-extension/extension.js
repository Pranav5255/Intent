const vscode = require("vscode");

const EVENT_ENDPOINT = process.env.INTENT_OS_EVENT_ENDPOINT || "http://127.0.0.1:9477/v1/event";
const CONFIG_ENDPOINT = process.env.INTENT_OS_CONFIG_ENDPOINT || "http://127.0.0.1:9477/v1/detailed-capture/config";
const CONFIG_CACHE_MS = 30_000;

let configCache = null;
let configFetchedAt = 0;
let configRequest = null;
const editTimers = new Map();
const openedWorkspaces = new Set();

function boundedText(value, maximum) {
  return typeof value === "string" ? value.replace(/\s+/g, " ").trim().slice(0, maximum) : "";
}

async function fetchConfig() {
  const now = Date.now();
  if (configCache && now - configFetchedAt < CONFIG_CACHE_MS) {
    return configCache;
  }
  if (configRequest) {
    return configRequest;
  }
  configRequest = fetch(CONFIG_ENDPOINT, { method: "GET" })
    .then((response) => response.json())
    .then((payload) => {
      configCache = payload;
      configFetchedAt = Date.now();
      configRequest = null;
      return payload;
    })
    .catch(() => {
      configRequest = null;
      return configCache || { editor: { enabled: false }, approved_workspaces: [] };
    });
  return configRequest;
}

function workspaceFolderPath(document) {
  const folder = vscode.workspace.getWorkspaceFolder(document.uri);
  return folder ? folder.uri.fsPath : "";
}

function isApprovedWorkspace(document, config) {
  const workspacePath = workspaceFolderPath(document);
  if (!workspacePath) {
    return false;
  }
  const approved = Array.isArray(config.approved_workspaces) ? config.approved_workspaces : [];
  return approved.some((item) => {
    const normalized = String(item);
    return workspacePath === normalized || workspacePath.startsWith(`${normalized}/`);
  });
}

async function postEvent(type, payload) {
  const event = {
    id: crypto.randomUUID(),
    schema_version: 1,
    ts: Math.floor(Date.now() / 1000),
    source: "vscode",
    type,
    payload,
  };
  try {
    await fetch(EVENT_ENDPOINT, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(event),
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
    clearTimeout(existing);
  }
  editTimers.set(
    pathValue,
    setTimeout(() => {
      editTimers.delete(pathValue);
      void postEvent("file_edit", { path: pathValue });
    }, 5000)
  );
}

function documentChanges(event) {
  const changes = [];
  for (const change of event.contentChanges) {
    const kind = change.rangeLength > 0 ? (change.text ? "replace" : "delete") : "insert";
    const item = {
      kind,
      range: {
        start: {
          line: change.range.start.line,
          character: change.range.start.character,
        },
        end: {
          line: change.range.end.line,
          character: change.range.end.character,
        },
      },
      removed_characters: change.rangeLength,
    };
    if (kind !== "delete") {
      item.text = change.text;
    }
    changes.push(item);
    if (changes.length >= 25) {
      break;
    }
  }
  return changes;
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
      if (document.uri.scheme !== "file") {
        return;
      }
      void postEvent("file_open", { path: document.uri.fsPath });
    }),
    vscode.workspace.onDidSaveTextDocument((document) => {
      if (document.uri.scheme !== "file") {
        return;
      }
      void postEvent("file_save", { path: document.uri.fsPath });
    }),
    vscode.workspace.onDidChangeTextDocument((event) => {
      const document = event.document;
      if (document.uri.scheme !== "file") {
        return;
      }
      const pathValue = document.uri.fsPath;
      scheduleFileEdit(pathValue);
      void fetchConfig().then((config) => {
        if (!config.editor?.enabled || !isApprovedWorkspace(document, config)) {
          return;
        }
        const changes = documentChanges(event);
        if (!changes.length) {
          return;
        }
        void postEvent("document_change", {
          path: pathValue,
          workspace: workspaceFolderPath(document),
          language: boundedText(document.languageId, 64),
          changes,
        });
      });
    })
  );
}

function deactivate() {
  for (const timer of editTimers.values()) {
    clearTimeout(timer);
  }
  editTimers.clear();
}

module.exports = {
  activate,
  deactivate,
};
