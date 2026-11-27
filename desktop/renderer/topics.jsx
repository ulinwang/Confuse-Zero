import React, { useEffect, useMemo, useState } from "react";
import { Activity, ArrowRight, Boxes, History, Pause, Play, Plus, RefreshCw, Trash2 } from "lucide-react";
import { Dialog, Empty, count, time, useQuery } from "./shared.jsx";

const blank = { topics: [], assignments: [] };
const agentNames = ["Kimi Code","Codex","Claude Code","DeepSeek Harness","Pi","SDK","Gateway","Playground","Experiment"];

export function Topics({ api, tick, refresh, openTrace, filterByTopic, navigate }) {
  const [state,error] = useQuery(api,"topics.list",{},[tick],{maps:[],facets:[]});
  const [providers] = useQuery(api,"workspace.list",{kind:"llm-connections"},[tick],[]);
  const [selected,setSelected] = useState("");
  const [requestedFacet,setFacet] = useState("intent");
  const [chart,setChart] = useState("treemap");
  const [zoom,setZoom] = useState(1);
  const [activeTopic,setActiveTopic] = useState("");
  const [editing,setEditing] = useState(null);
  const [history,setHistory] = useState(false);
  const [fault,setFault] = useState("");
  const [search,setSearch] = useState("");
  const [pending,setPending] = useState(false);
  const active = state.maps.find(map=>map.id===selected) || state.maps[0];
  const id = active?.id;
  const facet = active?.facetIds.includes(requestedFacet) ? requestedFacet : active?.facetIds[0];
  const [result,resultError] = useQuery(api,id && facet ? "topics.result" : null,{id,facet},[id,facet,tick],blank);
  const firstAssignment = result.assignments[0];
  const data = firstAssignment && (firstAssignment.ruleId !== id || firstAssignment.facetId !== facet) ? blank : result;
  const [runs] = useQuery(api,history && id ? "topics.history" : null,{id},[history,id,tick],[]);
  useEffect(()=> { setActiveTopic(""); },[id,facet]);

  async function invoke(channel,payload) {
    try {
      setPending(true);
      const result=await api(channel,payload);
      setFault("");
      refresh();
      return result;
    } catch (error) { setFault(error.message); return null; }
    finally { setPending(false); }
  }
  const shown= data.assignments.filter(row=>
    (!activeTopic || (activeTopic==="unassigned" ? !row.topicStableId : row.topicStableId===activeTopic)) &&
    (!search || (row.summary||"").toLowerCase().includes(search.toLowerCase()) || row.traceId===search)
  );
  const topic=data.topics.find(item=>item.stableId===activeTopic);

  return <div className="topics">
    <div className="toolbar topic-toolbar">
      {active && <select aria-label="Topic map" value={id} onChange={event=>setSelected(event.target.value)}>
        {state.maps.map(map=><option key={map.id} value={map.id}>{map.name} · {map.mode==="live"?"Live map":"Snapshot"}</option>)}
      </select>}
      {active && <span className="muted">{active.status}</span>}
      <div className="toolbar-spacer"></div>
      <button className="subtle" onClick={()=>setEditing({ mode:"live" })}><Plus size={15}/> New live map</button>
      <button className="subtle" onClick={()=>setEditing({ mode:"snapshot" })}><Plus size={15}/> New snapshot</button>
    </div>
    {(fault||error||resultError||active?.error) && <p className="error" role="alert">{fault||error||resultError||active?.error}</p>}
    {!active && <Empty title={providers.length ? "No topic maps yet" : "Set up model connections"}
      action={providers.length ? undefined : "Model connections"} onAction={()=>navigate("Model connections")}/>}
    {active && <>
      <div className="section-heading topic-head">
        <div className="tabs" aria-label="Topics facets">
          {state.facets.filter(item=>active.facetIds.includes(item.id)).map(item=><button aria-pressed={facet===item.id}
            key={item.id} onClick={()=>setFacet(item.id)}>{item.name}</button>)}
        </div>
        <div className="actions">
          {active.mode==="live" && <><button className="icon ghost" aria-label="Refresh map" disabled={pending} onClick={()=>invoke("topics.change",{id,action:"refresh"})}><RefreshCw size={16}/></button>
          <button className="icon ghost" aria-label={active.enabled?"Pause map":"Resume map"} disabled={pending} onClick={()=>invoke("topics.change",{id,action:active.enabled?"pause":"resume"})}>
            {active.enabled?<Pause size={16}/>:<Play size={16}/>}</button>
          <button className="subtle" onClick={()=>setEditing(active)}>Edit map</button></>}
          {active.mode==="snapshot" && active.status==="Failed" && <button className="subtle" onClick={()=>invoke("topics.change",{id,action:"refresh"})}>Retry</button>}
          <button className="icon ghost" aria-label="Run history" onClick={()=>setHistory(true)}><History size={17}/></button>
          <button className="icon ghost" aria-label="Delete topic map" onClick={()=>setEditing({deleteId:id})}><Trash2 size={16}/></button>
        </div>
      </div>
      <div className="toolbar chart-toolbar">
        <div className="tabs">
          <button aria-pressed={chart==="treemap"} onClick={()=>setChart("treemap")}><Boxes size={15}/> Treemap</button>
          <button aria-pressed={chart==="scatter"} onClick={()=>setChart("scatter")}><Activity size={15}/> Scatter</button>
        </div>
        <div className="actions">
          {chart==="scatter" && <label className="scatter-zoom">Zoom <input type="range" min="1" max="3" step=".1" value={zoom} onChange={event=>setZoom(Number(event.target.value))}/></label>}
          <span className="muted">{count(data.assignments.length)} traces</span>
        </div>
      </div>
      {data.assignments.length===0 ? <Empty title={active.status==="Insufficient data"?"Insufficient data":"Waiting for results"}/>
        : chart==="treemap" ? <Treemap topics={data.topics} assignments={data.assignments} selected={activeTopic} select={setActiveTopic}/>
        : <Scatter rows={data.assignments} zoom={zoom} topic={activeTopic} select={setActiveTopic} openTrace={openTrace}/>}
      <div className="section-heading">
        <h2>{topic?.name || (activeTopic==="unassigned" ? "Unassigned" : "Traces")}</h2>
        <div className="actions">
          {topic && <button className="subtle" onClick={()=>filterByTopic(topic.stableId,topic.name)}>Open in Tracing<ArrowRight size={14}/></button>}
          {activeTopic && <button className="subtle" onClick={()=>setActiveTopic("")}>Clear selection</button>}
        </div>
      </div>
      {topic?.description && <p className="muted topic-description">{topic.description}</p>}
      {data.assignments.length>0 && <input aria-label="Search topic traces" placeholder="Search summaries or Trace ID" value={search} onChange={event=>setSearch(event.target.value)}/>}
      <table className="data-table topic-traces"><thead><tr><th>Summary</th><th>Trace ID</th></tr></thead>
        <tbody>{shown.slice(0,100).map(row=><tr key={row.traceId}>
          <td>{row.summary}</td><td><button className="table-link" onClick={()=>openTrace(row.traceId)}>{row.traceId}</button></td>
        </tr>)}</tbody></table>
    </>}
    {history && <Dialog title="Run history" close={()=>setHistory(false)} wide>
      <table className="data-table"><thead><tr><th>Run</th><th>Traces</th><th>Finished</th></tr></thead>
        <tbody>{runs.map(run=><tr key={run.id}><td>{run.status}</td><td>{count(run.traceCount)}</td><td>{time(run.finishedAt)}</td></tr>)}</tbody>
      </table>
      {!runs.length && <Empty title="No runs yet"/>}
    </Dialog>}
    {editing?.deleteId && <Dialog title="Delete topic map?" close={()=>setEditing(null)}><footer>
      <button className="subtle" onClick={()=>setEditing(null)}>Cancel</button>
      <button className="primary" onClick={async()=>{await invoke("topics.delete",{id:editing.deleteId});setEditing(null);setSelected("");}}>Delete</button>
    </footer></Dialog>}
    {editing && !editing.deleteId && <TopicForm initial={editing} close={()=>setEditing(null)} facets={state.facets} providers={providers}
      estimate={map=>api("topics.estimate",{map})} submit={async map=>{
        const saved=await api("topics.save",{map});
        setSelected(saved.id);setFacet(saved.facetIds[0]);refresh();setEditing(null);
      }}/>}
  </div>;
}

function TopicForm({ initial, close, facets, providers, submit, estimate }) {
  const [selected,setSelected] = useState(initial.facetIds||facets.map(item=>item.id));
  const [error,setError] = useState("");
  const [busy,setBusy] = useState(false);
  const [cost,setCost] = useState(null);
  function mapData(form) {
    const fields=Object.fromEntries(new FormData(form));
    const date=key=>fields[key] ? new Date(fields[key]).toISOString() : undefined;
    return {id:initial.id,mode:initial.mode,...fields,facetIds:selected,sample:Number(fields.sample),
      dimensions:Number(fields.dimensions),readinessSeconds:Number(fields.readinessSeconds),
      fromTimestamp:date("fromTimestamp"),toTimestamp:date("toTimestamp")};
  }
  return <Dialog title={initial.id?"Edit map":initial.mode==="live"?"New live map":"New snapshot"} close={close} wide>
    {error && <p className="error" role="alert">{error}</p>}
    <form onSubmit={async event=>{
      event.preventDefault();
      setBusy(true);
      try {await submit(mapData(event.currentTarget));} catch (error) {setError(error.message);}
      finally {setBusy(false);}
    }}>
      <label className="field">Name<input name="name" defaultValue={initial.name} autoFocus required/></label>
      <div className="fields-grid">
        <label className="field">Source<select name="source" defaultValue={initial.source||""}>
          <option value="">All sources</option>{agentNames.map(value=><option key={value}>{value}</option>)}</select></label>
        <label className="field">Sample (%)<input name="sample" type="number" min="1" max="100" defaultValue={initial.sample||100}/></label>
        {initial.mode==="snapshot" && <>
          <label className="field">Start time<input name="fromTimestamp" type="datetime-local" required/></label>
          <label className="field">End time<input name="toTimestamp" type="datetime-local" required/></label>
        </>}
        {[["summaryProvider","Summaries"],["embeddingProvider","Embeddings"],["namingProvider","Topic names"]].map(([key,label])=>
          <label className="field" key={key}>{label}<select name={key} defaultValue={initial[key]||""} required>
            <option value="">Select connection</option>{providers.filter(item=>key!=="embeddingProvider" || item.embeddingModel).map(item=><option key={item.id} value={item.id}>{item.name}</option>)}
          </select></label>)}
        <label className="field">Embedding dimensions<input name="dimensions" type="number" min="64" max="2048" defaultValue={initial.dimensions||1024}/></label>
        <label className="field">Readiness delay (s)<input name="readinessSeconds" type="number" min="0" max="86400" defaultValue={initial.readinessSeconds??300}/></label>
      </div>
      <div className="facet-choices">
        {facets.map(item=><label key={item.id}><input type="checkbox" checked={selected.includes(item.id)} onChange={event=>
          setSelected(event.target.checked?[...selected,item.id]:selected.filter(key=>key!==item.id))
        }/>{item.name}</label>)}
      </div>
      {cost && <p className="muted padded" role="status">{cost.eligibleTraces} eligible traces · {cost.summaryCalls} summaries
        {cost.estimatedUsd!=null ? " · about $"+cost.estimatedUsd.toFixed(4) : ""}</p>}
      <footer><button className="subtle left" type="button" onClick={async event=>{
        try {setCost(await estimate(mapData(event.currentTarget.form)));setError("");} catch(error){setError(error.message);}
      }}>Estimate</button><button className="subtle" type="button" onClick={close}>Cancel</button><button className="primary" disabled={busy||!selected.length}>Save map</button></footer>
    </form>
  </Dialog>;
}

function Treemap({ topics, assignments, selected, select }) {
  const cells=[...topics].sort((a,b)=>b.traceCount-a.traceCount);
  const unassigned=assignments.filter(row=>!row.topicStableId).length;
  if (unassigned) cells.push({stableId:"unassigned",name:"Unassigned",traceCount:unassigned});
  function layout(tiles, box) {
    if (tiles.length<2) return tiles.map(tile=>({...tile,...box}));
    const total=tiles.reduce((value,tile)=>value+tile.traceCount,0);
    let subtotal=0, cut=0;
    while(cut<tiles.length-1 && subtotal<total/2) subtotal+=tiles[cut++].traceCount;
    const portion=subtotal/total;
    if (box.width>box.height) return [
      ...layout(tiles.slice(0,cut),{...box,width:box.width*portion}),
      ...layout(tiles.slice(cut),{...box,left:box.left+box.width*portion,width:box.width*(1-portion)})
    ];
    return [
      ...layout(tiles.slice(0,cut),{...box,height:box.height*portion}),
      ...layout(tiles.slice(cut),{...box,top:box.top+box.height*portion,height:box.height*(1-portion)})
    ];
  }
  return <div className="topic-plot treemap" aria-label="Topics treemap">
    {layout(cells,{left:0,top:0,width:100,height:100}).map(tile=><button className={"topic-cell"+(tile.stableId===selected?" active":"")} key={tile.stableId}
      style={{left:tile.left+"%",top:tile.top+"%",width:tile.width+"%",height:tile.height+"%"}}
      title={tile.name+" · "+tile.traceCount} onClick={()=>select(tile.stableId===selected?"":tile.stableId)}>
      <strong>{tile.name}</strong><span>{tile.traceCount}</span>
    </button>)}
  </div>;
}

function Scatter({ rows, zoom, topic, select, openTrace }) {
  const extent=useMemo(()=>{
    let left=Infinity,top=Infinity,right=-Infinity,bottom=-Infinity;
    for(const row of rows){left=Math.min(left,row.x);right=Math.max(right,row.x);top=Math.min(top,row.y);bottom=Math.max(bottom,row.y);}
    return {left,right,bottom,top};
  },[rows]);
  return <div className="topic-plot scatter" aria-label="Topics scatter plot">
    {rows.map(row=>{
      const x=extent.left===extent.right ? 50: 8+84*(row.x-extent.left)/(extent.right-extent.left);
      const y=extent.top===extent.bottom ? 50: 8+84*(row.y-extent.top)/(extent.bottom-extent.top);
      const selected=!topic || topic===(row.topicStableId||"unassigned");
      return <button key={row.traceId} aria-label={row.summary||row.traceId} title={row.summary||row.traceId}
        className={"point"+(selected?" selected":"")} style={{left:(50+(x-50)*zoom)+"%",top:(50+(y-50)*zoom)+"%"}}
        onClick={()=>select(row.topicStableId||"unassigned")} onDoubleClick={()=>openTrace(row.traceId)}/>;
    })}
  </div>;
}
