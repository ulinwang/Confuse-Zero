const backend = new Set([
  "desktop.get-state", "desktop.complete-onboarding", "desktop.set-port",
  "agents.scan", "agents.connect", "agents.disable", "agents.retry",
  "notifications.list", "notifications.mark-read",
  "projects.create", "keys.create", "keys.list", "keys.revoke",
  "dashboard.summary", "traces.list", "traces.get", "traces.delete",
  "sessions.list", "scores.list", "scores.put"
]);
const shell = new Set(["desktop.pick-directory", "desktop.copy", "desktop.uninstall"]);
module.exports = { backend, shell };
