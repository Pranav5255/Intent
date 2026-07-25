const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('intent', {
  request: (url, init) => ipcRenderer.invoke('intent:request', { url, init }),
  setInteractionActive: (active) => ipcRenderer.send('intent:overlay-interaction', active),
  setOverlayVisible: (visible) => ipcRenderer.send('intent:overlay-visible', Boolean(visible)),
  onToggleOverlay: (listener) => {
    const wrapped = () => listener();
    ipcRenderer.on('intent:toggle-overlay', wrapped);
    return () => ipcRenderer.removeListener('intent:toggle-overlay', wrapped);
  },
  pickGeminiCredentials: () => ipcRenderer.invoke('intent:pick-gemini-credentials'),
});
