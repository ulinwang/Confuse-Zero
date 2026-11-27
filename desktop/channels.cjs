const backend = new Set([
  "desktop.get-state", "desktop.complete-onboarding", "desktop.set-port",
  "agents.scan", "agents.connect", "agents.disable", "agents.retry",
  "notifications.list", "notifications.mark-read",
  "projects.create", "keys.create", "keys.list", "keys.revoke",
  "dashboard.summary", "traces.list", "traces.get", "traces.delete",
  "sessions.list", "scores.list", "scores.put",
  "workspace.list", "workspace.save", "workspace.delete", "workspace.labels", "workspace.metrics", "workspace.export",
  "workspace.playground", "workspace.run-experiment", "workspace.evaluate",
  "topics.list", "topics.estimate", "topics.save", "topics.change", "topics.delete", "topics.result", "topics.history"
]);
const shell = new Set(["desktop.pick-directory", "desktop.copy", "desktop.hide", "desktop.quit", "desktop.uninstall"]);
module.exports = { backend, shell };
