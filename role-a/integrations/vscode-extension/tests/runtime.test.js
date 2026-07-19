const assert = require("assert/strict");
const test = require("node:test");

const { createExtensionRuntime } = require("../runtime");

function change(text, rangeLength = 0) {
  return {
    text,
    rangeLength,
    range: { start: { line: 1, character: 0 }, end: { line: 1, character: rangeLength } },
  };
}

function createHarness({ detailedCapture, config, documentPath = "/work/main.js" }) {
  const handlers = {};
  const posts = [];
  const document = { uri: { scheme: "file", fsPath: documentPath }, languageId: "javascript" };
  const vscode = {
    workspace: {
      workspaceFolders: [{ uri: { fsPath: "/work" } }],
      getConfiguration: () => ({ get: () => detailedCapture }),
      onDidChangeWorkspaceFolders: (handler) => {
        handlers.workspace = handler;
        return { dispose() {} };
      },
      onDidOpenTextDocument: (handler) => {
        handlers.open = handler;
        return { dispose() {} };
      },
      onDidSaveTextDocument: (handler) => {
        handlers.save = handler;
        return { dispose() {} };
      },
      onDidChangeTextDocument: (handler) => {
        handlers.change = handler;
        return { dispose() {} };
      },
    },
  };
  const runtime = createExtensionRuntime(vscode, {
    fetchImpl: async (url, options) => {
      if (url.includes("detailed-capture")) {
        return { ok: true, json: async () => config };
      }
      posts.push(JSON.parse(options.body));
      return { ok: true };
    },
  });
  runtime.activate({ subscriptions: [] });
  return { document, handlers, posts, runtime };
}

async function settle() {
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
}

function detailedPosts(posts) {
  return posts.filter((item) => item.type === "document_change");
}

const enabledConfig = {
  editor: { enabled: true, excluded_patterns: [] },
  approved_workspaces: ["/work"],
};

test("requires the documented editor setting before sending detailed changes", async () => {
  const harness = createHarness({ detailedCapture: false, config: enabledConfig });
  harness.handlers.change({ document: harness.document, contentChanges: [change("safe")] });
  await settle();
  assert.equal(detailedPosts(harness.posts).length, 0);
  harness.runtime.deactivate();
});

test("uses the approved config root and splits detailed changes into bounded events", async () => {
  const harness = createHarness({ detailedCapture: true, config: enabledConfig, documentPath: "/work/nested/main.js" });
  harness.handlers.change({
    document: harness.document,
    contentChanges: Array.from({ length: 26 }, () => change("a")),
  });
  await settle();
  const posts = detailedPosts(harness.posts);
  assert.equal(posts.length, 2);
  assert.equal(posts[0].schema_version, 1);
  assert.equal(posts[0].payload.workspace, "/work");
  assert.equal(posts[0].payload.changes.length, 25);
  assert.equal(posts[1].payload.changes.length, 1);
  harness.runtime.deactivate();
});

test("does not send sensitive files and bounds oversized inserted text", async () => {
  const sensitive = createHarness({ detailedCapture: true, config: enabledConfig, documentPath: "/work/.env" });
  sensitive.handlers.change({ document: sensitive.document, contentChanges: [change("SECRET=value")] });
  await settle();
  assert.equal(detailedPosts(sensitive.posts).length, 0);
  sensitive.runtime.deactivate();

  const oversized = createHarness({ detailedCapture: true, config: enabledConfig });
  oversized.handlers.change({ document: oversized.document, contentChanges: [change("x".repeat(8193))] });
  await settle();
  const item = detailedPosts(oversized.posts)[0].payload.changes[0];
  assert.equal(item.text, "[omitted: size-limit]");
  assert.equal(item.text_length, 8193);
  oversized.runtime.deactivate();
});
