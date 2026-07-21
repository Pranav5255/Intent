const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('intentOS', {
  request: (url, init) => ipcRenderer.invoke('intent-os:request', { url, init }),
  setInteractionActive: (active) => ipcRenderer.send('intent-os:overlay-interaction', active),
  setOverlayVisible: (visible) => ipcRenderer.send('intent-os:overlay-visible', Boolean(visible)),
  onToggleOverlay: (listener) => {
    const wrapped = () => listener();
    ipcRenderer.on('intent-os:toggle-overlay', wrapped);
    return () => ipcRenderer.removeListener('intent-os:toggle-overlay', wrapped);
  },
});
