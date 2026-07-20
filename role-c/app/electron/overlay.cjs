const path = require('node:path');

function localApiOrigins(environment = process.env) {
  return new Set([
    new URL(environment.INTENT_OS_ROLE_A_URL || 'http://127.0.0.1:9477').origin,
    new URL(environment.INTENT_OS_ROLE_B_URL || 'http://127.0.0.1:9478').origin,
  ]);
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
  return {
    x: bounds.x,
    y: bounds.y,
    width: bounds.width,
    height: bounds.height,
    transparent: true,
    frame: false,
    hasShadow: false,
    alwaysOnTop: true,
    skipTaskbar: false,
    resizable: false,
    fullscreenable: false,
    backgroundColor: '#00000000',
    webPreferences: {
      preload: path.join(__dirname, 'preload.cjs'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  };
}

module.exports = { allowedLocalApiUrl, localApiOrigins, windowOptions };
