import React, { useCallback, useEffect, useRef, useState } from "react";
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from "@/src/components/ui/table";
import { createRoot } from "react-dom/client";
import {
  ArrowLeft, ArrowRight, Bell, Check, ChevronDown, ChevronRight,
  CircleAlert, CodeXml, Database, FolderOpen, KeyRound, Plus,
  Settings, Terminal, Trash2, X
} from "lucide-react";
import * as Icons from "@langfuse/icons";
import { NavMain } from "@/src/components/nav/NavMain";
import { Sidebar, SidebarContent, SidebarFooter, SidebarMenuButton, SidebarProvider } from "@/src/components/ui/sidebar";
import { Table as DataTable } from "@/src/components/design-system/table/Table";
import { SearchInput } from "@/src/components/design-system/SearchInput/SearchInput";
import { Timeline } from "@/src/components/design-system/Timeline/Timeline";
import { Metric } from "@/src/components/design-system/Metric/Metric";
import { Button } from "@/src/components/design-system/Button/Button";
import { LayerProvider } from "@/src/context/LayerContext/LayerContext";
import { DesktopNavigation } from "./langfuse-bridge/navigation.jsx";
import { DesktopTheme } from "./langfuse-bridge/theme.jsx";
import { safeTriangle } from "./menu.cjs";
import { Dialog, Empty, ValueBlock, count, time, duration, useQuery } from "./shared.jsx";
import { Topics } from "./topics.jsx";
import { Workspace } from "./workspace.jsx";

const agents = { "kimi-code":"Kimi Code", codex:"Codex", "claude-code":"Claude Code", "deepseek-harness":"DeepSeek Harness", pi:"Pi" };
const pages = [
  ["Overview", Icons.LayoutDashboard, "/overview"], ["Tracing", Icons.Activity, "/traces"],
  ["Sessions", Icons.Layers3, "/sessions"], ["Topics", Icons.Tags, "/topics"],
  ["Dashboards", Icons.LayoutDashboard, "/dashboards"], ["Prompts", Icons.BookOpen, "/prompts"],
  ["Skills", Icons.Box, "/skills"], ["Datasets", Icons.Database, "/datasets"],
  ["Evaluations", Icons.FlaskConical, "/evaluations"], ["Scores", Icons.Check, "/scores"],
  ["Playground", Icons.Sparkles, "/playground"], ["AI Gateway", Icons.Network, "/ai-gateway"],
  ["Exports", Icons.Upload, "/exports"]
].map(([title, icon, url]) => ({ title, icon, url }));
const routes = new Map(pages.map(({ title, url }) => [url, title]));
const emptyList = { data: [], total: 0 };
function App() {
  const [state, setState] = useState(null);
  const [project, setProject] = useState("local");
  const [page, setPage] = useState("Overview");
  const [setup, setSetup] = useState(false);
  const [noticeOpen, setNoticeOpen] = useState(false);
  const [projectMenu, setProjectMenu] = useState(false);
  const [form, setForm] = useState(null);
  const [error, setError] = useState("");
  const [tick, setTick] = useState(0);
  const [query, setQuery] = useState("");
  const [source, setSource] = useState("");
  const [session, setSession] = useState("");
  const [topic, setTopic] = useState(null);
  const [trace, setTrace] = useState(null);
  const [sdkKey, setSdkKey] = useState(null);
  const projectToggle = useRef(null);
  const closeProjectMenu = useCallback((restoreFocus = false) => {
    setProjectMenu(false);
    if (restoreFocus) projectToggle.current?.focus();
  }, []);
  const [notices] = useQuery(
    useCallback((channel, payload) => window.confuse.invoke(channel, { ...payload, project }), [project]),
    "notifications.list", {}, [tick], []
  );
  const api = useCallback((channel, payload) => window.confuse.invoke(channel, { ...payload, project }), [project]);
  const refresh = () => setTick(value => value+1);

  function reload() {
    return api("desktop.get-state").then(next => setState(next)).catch(reason => setError(reason.message));
  }

  useEffect(() => {
    api("desktop.get-state").then(next => {
      setState(next);
      if (!next.onboarded) setSetup(true);
    }, reason => setError(reason.message));
    const timer = setInterval(() => setTick(value => value+1), 2500);
    const off = window.confuse.onReopenSetup(() => setSetup(true));
    const offUninstall = window.confuse.onUninstallRequested(() => setForm("uninstall"));
    const onShortcut = event => {
      if (!event.metaKey || event.ctrlKey || event.altKey || event.shiftKey || event.repeat) return;
      const channel = { w: "desktop.hide", q: "desktop.quit" }[event.key.toLowerCase()];
      if (!channel) return;
      event.preventDefault();
      window.confuse.invoke(channel).catch(reason => setError(reason.message));
    };
    window.addEventListener("keydown", onShortcut);
    return () => {
      clearInterval(timer);
      off();
      offUninstall();
      window.removeEventListener("keydown", onShortcut);
    };
  }, []);

  function navigate(next) {
    setTrace(null);
    setSession("");
    setTopic(null);
    setPage(next);
    setError("");
    setProjectMenu(false);
  }

  async function submit(channel, data) {
    try {
      const result = await api(channel, data);
      setError("");
      refresh();
      return result;
    } catch (reason) {
      setError(reason.message);
      throw reason;
    }
  }

  function openTrace(id) {
    setPage("Tracing");
    setTrace(id);
  }

  const name = state?.projects.find(item => item.id === project)?.name || "Local";
  const unread = notices.filter(item => !item.read).length;
  return (
    <DesktopNavigation routes={routes} page={page} navigate={navigate}>
    <SidebarProvider className="desktop" style={{ "--sidebar-width": "var(--window-sidebar-width)" }} open>
      <Sidebar collapsible="none" className="desktop-sidebar" role="complementary" aria-label="Main navigation">
        <div className="titlebar"></div>
        <div className="wordmark"><span className="mark"><Database size={16}/></span> Confuse</div>
        <div className="project">
          <button ref={projectToggle} className="project-toggle" aria-expanded={projectMenu} onClick={() => setProjectMenu(!projectMenu)}>
            <span>{name}</span><ChevronDown size={14}/>
          </button>
          {projectMenu && <ProjectMenu projects={state?.projects || []}
            selected={project} close={closeProjectMenu} select={id => { setProject(id); navigate("Overview"); }}
            create={() => { setProjectMenu(false); setForm("project"); }}
            showKeys={() => navigate("Settings")} copy={() => state?.sdkEndpoint && api("desktop.copy", { text:state.sdkEndpoint })}/>}
        </div>
        <SidebarContent>
          <nav aria-label="Workspace">
            <NavMain items={{ ungrouped: pages.map(item => ({ ...item, isActive: item.title === page })), grouped: null }}/>
          </nav>
        </SidebarContent>
        <SidebarFooter className="sidebar-bottom">
          <SidebarMenuButton isActive={page === "Settings"} onClick={() => navigate("Settings")}><Settings size={17}/> Settings</SidebarMenuButton>
          <SidebarMenuButton isActive={noticeOpen} aria-expanded={noticeOpen} onClick={() => setNoticeOpen(!noticeOpen)}>
            <Bell size={17}/> Notifications{unread > 0 && <span className="unread" aria-label={unread+" unread"}>{unread}</span>}
          </SidebarMenuButton>
        </SidebarFooter>
      </Sidebar>
      <main className="main">
        <header className="window-strip" aria-hidden="true"></header>
        <div className="content" key={project}>
          <div className="heading">
            <div className="heading-left">
              {trace && <button className="icon ghost" aria-label="Back to traces" onClick={() => setTrace(null)}><ArrowLeft size={18}/></button>}
              <h1>{trace ? "Trace" : page}</h1>
            </div>
            {page === "Overview" && <Button variant="outline" text="Connect Agents" onClick={() => setSetup(true)}/>}
          </div>
          {error && <p role="alert" className="error"><CircleAlert size={16}/>{error}<button className="icon ghost" aria-label="Dismiss" onClick={() => setError("")}><X size={15}/></button></p>}
          {page === "Overview" && <Overview api={api} tick={tick} openTrace={openTrace}/>}
          {page === "Tracing" && (trace
            ? <TraceDetail api={api} id={trace} tick={tick} remove={async () => {
                await submit("traces.delete",{id:trace}); setTrace(null);
              }} update={refresh}/>
            : <Tracing api={api} tick={tick} query={query} source={source} session={session}
                setQuery={setQuery} setSource={setSource} setSession={setSession} topic={topic} setTopic={setTopic} openTrace={openTrace}/>)}
          {page === "Topics" && <Topics api={api} tick={tick} refresh={refresh} openTrace={openTrace} navigate={navigate}
            filterByTopic={(id,name)=>{navigate("Tracing");setTopic({id,name});setSource("");setQuery("");}}/>}
          {["Dashboards","Prompts","Skills","Datasets","Evaluations","Playground","AI Gateway","Exports","Model connections"].includes(page)
            && <Workspace key={page+project} page={page} api={api} tick={tick} refresh={refresh} navigate={navigate} openTrace={openTrace}
              state={state} createKey={async kind=>setSdkKey(await submit("keys.create",{kind}))}/>}
          {page === "Sessions" && <Sessions api={api} tick={tick} select={id => {setSession(id);setPage("Tracing");}}/>}
          {page === "Scores" && <Scores api={api} tick={tick} openTrace={openTrace}/>}
          {page === "Settings" && <SettingsPage api={api} state={state} tick={tick} connect={() => setSetup(true)}
            modelConnections={()=>navigate("Model connections")} uninstall={()=>setForm("uninstall")} createKey={async kind => setSdkKey(await submit("keys.create",{kind}))}
            revoke={id => submit("keys.revoke",{id})} changePort={async port => {await submit("desktop.set-port",{port});await reload();}}/>}
        </div>
      </main>
      {noticeOpen && <Notifications notices={notices} close={() => setNoticeOpen(false)} mark={() => submit("notifications.mark-read",{})}
        retry={agent => submit("agents.retry",{agent})}/>}
      {setup && <Onboarding api={api} done={async selections => {
        await submit("desktop.complete-onboarding",{selections});setSetup(false);await reload();
      }} close={() => setSetup(false)} canClose={state?.onboarded}/>}
      {form === "project" && <Dialog title="New project" close={() => setForm(null)}>
        <form onSubmit={async event => {
          event.preventDefault();
          const project = await submit("projects.create",{name:new FormData(event.currentTarget).get("name")});
          setForm(null); setProject(project.id); navigate("Overview"); await reload();
        }}>
          <label className="field">Name<input name="name" required autoFocus maxLength={150}/></label>
          <footer><button type="button" className="subtle" onClick={() => setForm(null)}>Cancel</button><button className="primary">Create project</button></footer>
        </form>
      </Dialog>}
      {form === "uninstall" && <Dialog title="Remove Confuse and its local data?" close={() => setForm(null)}>
        <footer><button className="subtle" onClick={() => setForm(null)}>Cancel</button>
          <button className="primary" onClick={async () => {
            try {await api("desktop.uninstall", {confirmed:true});}
            catch (reason) {setForm(null);setError(reason.message);}
          }}>Uninstall Confuse</button></footer>
      </Dialog>}
      {sdkKey && <Dialog title="New API key" close={() => setSdkKey(null)}>
        <div className="key-field"><span>Public key</span><code>{sdkKey.publicKey}</code><button className="subtle" onClick={() => api("desktop.copy",{text:sdkKey.publicKey})}>Copy</button></div>
        <div className="key-field"><span>Secret key</span><code>{sdkKey.secretKey}</code><button className="subtle" onClick={() => api("desktop.copy",{text:sdkKey.secretKey})}>Copy</button></div>
        <footer><button className="primary" onClick={() => setSdkKey(null)}>Done</button></footer>
      </Dialog>}
    </SidebarProvider>
    </DesktopNavigation>
  );
}

function ProjectMenu({ projects, selected, select, create, showKeys, copy, close }) {
  const [open, setOpen] = useState(false);
  const submenu = useRef(null);
  const parent = useRef(null);
  const timer = useRef(null);
  const anchor = useRef(null);
  const menu = useRef(null);
  function cancel() { clearTimeout(timer.current); }
  function enter() {
    cancel();
    anchor.current = null;
    if (!open) timer.current = setTimeout(() => setOpen(true), 150);
  }
  function leave(event) {
    cancel();
    if (!open) return;
    const rect = parent.current.getBoundingClientRect();
    anchor.current = {
      x: Math.max(rect.left, Math.min(rect.right, event.clientX)),
      y: Math.max(rect.top, Math.min(rect.bottom, event.clientY)),
    };
    timer.current=setTimeout(()=>setOpen(false),300);
  }
  useEffect(() => {
    function pointer(event) {
      if (!open || !anchor.current || !submenu.current) return;
      const rect = submenu.current.getBoundingClientRect();
      const x = rect.left;
      if (safeTriangle(anchor.current,{x,y:rect.top},{x,y:rect.bottom},{x:event.clientX,y:event.clientY})) {
        cancel();
        timer.current=setTimeout(()=>setOpen(false),300);
      }
    }
    function outside(event) {
      if (!menu.current?.parentElement?.contains(event.target)) close();
    }
    document.addEventListener("pointermove", pointer);
    document.addEventListener("pointerdown", outside);
    return () => { cancel(); document.removeEventListener("pointermove",pointer); document.removeEventListener("pointerdown",outside); };
  }, [open, close]);
  return <div className="menu" ref={menu} onKeyDown={event => {
    if (event.key === "Escape") {event.stopPropagation();close(true);}
  }}>
    {projects.map(p => <button key={p.id} onClick={() => select(p.id)}>{p.name}{selected===p.id && <Check size={14}/>}</button>)}
    <button onClick={create}>New project<Plus size={14}/></button>
    <div className="nested-menu" onPointerEnter={enter} onPointerLeave={leave}>
      <button ref={parent} aria-haspopup="true" aria-expanded={open} onClick={()=>setOpen(!open)}
        onKeyDown={event => {
          if (event.key === "ArrowRight") { event.preventDefault();setOpen(true);requestAnimationFrame(()=>submenu.current?.querySelector("button")?.focus()); }
        }}>SDK<ChevronRight size={14}/></button>
      {open && <div className="menu sub-menu" ref={submenu} onPointerEnter={cancel} onKeyDown={event => {
        if(event.key==="ArrowLeft"){event.preventDefault();setOpen(false);parent.current?.focus();}
      }}>
        <button onClick={()=>{copy();close(true);}}>Copy endpoint</button>
        <button onClick={showKeys}>API keys<KeyRound size={14}/></button>
      </div>}
    </div>
  </div>;
}

function Onboarding({ api, done, close, canClose }) {
  const [rows, setRows] = useState([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    scan();
    return () => { alive.current = false; };
  }, []);
  async function scan() {
    setBusy(true);
    try {
      const value = await api("agents.scan",{});
      if (!alive.current) return;
      setRows(value.installations.map(item => {
        const active=value.sources.find(source => source.active && source.selection.agent === item.agent);
        return { ...item, root: active?.selection.root ?? item.root, enabled:!!active,
          history:active?.selection.history ?? false, connected:!!active };
      }));
      setError("");
    } catch (reason) { if (alive.current) setError(reason.message); }
    finally { if (alive.current) setBusy(false); }
  }
  function change(agent, changes) {
    setRows(current => current.map(item => item.agent===agent ? {...item,...changes} : item));
  }
  async function directory(item) {
    try {
      const root = await api("desktop.pick-directory",{});
      if (root) change(item.agent,{root,manual:true,rootReadable:true,enabled:true});
    } catch (reason) {setError(reason.message);}
  }
  async function proceed() {
    setBusy(true);
    try {
      for (const row of rows.filter(item => item.connected && !item.enabled)) await api("agents.disable",{agent:row.agent});
      await done(rows.filter(item=>item.enabled).map(({agent,root,history})=>({agent,root,history})));
    } catch (reason) {if (alive.current) {setError(reason.message);setBusy(false);}}
  }
  return <Dialog title="Connect Agents" close={canClose ? close : undefined} wide>
    {error && <p className="error" role="alert">{error}</p>}
    <div className="agent-list">
      {rows.map(item => <div className="agent-row" key={item.agent}>
        <label className="agent-name">
          <input type="checkbox" aria-label={"Connect "+agents[item.agent]} checked={item.enabled}
            disabled={!item.executable && !item.manual && !item.connected} onChange={event => change(item.agent,{enabled:event.target.checked})}/>
          <span className="agent-icon"><Terminal size={17}/></span>
          <span><strong>{agents[item.agent]}</strong><small>{item.connected ? "Connected" : item.executable || item.manual ? "Detected" : "Unavailable"}{item.version ? " · "+item.version : ""}</small></span>
        </label>
        <label className="history"><input type="checkbox" aria-label={"Import "+agents[item.agent]+" history"}
          checked={item.history} disabled={!item.enabled} onChange={event=>change(item.agent,{history:event.target.checked})}/> Import history</label>
        <button className="icon ghost folder" onClick={() => directory(item)} title={item.root} aria-label={"Choose "+agents[item.agent]+" directory"}><FolderOpen size={17}/></button>
      </div>)}
      {rows.length===0 && busy && <div className="empty compact">Detecting Agents…</div>}
    </div>
    <footer>
      <button className="subtle left" disabled={busy} onClick={scan}>Scan again</button>
      <button className="primary" disabled={busy} onClick={proceed}>{rows.some(item=>item.enabled) ? "Connect" : "Continue"}<ArrowRight size={15}/></button>
    </footer>
  </Dialog>;
}

function Overview({ api, tick, openTrace }) {
  const [summary, error, loading] = useQuery(api,"dashboard.summary",{},[tick],{});
  return <>
    {error && <p className="error" role="alert">{error}</p>}
    <div className="metrics">
      {[["Traces",summary.traces],["Sessions",summary.sessions],["Tokens",summary.tokens],["Errors",summary.errors]].map(([name,value])=>(
        <div className="metric" key={name}><span>{name}</span><strong>{count(value)}</strong></div>
      ))}
    </div>
    <div className="section-heading"><h2>Recent traces</h2></div>
    <TraceTable rows={summary.recent || []} select={openTrace} loading={loading} empty={<Empty title="No traces yet"/>}/>
  </>;
}

function TraceTable({ rows, select, loading, empty }) {
  return <DataTable tableName="Traces" columns={traceColumns} data={loading ? { status: "loading" } : { status: "success", data: rows }}
    noResultsMessage={empty} onRowClick={row => select(row.id)} getRowLabel={row => `${row.name}, ${row.id}`}/>;
}

const traceColumns = [
  { accessorKey: "name", header: "Name", size: 230 },
  { accessorKey: "source", header: "Source", size: 135 },
  { accessorKey: "timestamp", header: "Time", cell: ({ getValue }) => time(getValue()), size: 175 },
  { accessorKey: "latency", header: "Latency", cell: ({ getValue }) => duration(getValue()), size: 90 },
];

function Tracing({ api, tick, query, source, session, setQuery, setSource, setSession, topic, setTopic, openTrace }) {
  const [offset,setOffset]=useState(0);
  const [result,error,loading]=useQuery(api,"traces.list",{query,source,session,topic:topic?.id,offset},[tick,query,source,session,topic,offset],emptyList);
  useEffect(()=>setOffset(0),[query,source,session,topic]);
  return <>
    <div className="toolbar">
      <div className="search-field"><SearchInput placeholder="Search traces" value={query} onChange={setQuery} onSubmit={setQuery}/></div>
      <select aria-label="Trace source" value={source} onChange={event=>setSource(event.target.value)}><option value="">All sources</option>{Object.values(agents).concat("SDK","Gateway","Playground").map(name=><option key={name}>{name}</option>)}</select>
      {session && <button className="subtle" onClick={()=>setSession("")}>Session <X size={14}/></button>}
      {topic && <button className="subtle" onClick={()=>setTopic(null)}>{topic.name} <X size={14}/></button>}
    </div>
    {error && <p className="error" role="alert">{error}</p>}
    <TraceTable rows={result.data} select={openTrace} loading={loading} empty={<Empty title="No matching traces"/>}/>
    <Pagination total={result.total} offset={offset} setOffset={setOffset}/>
  </>;
}

function Pagination({ total, offset, setOffset }) {
  if(total<=100) return null;
  return <div className="pagination">
    <span>{offset+1}–{Math.min(total,offset+100)} of {total}</span>
    <button className="subtle" disabled={offset===0} onClick={()=>setOffset(offset-100)}>Previous</button>
    <button className="subtle" disabled={offset+100>=total} onClick={()=>setOffset(offset+100)}>Next</button>
  </div>;
}

function Sessions({ api, tick, select }) {
  const [offset,setOffset]=useState(0);
  const [sessions,error,loading]=useQuery(api,"sessions.list",{offset},[tick,offset],emptyList);
  return <>
    {error && <p className="error" role="alert">{error}</p>}
    <DataTable tableName="Sessions" columns={sessionColumns} data={loading ? { status: "loading" } : { status: "success", data: sessions.data }}
      onRowClick={row => select(row.id)} getRowLabel={row => row.id} noResultsMessage={<Empty title="No sessions yet"/>}/>
    <Pagination total={sessions.total} offset={offset} setOffset={setOffset}/>
  </>;
}

const sessionColumns = [
  { accessorKey: "id", header: "Session", size: 225 },
  { accessorKey: "source", header: "Source", size: 135 },
  { accessorKey: "updatedAt", header: "Last active", cell: ({ getValue }) => time(getValue()), size: 175 },
  { accessorKey: "traceCount", header: "Traces", cell: ({ getValue }) => count(getValue()), size: 90 },
];

function Scores({ api, tick, openTrace }) {
  const [offset,setOffset]=useState(0);
  const [scores,error]=useQuery(api,"scores.list",{offset},[tick,offset],emptyList);
  return <>
    {error && <p className="error" role="alert">{error}</p>}
    <Table className="data-table"><TableHeader><TableRow><TableHead>Name</TableHead><TableHead>Value</TableHead><TableHead>Source</TableHead><TableHead>Trace</TableHead></TableRow></TableHeader>
      <TableBody>{scores.data.map(row=><TableRow key={row.id}><TableCell>{row.name}</TableCell><TableCell>{String(row.stringValue??row.value)}</TableCell><TableCell>{row.source}</TableCell>
        <TableCell>{row.traceId && <button className="table-link" onClick={()=>openTrace(row.traceId)}>{row.traceId}</button>}</TableCell></TableRow>)}</TableBody></Table>
    {!scores.data.length && <Empty title="No scores yet"/>}
    <Pagination total={scores.total} offset={offset} setOffset={setOffset}/>
  </>;
}

function TraceDetail({ api, id, tick, remove, update }) {
  const [data,error]=useQuery(api,"traces.get",{id},[id,tick],null);
  const [selected,setSelected]=useState(null);
  const [confirm,setConfirm]=useState(false);
  const [score,setScore]=useState(false);
  useEffect(()=>setSelected(null),[id]);
  if (!data || error) return <p className={error?"error":"muted"}>{error || "Loading trace…"}</p>;
  const observation=data.observations.find(row=>row.id===selected);
  const current=observation || data.trace;
  const parents=new Map(data.observations.map(row=>[row.id,row.parentObservationId]));
  function depth(row) {
    let current=row.parentObservationId, level=0;
    while(current && parents.has(current) && level<8){level++;current=parents.get(current);}
    return level;
  }
  return <>
    <div className="trace-title"><div><h2>{data.trace.name}</h2><code>{id}</code></div><div className="actions">
      <button className="subtle" onClick={()=>setScore(true)}><Plus size={15}/> Score</button>
      <button className="icon ghost" aria-label="Delete trace" onClick={()=>setConfirm(true)}><Trash2 size={16}/></button>
    </div></div>
    <div className="detail">
      <section className="observations" aria-label="Observations">
        <h3>Observations</h3>
        <Timeline>
          {data.observations.map(row=><Timeline.Item key={row.id} isActive={selected===row.id}>
            <button type="button" className="observation" style={{paddingInlineStart:depth(row)*13}}
              aria-pressed={selected===row.id} onClick={()=>setSelected(selected===row.id?null:row.id)}>
              <span>{row.name}</span><small>{row.type}</small>
            </button>
          </Timeline.Item>)}
        </Timeline>
      </section>
      <section className="inspector">
        <h3>{current.name}</h3>
        {current.model && <div className="actions">
          <Metric label="Model" value={current.model} shouldWrap/>
          {current.usage?.total != null && <Metric label="Tokens" value={count(current.usage.total)}/>}
        </div>}
        <ValueBlock name="Input" value={current.input}/><ValueBlock name="Output" value={current.output}/>
        {data.scores.length>0 && <ValueBlock name="Scores" value={data.scores}/>}
        {observation && <ValueBlock name="Metadata" value={observation.metadata}/>}
      </section>
    </div>
    {confirm && <Dialog title="Delete trace?" close={()=>setConfirm(false)}><footer>
      <button className="subtle" onClick={()=>setConfirm(false)}>Cancel</button><button className="primary" onClick={remove}>Delete</button></footer></Dialog>}
    {score && <Dialog title="Add score" close={()=>setScore(false)}>
      <form onSubmit={async event=>{
        event.preventDefault();
        const values=new FormData(event.currentTarget);
        await api("scores.put",{score:{traceId:id,name:values.get("name"),value:Number(values.get("value")),source:"ANNOTATION"}});
        setScore(false);update();
      }}>
        <label className="field">Name<input name="name" required autoFocus/></label>
        <label className="field">Value<input name="value" type="number" step="any" required/></label>
        <footer><button className="primary">Save score</button></footer>
      </form>
    </Dialog>}
  </>;
}

function SettingsPage({ api, state, tick, connect, modelConnections, uninstall, createKey, revoke, changePort }) {
  const [sources,error] = useQuery(api,"agents.scan",{},[tick],{installations:[],sources:[]});
  const [keys] = useQuery(api,"keys.list",{},[tick],[]);
  const [port,setPort]=useState(state?.port || 32127);
  const [pending,setPending]=useState("");
  return <>
    {error && <p className="error" role="alert">{error}</p>}
    <div className="section-heading"><h2>Agents</h2><button className="subtle" onClick={connect}>Manage</button></div>
    <Table className="data-table"><TableHeader><TableRow><TableHead>Agent</TableHead><TableHead>Installation</TableHead><TableHead>Collection</TableHead><TableHead>Session directory</TableHead></TableRow></TableHeader>
      <TableBody>{sources.installations.map(item=>{
        const current=sources.sources.find(row=>row.selection.agent===item.agent&&row.active);
        return <TableRow key={item.agent}><TableCell>{agents[item.agent]}</TableCell><TableCell>{item.executable?"Detected":"Unavailable"}</TableCell>
          <TableCell>{current ? current.pending?"Connecting":"Enabled" : "Disabled"}</TableCell><TableCell className="path" title={current?.selection.root||item.root}>{current?.selection.root||item.root}</TableCell></TableRow>;
      })}</TableBody></Table>
    <div className="section-heading"><h2>Models</h2><button className="subtle" onClick={modelConnections}>Model connections</button></div>
    <div className="section-heading"><h2>Local SDK</h2></div>
    <form className="inline-form" onSubmit={event=>{event.preventDefault();changePort(Number(port)).catch(()=>{});}}>
      <label className="field">Port<input type="number" min="1" max="65535" value={port} onChange={event=>setPort(event.target.value)}/></label>
      <button className="subtle">Save</button>
      {state?.sdkEndpoint && <button className="subtle" type="button" onClick={()=>api("desktop.copy",{text:state.sdkEndpoint})}><CodeXml size={16}/>{state.sdkEndpoint}</button>}
    </form>
    {state?.listenerError && <p className="error" role="alert">{state.listenerError}</p>}
    <div className="section-heading"><h2>API keys</h2><button className="subtle" onClick={()=>createKey("sdk")}><Plus size={15}/> Create key</button></div>
    <Table className="data-table"><TableHeader><TableRow><TableHead>Public key</TableHead><TableHead>Secret</TableHead><TableHead>Kind</TableHead><TableHead></TableHead></TableRow></TableHeader>
      <TableBody>{keys.map(key=><TableRow key={key.id}><TableCell><code>{key.publicKey}</code></TableCell><TableCell><code>{key.displaySecretKey}</code></TableCell>
        <TableCell>{key.kind}</TableCell><TableCell className="row-action"><button className="icon ghost" aria-label="Revoke key" onClick={()=>setPending(key.id)}><Trash2 size={15}/></button></TableCell></TableRow>)}</TableBody></Table>
    {!keys.length && <p className="muted padded">No API keys yet</p>}
    <div className="section-heading"><h2>Installation</h2><button className="subtle" onClick={uninstall}>Uninstall Confuse…</button></div>
    {pending && <Dialog title="Revoke key?" close={()=>setPending("")}>
      <footer><button className="subtle" onClick={()=>setPending("")}>Cancel</button><button className="primary" onClick={async()=>{await revoke(pending);setPending("");}}>Revoke</button></footer>
    </Dialog>}
  </>;
}

function Notifications({ notices, close, mark, retry }) {
  return <section className="notification-pane" aria-label="Notifications">
    <div className="notification-head"><h2>Notifications</h2><button className="icon ghost" aria-label="Close notifications" onClick={close}><X size={18}/></button></div>
    {notices.some(item=>!item.read) && <button className="small-button" onClick={mark}>Mark all read</button>}
    {!notices.length && <p className="muted padded">No notifications</p>}
    <div className="notification-list">
      {notices.map(item=><div className={"notice "+(!item.read?"new":"")} key={item.id}>
        <div className="notice-title"><strong>{item.title}</strong><time>{time(item.createdAt)}</time></div>
        <p>{item.message}</p>
        {item.source in agents && item.level!=="success" && <button className="subtle" onClick={()=>retry(item.source)}>Retry</button>}
      </div>)}
    </div>
  </section>;
}

createRoot(document.getElementById("app")).render(<DesktopTheme><LayerProvider><App/></LayerProvider></DesktopTheme>);
