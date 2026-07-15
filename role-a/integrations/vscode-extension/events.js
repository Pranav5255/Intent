const { randomUUID } = require("crypto");
const path = require("path");

const MAX_CHANGES_PER_EVENT = 25;
const MAX_TEXT_BYTES_PER_EVENT = 8 * 1024;
const OMITTED_TEXT = "[omitted: size-limit]";

function event(type, payload) {
  return {
    id: randomUUID(),
    ts: Math.floor(Date.now() / 1000),
    source: "vscode",
    type,
    payload
  };
}

function localDocument(document) {
  return document && document.uri && document.uri.scheme === "file" ? document : null;
}

function documentPayload(document) {
  return { path: document.uri.fsPath, language: document.languageId || "plaintext" };
}

function workspaceForPath(filePath, workspaces) {
  const candidate = path.resolve(filePath);
  const matches = (workspaces || [])
    .map((workspace) => path.resolve(workspace))
    .filter((workspace) => candidate === workspace || candidate.startsWith(workspace + path.sep));
  return matches.sort((left, right) => right.length - left.length)[0] || null;
}

function patternMatches(value, pattern) {
  const expression = pattern
    .split("*")
    .map((part) => part.replace(/[|\\{}()[\]^$+?.]/g, "\\$&"))
    .join(".*");
  return new RegExp("^" + expression + "$", "i").test(value);
}

function isExcludedPath(filePath, patterns) {
  const normalised = filePath.replace(/\\/g, "/").toLowerCase();
  const basename = path.basename(filePath).toLowerCase();
  if (
    basename === ".env" ||
    basename.startsWith(".env.") ||
    basename.endsWith(".pem") ||
    basename.endsWith(".key") ||
    basename.startsWith("id_rsa") ||
    /(secret|credential|password|token)/.test(normalised)
  ) {
    return true;
  }
  return (patterns || []).some((pattern) => patternMatches(basename, pattern));
}

function position(value) {
  return { line: value.line, character: value.character };
}

function normaliseContentChange(change) {
  const text = change.text || "";
  const removedCharacters = Number.isInteger(change.rangeLength) ? change.rangeLength : 0;
  const kind = removedCharacters === 0 ? "insert" : text ? "replace" : "delete";
  const result = {
    kind,
    range: { start: position(change.range.start), end: position(change.range.end) },
    removed_characters: removedCharacters
  };
  if (kind !== "delete") {
    const textLength = Buffer.byteLength(text, "utf8");
    result.text_length = textLength;
    result.text = textLength > MAX_TEXT_BYTES_PER_EVENT ? OMITTED_TEXT : text;
  }
  return result;
}

function eventTextBytes(change) {
  return typeof change.text === "string" ? Buffer.byteLength(change.text, "utf8") : 0;
}

function splitDocumentChanges(changes) {
  const chunks = [];
  let chunk = [];
  let bytes = 0;
  for (const change of changes) {
    const changeBytes = eventTextBytes(change);
    if (chunk.length && (chunk.length === MAX_CHANGES_PER_EVENT || bytes + changeBytes > MAX_TEXT_BYTES_PER_EVENT)) {
      chunks.push(chunk);
      chunk = [];
      bytes = 0;
    }
    chunk.push(change);
    bytes += changeBytes;
  }
  if (chunk.length) chunks.push(chunk);
  return chunks;
}

function documentChangePayload(document, workspace, changes) {
  return {
    path: document.uri.fsPath,
    workspace,
    language: document.languageId || "plaintext",
    changes
  };
}

module.exports = {
  documentChangePayload,
  documentPayload,
  event,
  isExcludedPath,
  localDocument,
  normaliseContentChange,
  splitDocumentChanges,
  workspaceForPath
};
