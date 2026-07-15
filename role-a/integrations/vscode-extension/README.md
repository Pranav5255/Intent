# Intent OS VS Code/Cursor companion

This package always emits local workspace, active-file, edit and save metadata.
Detailed editor capture is separately opt-in: enable Intent OS editor consent
with `intent-osctl detailed editor enable`, add the workspace with
`intent-osctl workspace add <path>`, and set `intentOS.detailedCapture` to
`true` in VS Code or Cursor settings.

When all three conditions are met, the extension emits bounded
`vscode/document_change` events for approved, non-sensitive files. These
contain inserted/replacement text, source ranges, and deleted-character counts;
they never take a document snapshot or retain deleted text. The server performs
final secret redaction before storage.

Run tests with `node --test tests/*.test.js`. Build the distributable bundle
with `npx @vscode/vsce package --out dist/intent-os-vscode.vsix`; the resulting
VSIX is bundled by the Ubuntu package and installed with the local `code` CLI,
not downloaded from the Marketplace.
