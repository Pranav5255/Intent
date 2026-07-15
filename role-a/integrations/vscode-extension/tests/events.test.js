const assert = require("assert/strict");
const test = require("node:test");
const {
  documentChangePayload,
  documentPayload,
  event,
  isExcludedPath,
  localDocument,
  normaliseContentChange,
  splitDocumentChanges,
  workspaceForPath
} = require("../events");

function change(text, rangeLength = 0) {
  return {
    text,
    rangeLength,
    range: { start: { line: 2, character: 4 }, end: { line: 2, character: 4 + rangeLength } }
  };
}

test("maps a local editor document to metadata only", () => {
  const document = { uri: { scheme: "file", fsPath: "/home/pranav/work/infra/iam.tf" }, languageId: "terraform" };
  assert.equal(localDocument(document), document);
  assert.deepEqual(documentPayload(document), { path: "/home/pranav/work/infra/iam.tf", language: "terraform" });
  assert.equal(localDocument({ uri: { scheme: "untitled" } }), null);
});

test("produces canonical VS Code event envelopes", () => {
  const result = event("file_save", { path: "/tmp/a.py" });
  assert.equal(result.source, "vscode");
  assert.equal(result.type, "file_save");
  assert.deepEqual(result.payload, { path: "/tmp/a.py" });
  assert.match(result.id, /^[0-9a-f-]{36}$/);
});

test("retains inserted text and describes replacement and deletion without document snapshots", () => {
  assert.deepEqual(normaliseContentChange(change("abc")), {
    kind: "insert",
    range: { start: { line: 2, character: 4 }, end: { line: 2, character: 4 } },
    removed_characters: 0,
    text_length: 3,
    text: "abc"
  });
  assert.equal(normaliseContentChange(change("xyz", 2)).kind, "replace");
  const deletion = normaliseContentChange(change("", 3));
  assert.equal(deletion.kind, "delete");
  assert.equal(deletion.removed_characters, 3);
  assert.equal("text" in deletion, false);
});

test("splits accumulated changes and only allows approved non-sensitive files", () => {
  const chunks = splitDocumentChanges(Array.from({ length: 26 }, () => normaliseContentChange(change("a"))));
  assert.equal(chunks.length, 2);
  assert.equal(chunks[0].length, 25);
  assert.equal(workspaceForPath("/home/pranav/work/a.py", ["/home/pranav/work"]), "/home/pranav/work");
  assert.equal(workspaceForPath("/home/pranav/other/a.py", ["/home/pranav/work"]), null);
  assert.equal(isExcludedPath("/home/pranav/work/.env", []), true);
  assert.equal(isExcludedPath("/home/pranav/work/token.txt", []), true);
  assert.equal(isExcludedPath("/home/pranav/work/main.py", []), false);

  const document = { uri: { fsPath: "/home/pranav/work/main.py" }, languageId: "python" };
  assert.deepEqual(documentChangePayload(document, "/home/pranav/work", chunks[0]).workspace, "/home/pranav/work");
});
