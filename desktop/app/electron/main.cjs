const { app, BrowserWindow, dialog, globalShortcut, ipcMain, net, screen } = require('electron');
const fs = require('node:fs/promises');
const path = require('node:path');
const { allowedLocalApiUrl, overlayShortcut, windowOptions } = require('./overlay.cjs');

const ROLE_B_URL = 'http://127.0.0.1:9478';

let overlayWindow = null;

function setPointerPassthrough(active) {
  if (!overlayWindow || overlayWindow.isDestroyed()) return;
  overlayWindow.setIgnoreMouseEvents(!active, active ? undefined : { forward: true });
}

function setOverlayVisible(visible) {
  if (!overlayWindow || overlayWindow.isDestroyed()) return;
  if (visible) overlayWindow.show();
  else overlayWindow.hide();
}

function createOverlay() {
  const display = screen.getPrimaryDisplay();
  const bounds = display.workArea;
  overlayWindow = new BrowserWindow(windowOptions(bounds));

  overlayWindow.setAlwaysOnTop(true, 'screen-saver');
  overlayWindow.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true });
  overlayWindow.on('closed', () => {
    overlayWindow = null;
  });
  overlayWindow.once('ready-to-show', () => setPointerPassthrough(false));

  const devServer = process.env.VITE_DEV_SERVER_URL;
  if (devServer) {
    overlayWindow.loadURL(devServer);
  } else {
    overlayWindow.loadFile(path.join(__dirname, '..', 'dist', 'index.html'));
  }
}

app.whenReady().then(() => {
  ipcMain.handle('intent:request', async (_event, request) => {
    if (!request || typeof request.url !== 'string' || !allowedLocalApiUrl(request.url)) {
      throw new Error('Intent only permits requests to the local Role A and Role B services.');
    }
    const init = request.init || {};
    const response = await net.fetch(request.url, {
      method: init.method || 'GET',
      headers: init.headers || {},
      body: init.body,
    });
    return { ok: response.ok, status: response.status, body: await response.text() };
  });

  ipcMain.handle('intent:pick-gemini-credentials', async () => {
    const parent = overlayWindow && !overlayWindow.isDestroyed() ? overlayWindow : undefined;
    const result = await dialog.showOpenDialog(parent, {
      title: 'Attach Gemini service account JSON',
      properties: ['openFile'],
      filters: [{ name: 'JSON', extensions: ['json'] }],
    });
    if (result.canceled || result.filePaths.length === 0) {
      return { ok: false };
    }
    try {
      const credentials = await fs.readFile(result.filePaths[0], 'utf8');
      const response = await net.fetch(`${ROLE_B_URL}/settings/llm/gemini-credentials`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ credentials }),
      });
      if (!response.ok) {
        const body = await response.text();
        return { ok: false, error: body || `Upload failed (${response.status})` };
      }
      return { ok: true };
    } catch (error) {
      return { ok: false, error: error instanceof Error ? error.message : 'Could not attach credentials' };
    }
  });

  ipcMain.on('intent:overlay-interaction', (_event, active) => setPointerPassthrough(Boolean(active)));
  ipcMain.on('intent:overlay-visible', (_event, visible) => setOverlayVisible(Boolean(visible)));

  createOverlay();
  globalShortcut.register(overlayShortcut(), () => {
    if (!overlayWindow || overlayWindow.isDestroyed()) return;
    if (!overlayWindow.isVisible()) overlayWindow.show();
    setPointerPassthrough(true);
    overlayWindow.webContents.send('intent:toggle-overlay');
  });

  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createOverlay();
  });
});

app.on('will-quit', () => globalShortcut.unregisterAll());
