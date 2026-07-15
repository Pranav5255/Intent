const http = require("http");
const vscode = require("vscode");
const {
  documentChangePayload,
  documentPayload,
  event,
  isExcludedPath,
  localDocument,
  normaliseContentChange,
  splitDocumentChanges,
  workspaceForPath
} = require("./events");

const EDIT_DEBOUNCE_MS = 500;
const MAX_PENDING_EVENTS = 100;
const CONFIG_CACHE_MS = 30_000;

function endpoint() {
  return vscode.workspace.getConfiguration("intentOS").get("endpoint");
}

function enabled() {
  return vscode.workspace.getConfiguration("intentOS").get("enabled");
}

function detailedCaptureRequested() {
  return vscode.workspace.getConfiguration("intentOS").get("detailedCapture");
}

function post(url, payload) {
  return new Promise((resolve, reject) => {
    const target = new URL(url);
    const body = JSON.stringify(payload);
    const request = http.request(
      {
        protocol: target.protocol,
        hostname: target.hostname,
        port: target.port,
        path: target.pathname + target.search,
        method: "POST",
        headers: { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(body) },
        timeout: 1000
      },
      (response) => {
        response.resume();
        response.statusCode >= 200 && response.statusCode < 300 ? resolve() : reject(new Error("HTTP " + response.statusCode));
      }
    );
    request.on("timeout", () => request.destroy(new Error("Intent OS request timed out")));
    request.on("error", reject);
    request.end(body);
  });
}

function getJson(url) {
  return new Promise((resolve, reject) => {
    const target = new URL(url);
    const request = http.request(
      { protocol: target.protocol, hostname: target.hostname, port: target.port, path: target.pathname + target.search, method: "GET", timeout: 1000 },
      (response) => {
        let body = "";
        response.setEncoding("utf8");
        response.on("data", (chunk) => { body += chunk; });
        response.on("end", () => {
          if (response.statusCode < 200 || response.statusCode >= 300) {
            reject(new Error("HTTP " + response.statusCode));
            return;
          }
          try {
            resolve(JSON.parse(body));
          } catch (error) {
            reject(error);
          }
        });
      }
    );
    request.on("timeout", () => request.destroy(new Error("Intent OS config request timed out")));
    request.on("error", reject);
    request.end();
  });
}

function detailedConfigEndpoint(url) {
  const target = new URL(url);
  target.pathname = target.pathname.replace(/\/event$/, "/detailed-capture/config");
  return target.toString();
}

function activate(context) {
  const pending = [];
  const edits = new Map();
  let configCache = null;
  let configFetchedAt = 0;
  let configRequest = null;

  async function getDetailedConfig() {
    if (configCache && Date.now() - configFetchedAt < CONFIG_CACHE_MS) return configCache;
    if (configRequest) return configRequest;
    configRequest = getJson(detailedConfigEndpoint(endpoint()))
      .then((value) => {
        configCache = value;
        configFetchedAt = Date.now();
        return value;
      })
      .catch(() => null)
      .finally(() => { configRequest = null; });
    return configRequest;
  }

  async function flush() {
    while (pending.length) {
      try {
        await post(endpoint(), pending[0]);
        pending.shift();
      } catch {
        return;
      }
    }
  }

  function emit(type, payload) {
    if (!enabled()) return;
    if (pending.length === MAX_PENDING_EVENTS) pending.shift();
    pending.push(event(type, payload));
    void flush();
  }

  function emitWorkspace(folder) {
    if (folder && folder.uri.scheme === "file") emit("workspace_open", { folder: folder.uri.fsPath });
  }

  async function flushEdit(key) {
    const state = edits.get(key);
    if (!state) return;
    edits.delete(key);
    emit("file_edit", documentPayload(state.document));
    if (!detailedCaptureRequested() || !state.changes.length) return;

    const config = await getDetailedConfig();
    if (!config || !config.editor || !config.editor.enabled) return;
    const workspace = workspaceForPath(state.document.uri.fsPath, config.approved_workspaces);
    if (!workspace || isExcludedPath(state.document.uri.fsPath, config.editor.excluded_patterns)) return;
    for (const changes of splitDocumentChanges(state.changes)) {
      emit("document_change", documentChangePayload(state.document, workspace, changes));
    }
  }

  function queueEdit(document, contentChanges) {
    const key = document.uri.fsPath;
    let state = edits.get(key);
    if (!state) {
      state = { document, changes: [], timer: null };
      edits.set(key, state);
    }
    state.document = document;
    if (detailedCaptureRequested()) {
      state.changes.push(...contentChanges.map(normaliseContentChange));
    }
    clearTimeout(state.timer);
    state.timer = setTimeout(() => { void flushEdit(key); }, EDIT_DEBOUNCE_MS);
  }

  for (const folder of vscode.workspace.workspaceFolders || []) emitWorkspace(folder);

  context.subscriptions.push(
    vscode.workspace.onDidChangeWorkspaceFolders((change) => change.added.forEach(emitWorkspace)),
    vscode.window.onDidChangeActiveTextEditor((editor) => {
      const document = localDocument(editor && editor.document);
      if (document) emit("file_open", documentPayload(document));
    }),
    vscode.workspace.onDidChangeTextDocument((change) => {
      const document = localDocument(change.document);
      if (!document || !change.contentChanges.length) return;
      queueEdit(document, change.contentChanges);
    }),
    vscode.workspace.onDidSaveTextDocument((document) => {
      document = localDocument(document);
      if (document) emit("file_save", { path: document.uri.fsPath });
    })
  );
}

function deactivate() {}

module.exports = { activate, deactivate, detailedConfigEndpoint };
