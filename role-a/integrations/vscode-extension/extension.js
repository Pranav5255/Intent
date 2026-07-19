const vscode = require("vscode");
const { createExtensionRuntime } = require("./runtime");

const runtime = createExtensionRuntime(vscode);

module.exports = {
  activate: runtime.activate,
  deactivate: runtime.deactivate,
};
