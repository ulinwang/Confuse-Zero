import React, { useCallback, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  Activity, ArrowDownRight, ArrowLeft, ArrowRight, Bell, Check, ChevronDown, ChevronRight,
  CircleAlert, CodeXml, Database, FolderOpen, KeyRound, Layers3, LayoutDashboard, Plus,
  Search, Settings, Terminal, Trash2, X
} from "lucide-react";
import { safeTriangle } from "./menu.cjs";

const agents = { "kimi-code":"Kimi Code", codex:"Codex", "claude-code":"Claude Code", "deepseek-harness":"DeepSeek Harness", pi:"Pi" };
const pages = [
  ["Overview", LayoutDashboard], ["Tracing", Activity], ["Sessions", Layers3],
  ["Scores", Check]
];
const emptyList = { data: [], total: 0 };
const count = value => typeof value === "number" ? value.toLocaleString() : "—";
const time = value => value ? new Date(value).toLocaleString() : "—";
const duration = value => typeof value === "number" ? (value < 1 ? Math.round(value*1000)+" ms" : value.toFixed(2)+" s") : "—";

function useQuery(api, channel, payload, dependencies, initial) {
  const [state, setState] = useState(initial);
  const [error, setError] = useState("");
  useEffect(() => {
    let current = true;
    api(channel, payload).then(value => {
      if (current) { setState(value); setError(""); }
    }, reason => { if (current) setError(reason.message); });
    return () => { current = false; };
  }, [api, channel, ...dependencies]);
  return [state, error];
}

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
  const [trace, setTrace] = useState(null);
  const [sdkKey, setSdkKey] = useState(null);
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
    return () => { clearInterval(timer); off(); };
  }, []);

  function navigate(next) {
    setTrace(null);
    setSession("");
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
    <div className="desktop">
      <aside className="sidebar" aria-label="Main navigation">
        <div className="titlebar"></div>
        <div className="wordmark"><span className="mark"><Database size={16}/></span> Confuse</div>
        <div className="project">
          <button className="project-toggle" aria-expanded={projectMenu} onClick={() => setProjectMenu(!projectMenu)}>
            <span>{name}</span><ChevronDown size={14}/>
          </button>
          {projectMenu && <ProjectMenu projects={state?.projects || []}
            selected={project} close={() => setProjectMenu(false)} select={id => { setProject(id); navigate("Overview"); }}
            create={() => { setProjectMenu(false); setForm("project"); }}
            showKeys={() => navigate("Settings")} copy={() => state?.sdkEndpoint && api("desktop.copy", { text:state.sdkEndpoint })}/>}
        </div>
        <nav>
          {pages.map(([label, Icon]) => <button key={label} className={page === label ? "nav active" : "nav"}
            aria-current={page === label ? "page" : undefined} onClick={() => navigate(label)}>
            <Icon size={17}/>{label}
          </button>)}
        </nav>
        <div className="sidebar-bottom">
          <button className={page === "Settings" ? "nav active" : "nav"} onClick={() => navigate("Settings")}><Settings size={17}/> Settings</button>
          <button className={noticeOpen ? "nav active" : "nav"} aria-expanded={noticeOpen} onClick={() => setNoticeOpen(!noticeOpen)}>
            <Bell size={17}/> Notifications{unread > 0 && <span className="unread" aria-label={unread+" unread"}>{unread}</span>}
          </button>
        </div>
      </aside>
      <main className="main">
        <header className="window-strip" aria-hidden="true"></header>
        <div className="content">
          <div className="heading">
            <div className="heading-left">
              {trace && <button className="icon ghost" aria-label="Back to traces" onClick={() => setTrace(null)}><ArrowLeft size={18}/></button>}
              <h1>{trace ? "Trace" : page}</h1>
            </div>
            {page === "Overview" && <button className="subtle" onClick={() => setSetup(true)}><Plus size={16}/> Connect Agents</button>}
          </div>
          {error && <p role="alert" className="error"><CircleAlert size={16}/>{error}<button className="icon ghost" aria-label="Dismiss" onClick={() => setError("")}><X size={15}/></button></p>}
          {page === "Overview" && <Overview api={api} tick={tick} openTrace={openTrace} connect={() => setSetup(true)}/>}
          {page === "Tracing" && (trace
            ? <TraceDetail api={api} id={trace} tick={tick} remove={async () => {
                await submit("traces.delete",{id:trace}); setTrace(null);
              }} update={refresh}/>
            : <Tracing api={api} tick={tick} query={query} source={source} session={session}
                setQuery={setQuery} setSource={setSource} setSession={setSession} openTrace={openTrace}/>)}
          {page === "Sessions" && <Sessions api={api} tick={tick} select={id => {setSession(id);setPage("Tracing");}}/>}
          {page === "Scores" && <Scores api={api} tick={tick} openTrace={openTrace}/>}
          {page === "Settings" && <SettingsPage api={api} state={state} tick={tick} connect={() => setSetup(true)}
            createKey={async kind => setSdkKey(await submit("keys.create",{kind}))}
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
      {sdkKey && <Dialog title="New API key" close={() => setSdkKey(null)}>
        <div className="key-field"><span>Public key</span><code>{sdkKey.publicKey}</code><button className="subtle" onClick={() => api("desktop.copy",{text:sdkKey.publicKey})}>Copy</button></div>
        <div className="key-field"><span>Secret key</span><code>{sdkKey.secretKey}</code><button className="subtle" onClick={() => api("desktop.copy",{text:sdkKey.secretKey})}>Copy</button></div>
        <footer><button className="primary" onClick={() => setSdkKey(null)}>Done</button></footer>
      </Dialog>}
    </div>
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
  function enter() { cancel(); anchor.current = null; timer.current=setTimeout(()=>setOpen(true),150); }
  function leave(event) {
    cancel();
    anchor.current = {x:event.clientX,y:event.clientY};
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
    if (event.key === "Escape") {event.stopPropagation();close();}
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
        <button onClick={()=>{copy();close();}}>Copy endpoint</button>
        <button onClick={showKeys}>API keys<KeyRound size={14}/></button>
      </div>}
    </div>
  </div>;
}

function Dialog({ title, children, close, wide = false }) {
  const dialog = useRef(null);
  useEffect(() => {
    dialog.current.showModal();
    return () => { dialog.current?.close(); };
  }, []);
  return <dialog className={wide ? "dialog wide" : "dialog"} ref={dialog} onCancel={event => {
    event.preventDefault();
    close?.();
  }}>
    <div className="dialog-head"><h2>{title}</h2>{close && <button className="icon ghost" aria-label="Close" onClick={close}><X size={18}/></button>}</div>
    {children}
  </dialog>;
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

function Overview({ api, tick, openTrace, connect }) {
  const [summary, error] = useQuery(api,"dashboard.summary",{},[tick],{});
  return <>
    {error && <p className="error" role="alert">{error}</p>}
    <div className="metrics">
      {[["Traces",summary.traces],["Sessions",summary.sessions],["Tokens",summary.tokens],["Errors",summary.errors]].map(([name,value])=>(
        <div className="metric" key={name}><span>{name}</span><strong>{count(value)}</strong></div>
      ))}
    </div>
    <div className="section-heading"><h2>Recent traces</h2></div>
    <TraceTable rows={summary.recent || []} select={openTrace} empty={<Empty title="No traces yet" action="Connect Agents" onAction={connect}/>}/>
  </>;
}

function Empty({ title, action, onAction }) {
  return <div className="empty"><span className="empty-icon"><Activity size={23}/></span><strong>{title}</strong>
    {action && <button className="subtle" onClick={onAction}>{action}<ArrowRight size={14}/></button>}</div>;
}

function TraceTable({ rows, select, empty }) {
  return <div className="table-scroll">
    <table className="data-table">
      <thead><tr><th>Name</th><th>Source</th><th>Time</th><th className="numeric">Latency</th><th><span className="sr-only">Open</span></th></tr></thead>
      <tbody>{rows.map(row => <tr key={row.id} onClick={() => select(row.id)}>
        <td><button className="table-link" onClick={event => {event.stopPropagation();select(row.id);}}>{row.name}</button></td>
        <td>{row.source}</td><td>{time(row.timestamp)}</td><td className="numeric">{duration(row.latency)}</td><td className="row-action"><ArrowDownRight size={15}/></td>
      </tr>)}</tbody>
    </table>
    {!rows.length && empty}
  </div>;
}

function Tracing({ api, tick, query, source, session, setQuery, setSource, setSession, openTrace }) {
  const [offset,setOffset]=useState(0);
  const [result,error]=useQuery(api,"traces.list",{query,source,session,offset},[tick,query,source,session,offset],emptyList);
  useEffect(()=>setOffset(0),[query,source,session]);
  return <>
    <div className="toolbar">
      <label className="search"><Search size={17}/><input aria-label="Search traces" placeholder="Search traces" value={query} onChange={event=>setQuery(event.target.value)}/></label>
      <select aria-label="Trace source" value={source} onChange={event=>setSource(event.target.value)}><option value="">All sources</option>{Object.values(agents).concat("SDK","Gateway","Playground").map(name=><option key={name}>{name}</option>)}</select>
      {session && <button className="subtle" onClick={()=>setSession("")}>Session <X size={14}/></button>}
    </div>
    {error && <p className="error" role="alert">{error}</p>}
    <TraceTable rows={result.data} select={openTrace} empty={<Empty title="No matching traces"/>}/>
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
  const [sessions,error]=useQuery(api,"sessions.list",{offset},[tick,offset],emptyList);
  return <>
    {error && <p className="error" role="alert">{error}</p>}
    <table className="data-table"><thead><tr><th>Session</th><th>Source</th><th>Last active</th><th className="numeric">Traces</th></tr></thead>
      <tbody>{sessions.data.map(row=><tr key={row.id} onClick={()=>select(row.id)}>
        <td><button className="table-link" onClick={event=>{event.stopPropagation();select(row.id);}}>{row.id}</button></td>
        <td>{row.source}</td><td>{time(row.updatedAt)}</td><td className="numeric">{count(row.traceCount)}</td>
      </tr>)}</tbody></table>
    {!sessions.data.length && <Empty title="No sessions yet"/>}
    <Pagination total={sessions.total} offset={offset} setOffset={setOffset}/>
  </>;
}

function Scores({ api, tick, openTrace }) {
  const [offset,setOffset]=useState(0);
  const [scores,error]=useQuery(api,"scores.list",{offset},[tick,offset],emptyList);
  return <>
    {error && <p className="error" role="alert">{error}</p>}
    <table className="data-table"><thead><tr><th>Name</th><th>Value</th><th>Source</th><th>Trace</th></tr></thead>
      <tbody>{scores.data.map(row=><tr key={row.id}><td>{row.name}</td><td>{String(row.stringValue??row.value)}</td><td>{row.source}</td>
        <td>{row.traceId && <button className="table-link" onClick={()=>openTrace(row.traceId)}>{row.traceId}</button>}</td></tr>)}</tbody></table>
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
        {data.observations.map(row=><button key={row.id} className={"observation"+(selected===row.id?" selected":"")} style={{paddingInlineStart:12+depth(row)*13}}
          onClick={()=>setSelected(selected===row.id?null:row.id)}><span className="observation-dot"></span><span>{row.name}</span><small>{row.type}</small></button>)}
      </section>
      <section className="inspector">
        <h3>{current.name}</h3>
        {current.model && <span className="muted">{current.model} · {count(current.usage?.total)} tokens</span>}
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

function ValueBlock({ name, value }) {
  if (value==null) return null;
  return <div className="value-block"><h4>{name}</h4><pre>{typeof value==="string" ? value : JSON.stringify(value,null,2)}</pre></div>;
}

function SettingsPage({ api, state, tick, connect, createKey, revoke, changePort }) {
  const [sources,error] = useQuery(api,"agents.scan",{},[tick],{installations:[],sources:[]});
  const [keys] = useQuery(api,"keys.list",{},[tick],[]);
  const [port,setPort]=useState(state?.port || 32127);
  const [pending,setPending]=useState("");
  return <>
    {error && <p className="error" role="alert">{error}</p>}
    <div className="section-heading"><h2>Agents</h2><button className="subtle" onClick={connect}>Manage</button></div>
    <table className="data-table"><thead><tr><th>Agent</th><th>Installation</th><th>Collection</th><th>Session directory</th></tr></thead>
      <tbody>{sources.installations.map(item=>{
        const current=sources.sources.find(row=>row.selection.agent===item.agent&&row.active);
        return <tr key={item.agent}><td>{agents[item.agent]}</td><td>{item.executable?"Detected":"Unavailable"}</td>
          <td>{current ? current.pending?"Connecting":"Enabled" : "Disabled"}</td><td className="path" title={current?.selection.root||item.root}>{current?.selection.root||item.root}</td></tr>;
      })}</tbody></table>
    <div className="section-heading"><h2>Local SDK</h2></div>
    <form className="inline-form" onSubmit={event=>{event.preventDefault();changePort(Number(port)).catch(()=>{});}}>
      <label className="field">Port<input type="number" min="1" max="65535" value={port} onChange={event=>setPort(event.target.value)}/></label>
      <button className="subtle">Save</button>
      {state?.sdkEndpoint && <button className="subtle" type="button" onClick={()=>api("desktop.copy",{text:state.sdkEndpoint})}><CodeXml size={16}/>{state.sdkEndpoint}</button>}
    </form>
    {state?.listenerError && <p className="error" role="alert">{state.listenerError}</p>}
    <div className="section-heading"><h2>API keys</h2><button className="subtle" onClick={()=>createKey("sdk")}><Plus size={15}/> Create key</button></div>
    <table className="data-table"><thead><tr><th>Public key</th><th>Secret</th><th>Kind</th><th></th></tr></thead>
      <tbody>{keys.map(key=><tr key={key.id}><td><code>{key.publicKey}</code></td><td><code>{key.displaySecretKey}</code></td>
        <td>{key.kind}</td><td className="row-action"><button className="icon ghost" aria-label="Revoke key" onClick={()=>setPending(key.id)}><Trash2 size={15}/></button></td></tr>)}</tbody></table>
    {!keys.length && <p className="muted padded">No API keys yet</p>}
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

createRoot(document.getElementById("app")).render(<App/>);
