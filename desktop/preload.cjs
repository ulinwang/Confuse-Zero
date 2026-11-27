const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("confuse", {
  invoke(channel, payload = {}) {
    return ipcRenderer.invoke("confuse:invoke", { channel, payload });
  },
  onUninstallRequested(listener) {
    const callback = () => listener();
    ipcRenderer.on("confuse:request-uninstall", callback);
    return () => ipcRenderer.off("confuse:request-uninstall", callback);
  },
  onReopenSetup(listener) {
    const callback = () => listener();
    ipcRenderer.on("confuse:reopen-setup", callback);
    return () => ipcRenderer.off("confuse:reopen-setup", callback);
  }
});
