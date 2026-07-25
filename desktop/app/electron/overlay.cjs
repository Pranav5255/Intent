const path = require('node:path');

function localApiOrigins(environment = process.env) {
  return new Set([
    new URL(environment.INTENT_CAPTURE_URL || 'http://127.0.0.1:9477').origin,
    new URL(environment.INTENT_ENGINE_URL || 'http://127.0.0.1:9478').origin,
  ]);
}

function overlayShortcut(environment = process.env) {
  return environment.INTENT_OVERLAY_SHORTCUT?.trim() || 'Control+Space';
}

function allowedLocalApiUrl(rawUrl, origins = localApiOrigins()) {
  try {
    const url = new URL(rawUrl);
    return url.protocol === 'http:' && origins.has(url.origin);
  } catch {
    return false;
  }
}

function windowOptions(bounds) {
  const width = Math.min(1120, Math.max(1, bounds.width - 64));
  const height = Math.min(760, Math.max(1, bounds.height - 64));
  return {
    x: bounds.x + Math.round((bounds.width - width) / 2),
    y: bounds.y + Math.round((bounds.height - height) / 2),
    width,
    height,
    transparent: true,
    frame: false,
    hasShadow: false,
    alwaysOnTop: true,
    skipTaskbar: false,
    resizable: false,
    fullscreenable: false,
    backgroundColor: '#00000000',
    show: false,
    webPreferences: {
      preload: path.join(__dirname, 'preload.cjs'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  };
}

module.exports = { allowedLocalApiUrl, localApiOrigins, overlayShortcut, windowOptions };
