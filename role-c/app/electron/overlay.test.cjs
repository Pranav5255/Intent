const test = require('node:test');
const assert = require('node:assert/strict');
const { allowedLocalApiUrl, localApiOrigins, overlayShortcut, windowOptions } = require('./overlay.cjs');

test('allows only the configured local Role A and Role B API origins', () => {
  const origins = localApiOrigins({
    INTENT_OS_ROLE_A_URL: 'http://127.0.0.1:9477',
    INTENT_OS_ROLE_B_URL: 'http://127.0.0.1:9478',
  });
  assert.equal(allowedLocalApiUrl('http://127.0.0.1:9477/v1/restore', origins), true);
  assert.equal(allowedLocalApiUrl('http://127.0.0.1:9478/intents/digest', origins), true);
  assert.equal(allowedLocalApiUrl('http://localhost:9478/intents/digest', origins), false);
  assert.equal(allowedLocalApiUrl('https://example.com/restore', origins), false);
  assert.equal(allowedLocalApiUrl('file:///home/pranav/private.txt', origins), false);
});

test('uses the normal shortcut unless a local test shortcut is configured', () => {
  assert.equal(overlayShortcut({}), 'Control+Space');
  assert.equal(overlayShortcut({ INTENT_OS_OVERLAY_SHORTCUT: 'Control+Shift+Space' }), 'Control+Shift+Space');
});

test('creates a transparent, non-resizable desktop-work-area overlay', () => {
  const options = windowOptions({ x: 12, y: 34, width: 1920, height: 1046 });
  assert.deepEqual(
    { x: options.x, y: options.y, width: options.width, height: options.height },
    { x: 12, y: 34, width: 1920, height: 1046 },
  );
  assert.equal(options.transparent, true);
  assert.equal(options.frame, false);
  assert.equal(options.alwaysOnTop, true);
  assert.equal(options.resizable, false);
  assert.equal(options.webPreferences.contextIsolation, true);
  assert.equal(options.webPreferences.nodeIntegration, false);
});
