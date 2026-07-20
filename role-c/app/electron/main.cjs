const { app, BrowserWindow, globalShortcut, ipcMain, net, screen } = require('electron');
const path = require('node:path');
const { allowedLocalApiUrl, windowOptions } = require('./overlay.cjs');

let overlayWindow = null;

function setPointerPassthrough(active) {
  if (!overlayWindow || overlayWindow.isDestroyed()) return;
  overlayWindow.setIgnoreMouseEvents(!active, active ? undefined : { forward: true });
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
  ipcMain.handle('intent-os:request', async (_event, request) => {
    if (!request || typeof request.url !== 'string' || !allowedLocalApiUrl(request.url)) {
      throw new Error('Intent OS only permits requests to the local Role A and Role B services.');
    }
    const init = request.init || {};
    const response = await net.fetch(request.url, {
      method: init.method || 'GET',
      headers: init.headers || {},
      body: init.body,
    });
    return { ok: response.ok, status: response.status, body: await response.text() };
  });

  ipcMain.on('intent-os:overlay-interaction', (_event, active) => setPointerPassthrough(Boolean(active)));

  createOverlay();
  globalShortcut.register('Control+Space', () => {
    if (!overlayWindow || overlayWindow.isDestroyed()) return;
    setPointerPassthrough(true);
    overlayWindow.webContents.send('intent-os:toggle-overlay');
  });

  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createOverlay();
  });
});

app.on('will-quit', () => globalShortcut.unregisterAll());
