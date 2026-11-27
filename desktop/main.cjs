// The BrowserWindow lifecycle, isolated preload and trusted-sender check follow
// the MIT-licensed Confuse desktop shell (see LICENSE).
const { app, BrowserWindow, Menu, clipboard, dialog, ipcMain, net, protocol, shell } = require("electron");
const { existsSync, mkdirSync } = require("node:fs");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const { Backend } = require("./backend.cjs");
const { verifyBundle, removeInstallation } = require("./uninstall.cjs");
const channels = require("./channels.cjs");

const ui = "confuse-ui://workspace";
const renderer = path.join(__dirname, "build");
const assets = new Set(["index.html", "app.js", "app.css", "desktop.css", "langfuse.css"]);
const data = path.resolve(process.env.CONFUSE_DATA_DIR || path.join(app.getPath("appData"), "Confuse-Zero"));
const userData = path.join(data, "Desktop");
mkdirSync(userData, { recursive: true, mode: 0o700 });
app.setName("Confuse");
app.setPath("userData", userData);
protocol.registerSchemesAsPrivileged([{ scheme: "confuse-ui", privileges: { standard: true, secure: true, supportFetchAPI: true } }]);

let mainWindow;
let backend;
let quitting = false;
let didQuit = false;

function isTrustedSender(event) {
  return mainWindow && !mainWindow.isDestroyed() &&
    BrowserWindow.fromWebContents(event.sender) === mainWindow &&
    event.senderFrame === event.sender.mainFrame &&
    event.senderFrame.url === ui + "/index.html";
}

function createWindow() {
  mainWindow = new BrowserWindow({
    title: "Confuse",
    width: 1280,
    height: 840,
    minWidth: 960,
    minHeight: 640,
    show: false,
    titleBarStyle: "hiddenInset",
    backgroundColor: "#f8f8f7",
    webPreferences: {
      preload: path.join(__dirname, "preload.cjs"),
      contextIsolation: true,
      sandbox: true,
      nodeIntegration: false,
      webSecurity: true,
      allowRunningInsecureContent: false
    }
  });
  mainWindow.once("ready-to-show", () => mainWindow?.show());
  mainWindow.on("close", event => {
    if (didQuit) return;
    event.preventDefault();
    mainWindow.hide();
  });
  mainWindow.on("closed", () => { mainWindow = undefined; });
  mainWindow.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
  mainWindow.webContents.on("will-navigate", event => event.preventDefault());
  mainWindow.webContents.on("will-attach-webview", event => event.preventDefault());
  mainWindow.webContents.session.setPermissionRequestHandler((_contents, _permission, callback) => callback(false));
  mainWindow.webContents.session.setPermissionCheckHandler(() => false);
  void mainWindow.loadURL(ui + "/index.html");
}

function showMainWindow() {
  if (!mainWindow || mainWindow.isDestroyed()) {
    createWindow();
    return;
  }
  if (mainWindow.isMinimized()) mainWindow.restore();
  mainWindow.show();
  mainWindow.focus();
}

async function uninstall() {
  if (!app.isPackaged) throw new Error("Uninstall is available in the installed application");
  const bundle = path.resolve(process.execPath, "../../..");
  verifyBundle(bundle);
  await backend.request("desktop.cleanup", {});
  await backend.stop();
  didQuit = true;
  mainWindow.close();
  try {
    await removeInstallation({ bundle, data, trash: file => shell.trashItem(file) });
  } catch (error) {
    dialog.showErrorBox("Uninstall Confuse", error.message);
    throw error;
  } finally {
    app.quit();
  }
  return true;
}

async function invoke(event, message) {
  if (!isTrustedSender(event)) throw new Error("Untrusted desktop sender");
  const { channel, payload } = message ?? {};
  if (backend && channels.backend.has(channel)) return backend.request(channel, payload);
  if (!channels.shell.has(channel)) throw new Error("Unsupported desktop action");
  if (channel === "desktop.hide") {
    mainWindow.close();
    return true;
  }
  if (channel === "desktop.quit") {
    setImmediate(() => app.quit());
    return true;
  }
  if (channel === "desktop.pick-directory") {
    const result = await dialog.showOpenDialog(mainWindow, { properties: ["openDirectory"] });
    return result.canceled ? null : result.filePaths[0];
  }
  if (channel === "desktop.copy") {
    if (typeof payload?.text !== "string" || payload.text.length > 1_000_000) throw new Error("Invalid clipboard text");
    clipboard.writeText(payload.text);
    return true;
  }
  if (payload?.confirmed !== true) throw new Error("Confirm removal before uninstalling Confuse");
  return uninstall();
}

function installMenu() {
  const mac = { label: "Confuse", submenu: [
    { role: "about" },
    { label: "Connect Agents…", click: () => {
      showMainWindow();
      mainWindow.webContents.send("confuse:reopen-setup");
    } },
    { type: "separator" },
    { role: "hide" }, { role: "hideOthers" }, { role: "unhide" },
    { type: "separator" },
    { label: "Uninstall Confuse…", click: () => {
      showMainWindow();
      mainWindow.webContents.send("confuse:request-uninstall");
    } },
    { role: "quit" }
  ] };
  Menu.setApplicationMenu(Menu.buildFromTemplate([mac, { role: "editMenu" }, { role: "windowMenu" }]));
}

function setup() {
  protocol.handle("confuse-ui", request => {
    const url = new URL(request.url);
    const requested = url.pathname.slice(1);
    if (url.hostname !== "workspace" || !assets.has(requested) || !existsSync(path.join(renderer, requested))) return new Response("Not found", { status: 404 });
    return net.fetch(pathToFileURL(path.join(renderer, requested)).href);
  });
  const executable = app.isPackaged ? path.join(process.resourcesPath, "confuse-backend")
    : path.join(__dirname, "..", "target", "debug", "confuse");
  backend = new Backend(executable, data);
  ipcMain.handle("confuse:invoke", invoke);
  installMenu();
  showMainWindow();
}

if (!app.requestSingleInstanceLock()) {
  app.exit(0);
} else {
  app.whenReady().then(setup).catch(error => dialog.showErrorBox("Confuse", error.message));
  app.on("second-instance", () => { if (app.isReady()) showMainWindow(); });
  app.on("activate", () => { if (app.isReady()) showMainWindow(); });
  app.on("window-all-closed", () => {});
  app.on("before-quit", event => {
    if (didQuit || !backend) return;
    event.preventDefault();
    if (quitting) return;
    quitting = true;
    backend.stop().finally(() => {
      didQuit = true;
      app.quit();
    });
  });
}
