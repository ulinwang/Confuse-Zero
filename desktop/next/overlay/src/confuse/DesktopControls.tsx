import { Root as DialogState } from "@radix-ui/react-dialog";
import { Bell, Plug, RefreshCw } from "@langfuse/icons";
import { useRouter } from "next/router";
import { useCallback, useEffect, useState } from "react";
import { Button } from "@/src/components/ui/button";
import { Dialog } from "@/src/components/design-system/Dialog/Dialog";
import { type DesktopState, invoke } from "./native";

const agents = {
  "kimi-code": "Kimi Code",
  codex: "Codex",
  "claude-code": "Claude Code",
  "deepseek-harness": "DeepSeek Harness",
  pi: "Pi",
} as const;
type Agent = keyof typeof agents;
type Selection = { agent: Agent; root: string; history: boolean };
type Installation = {
  agent: Agent; executable: string | null; root: string; rootReadable: boolean;
};
type Source = { selection: Selection; pending: boolean; active: boolean };
type Notice = {
  id: string | number; title: string; message: string; source: string | null;
  level: string; createdAt: string; read: boolean;
};

export function DesktopControls({ collapsed = false }: { collapsed?: boolean }) {
  const router = useRouter();
  const project = typeof router.query.projectId === "string" ? router.query.projectId : "local";
  const [panel, setPanel] = useState<"setup" | "notifications" | "uninstall" | null>(null);
  const [firstLaunch, setFirstLaunch] = useState(false);
  const [notices, setNotices] = useState<Notice[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const refresh = useCallback(async () => {
    const items = await invoke<Notice[]>("notifications.list", { project });
    setNotices(items);
  }, [project]);

  useEffect(() => {
    if (!window.confuse) return;
    let active = true;
    invoke<DesktopState>("desktop.get-state").then(state => {
      if (active && !state.onboarded) {
        setFirstLaunch(true);
        setPanel("setup");
      }
    }).catch(error => { if (active) setError(error.message); });
    const closeSetup = window.confuse.onReopenSetup(() => setPanel("setup"));
    const closeUninstall = window.confuse.onUninstallRequested(() => setPanel("uninstall"));
    return () => { active = false; closeSetup(); closeUninstall(); };
  }, []);

  useEffect(() => {
    if (!window.confuse) return;
    let active = true;
    setNotices([]);
    const poll = async () => {
      try {
        const items = await invoke<Notice[]>("notifications.list", { project });
        if (active) setNotices(items);
      } catch (error) {
        if (active) setError((error as Error).message);
      }
    };
    void poll();
    const timer = window.setInterval(poll, 3_000);
    return () => { active = false; clearInterval(timer); };
  }, [project]);

  async function drive(action: () => Promise<unknown>) {
    setBusy(true);
    setError("");
    try {
      await action();
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }

  const unread = notices.filter(notice => !notice.read).length;
  const close = () => { if (!busy) { setPanel(null); setError(""); } };
  return (
    <div className="min-w-0 px-1">
      <Button
        variant="ghost"
        className="w-full justify-start"
        aria-label={unread ? `Notifications (${unread} unread)` : "Notifications"}
        title="Notifications"
        onClick={() => { setError(""); setPanel("notifications"); }}
      >
        <Bell size="base" />
        {!collapsed && <span>Notifications{unread ? ` (${unread})` : ""}</span>}
      </Button>
      {panel === "setup" && (
        <ConnectAgents key={project} project={project} firstLaunch={firstLaunch} onClose={() => {
          setFirstLaunch(false); close();
        }} />
      )}
      {panel === "notifications" && (
        <DialogState open onOpenChange={open => { if (!open) close(); }}>
          <Dialog title="Notifications" closeLabel="Close">
            <Dialog.Body>
              {error && <p role="alert" className="text-destructive">{error}</p>}
              {unread > 0 && <Button className="self-start" variant="secondary" disabled={busy} onClick={() => void drive(async () => {
                await invoke("notifications.mark-read", { project });
                await refresh();
              })}>Mark all read</Button>}
              {notices.length === 0 && !error && <p className="text-muted-foreground">No notifications</p>}
              <ul className="flex flex-col gap-4">
                {notices.map(notice => (
                  <li key={notice.id} className="flex flex-col gap-1">
                    <div className="flex items-baseline justify-between gap-4">
                      <strong className={notice.read ? "font-medium" : "font-bold"}>{notice.title}</strong>
                      <time dateTime={notice.createdAt} className="shrink-0 text-xs text-muted-foreground">
                        {new Date(notice.createdAt).toLocaleString()}
                      </time>
                    </div>
                    <p className="break-words text-muted-foreground">{notice.message}</p>
                    {notice.source && Object.hasOwn(agents, notice.source) && notice.level !== "success" && (
                      <Button variant="secondary" className="self-start" disabled={busy} onClick={() => void drive(async () => {
                        await invoke("agents.retry", { project, agent: notice.source });
                        await refresh();
                      })}>Retry</Button>
                    )}
                  </li>
                ))}
              </ul>
              <Button variant="ghost" className="self-start" onClick={() => setPanel("setup")}>
                <Plug size="base" /> Connect Agents
              </Button>
            </Dialog.Body>
          </Dialog>
        </DialogState>
      )}
      {panel === "uninstall" && (
        <DialogState open onOpenChange={open => { if (!open) close(); }}>
          <Dialog title="Uninstall Confuse" actions={[{
            label: "Uninstall", variant: "destructive", disabled: busy, loading: busy,
            onClick: () => void drive(() => invoke("desktop.uninstall", { confirmed: true })),
          }]}>
            <Dialog.Body>
              <p>Move Confuse to Trash and delete its local workspace and credentials?</p>
              {error && <p role="alert" className="text-destructive">{error}</p>}
            </Dialog.Body>
          </Dialog>
        </DialogState>
      )}
    </div>
  );
}

function ConnectAgents({ project, firstLaunch, onClose }: { project: string; firstLaunch: boolean; onClose: () => void }) {
  const [installations, setInstallations] = useState<Installation[]>([]);
  const [sources, setSources] = useState<Source[]>([]);
  const [selected, setSelected] = useState<Partial<Record<Agent, Selection>>>({});
  const [busy, setBusy] = useState(false);
  const [scanning, setScanning] = useState(true);
  const [error, setError] = useState("");

  const scan = useCallback(async () => {
    setScanning(true);
    setError("");
    try {
      const data = await invoke<{ installations: Installation[]; sources: Source[] }>("agents.scan", { project });
      setInstallations(data.installations);
      setSources(data.sources);
    } catch (error) {
      setError((error as Error).message);
    } finally { setScanning(false); }
  }, [project]);

  useEffect(() => { void scan(); }, [scan]);
  async function submit(selections: Selection[]) {
    setBusy(true);
    setError("");
    try {
      await invoke(firstLaunch ? "desktop.complete-onboarding" : "agents.connect", { project, selections });
      onClose();
    } catch (error) {
      setError((error as Error).message);
    } finally { setBusy(false); }
  }

  function select(agent: Agent, root: string) {
    setSelected(previous => ({ ...previous, [agent]: { agent, root, history: previous[agent]?.history ?? false } }));
  }

  const choices = Object.values(selected);
  const canClose = !busy && !firstLaunch;
  return (
    <DialogState open onOpenChange={open => { if (!open && canClose) onClose(); }}>
      <Dialog title="Connect Agents" size="md" requireExplicitClose={firstLaunch || busy} hideCloseButton={firstLaunch}
        actions={[
          ...(firstLaunch ? [{ label: "Skip", variant: "secondary" as const, disabled: busy, onClick: () => void submit([]) }] : []),
          { label: "Connect", disabled: busy || scanning || choices.length === 0, loading: busy, onClick: () => void submit(choices) },
        ]}
      >
        <Dialog.Body>
          {error && <p role="alert" className="text-destructive">{error}</p>}
          <Button variant="secondary" className="self-end" disabled={scanning || busy} onClick={() => void scan()}>
            <RefreshCw size="base" /> {scanning ? "Scanning…" : "Rescan"}
          </Button>
          {Object.entries(agents).map(([key, name]) => {
            const agent = key as Agent;
            const installation = installations.find(item => item.agent === agent);
            const source = sources.find(item => item.selection.agent === agent && item.active);
            const choice = selected[agent];
            const canSelect = Boolean(choice || installation?.rootReadable);
            const status = source?.pending ? "Importing…" : source ? "Connected" : installation?.rootReadable ? "Detected" :
              installation?.executable ? "Choose session folder" : "Undetected";
            return (
              <div key={agent} className="flex flex-wrap items-start justify-between gap-3">
                <div className="flex min-w-0 flex-1 flex-col gap-2">
                  <label className="flex items-center gap-2">
                    <input type="checkbox" disabled={busy || scanning || !canSelect} checked={!!choice} onChange={event => {
                      if (event.target.checked && installation) select(agent, installation.root);
                      else setSelected(previous => { const next = { ...previous }; delete next[agent]; return next; });
                    }} />
                    <span className="font-medium">{name}</span>
                    <span className="text-xs text-muted-foreground">{status}</span>
                  </label>
                  {(choice?.root || source?.selection.root) && (
                    <span className="truncate text-xs text-muted-foreground" title={choice?.root ?? source?.selection.root}>
                      {choice?.root ?? source?.selection.root}
                    </span>
                  )}
                  {choice && (
                    <label className="flex items-center gap-2">
                      <input type="checkbox" disabled={busy} checked={choice.history} onChange={event => {
                        setSelected(previous => ({ ...previous, [agent]: { ...choice, history: event.target.checked } }));
                      }} /> Import history
                    </label>
                  )}
                </div>
                <Button variant="secondary" disabled={busy || scanning} onClick={async () => {
                  try {
                    const root = await invoke<string | null>("desktop.pick-directory");
                    if (root) select(agent, root);
                  } catch (error) { setError((error as Error).message); }
                }}>Choose folder</Button>
                {source && <Button variant="ghost" disabled={busy} onClick={async () => {
                  try { await invoke("agents.disable", { project, agent }); await scan(); }
                  catch (error) { setError((error as Error).message); }
                }}>Disconnect</Button>}
              </div>
            );
          })}
        </Dialog.Body>
      </Dialog>
    </DialogState>
  );
}

export function LocalExport({ project }: { project: string }) {
  const [format, setFormat] = useState("jsonl");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ path: string; count: number }>();
  const [error, setError] = useState("");
  return (
    <section className="flex flex-col gap-3">
      <h2 className="font-bold">Local export</h2>
      <div className="flex items-center gap-3">
        <select aria-label="Export format" value={format} onChange={event => setFormat(event.target.value)}>
          <option value="jsonl">JSONL</option>
          <option value="csv">CSV</option>
        </select>
        <Button variant="secondary" disabled={busy} onClick={async () => {
          setBusy(true); setError("");
          try {
            const directory = await invoke<string | null>("desktop.pick-directory");
            if (directory) setResult(await invoke("workspace.export", { project, data: { directory, format } }));
          } catch (error) { setError((error as Error).message); }
          finally { setBusy(false); }
        }}>Export traces</Button>
      </div>
      {error && <p role="alert" className="text-destructive">{error}</p>}
      {result && <p role="status" className="break-all">{result.count} traces saved to {result.path}</p>}
    </section>
  );
}
