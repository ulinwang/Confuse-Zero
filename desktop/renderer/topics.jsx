import React, { useEffect, useMemo, useState } from "react";
import { ArrowRight, History, Pause, Play, Plus, RefreshCw, Trash2 } from "lucide-react";
import { Table as DataTable } from "@/src/components/design-system/table/Table";
import { SearchInput } from "@/src/components/design-system/SearchInput/SearchInput";
import { TopicTreemap } from "@/src/features/topics/TopicTreemap";
import { TopicMapExplorer } from "@/src/features/topics/scatter/TopicMapExplorer";
import { TopicsFacetTabs } from "@/src/features/topics/TopicsFacetTabs";
import { TopicsResultsViewSelect, useTopicsResultsView } from "@/src/features/topics/TopicsResultsViewSelect";
import { prepareTopicDisplay, UNASSIGNED_TOPIC_ID } from "@/src/features/topics/prepare-topic-display";
import { Table, TableHeader, TableBody, TableHead, TableRow, TableCell } from "@/src/components/ui/table";
import { Dialog, Empty, count, time, useQuery } from "./shared.jsx";

const blank = { topics: [], assignments: [] };
const agentNames = ["Kimi Code","Codex","Claude Code","DeepSeek Harness","Pi","SDK","Gateway","Playground","Experiment"];
const traceColumns = [
  { accessorKey: "summary", header: "Summary", size: 350 },
  { accessorKey: "traceId", header: "Trace ID", size: 250, cellClassName: "font-mono" },
];

function samePublication(previous, next) {
  return previous.publishedAt === next.publishedAt &&
    previous.assignments.length === next.assignments.length &&
    previous.assignments[0]?.ruleId === next.assignments[0]?.ruleId &&
    previous.assignments[0]?.facetId === next.assignments[0]?.facetId;
}

export function Topics({ api, tick, refresh, openTrace, filterByTopic, navigate }) {
  const [state,error] = useQuery(api,"topics.list",{},[tick],{maps:[],facets:[]});
  const [providers] = useQuery(api,"workspace.list",{kind:"llm-connections"},[tick],[]);
  const [selected,setSelected] = useState("");
  const [requestedFacet,setFacet] = useState("intent");
  const [chart,setChart] = useTopicsResultsView();
  const [activeTopic,setActiveTopic] = useState("");
  const [editing,setEditing] = useState(null);
  const [history,setHistory] = useState(false);
  const [fault,setFault] = useState("");
  const [search,setSearch] = useState("");
  const [pending,setPending] = useState(false);
  const active = state.maps.find(map=>map.id===selected) || state.maps[0];
  const id = active?.id;
  const facet = active?.facetIds.includes(requestedFacet) ? requestedFacet : active?.facetIds[0];
  const [result,resultError] = useQuery(api,id && facet ? "topics.result" : null,{id,facet},[id,facet,tick],blank,samePublication);
  const firstAssignment = result.assignments[0];
  const data = firstAssignment && (firstAssignment.ruleId !== id || firstAssignment.facetId !== facet) ? blank : result;
  const [runs] = useQuery(api,history && id ? "topics.history" : null,{id},[history,id,tick],[]);
  useEffect(()=> { setActiveTopic(""); },[id,facet]);
  const chartData = useMemo(() => {
    const topics = data.topics.map(({ stableId, name, description, traceCount }) => ({
      id: stableId, name, description, count: traceCount,
    }));
    const assigned = data.assignments.filter(row => row.topicStableId).length;
    const counts = { total: data.assignments.length, assigned };
    const points = data.assignments.map(row => ({
      traceId: row.traceId, topicId: row.topicStableId, summary: row.summary ?? row.traceId,
      x: row.x, y: row.y, outcome: row.topicStableId ? "assigned" : "outlier",
    }));
    return {
      treemap: prepareTopicDisplay({ topics, counts }, "treemap"),
      topics,
      projection: { points, missingSummaryCount: 0, unpositionedCount: 0 },
    };
  }, [data]);

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
    (!activeTopic || (activeTopic===UNASSIGNED_TOPIC_ID ? !row.topicStableId : row.topicStableId===activeTopic)) &&
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
        <TopicsFacetTabs facets={state.facets.filter(item=>active.facetIds.includes(item.id))
          .map(({id, name})=>({facetId:id, name}))} selectedFacet={facet} onSelectFacet={setFacet}/>
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
        <TopicsResultsViewSelect value={chart} onValueChange={setChart}/>
        {chart==="treemap" && <span className="muted">{count(data.assignments.length)} traces</span>}
      </div>
      {data.assignments.length===0 ? <Empty title={active.status==="Insufficient data"?"Insufficient data":"Waiting for results"}/>
        : <div className="topic-plot">
          {chart==="treemap" ? <TopicTreemap {...chartData.treemap} selectedTopicId={activeTopic || null}
            onSelectTopic={topicId=>setActiveTopic(topicId===activeTopic ? "" : topicId)}/>
            : <TopicMapExplorer
              data={chartData.projection} topics={chartData.topics}
              selectedTopic={activeTopic===UNASSIGNED_TOPIC_ID ? "outliers" : activeTopic || null}
              onSelectTopic={topicId=>setActiveTopic(topicId==="outliers" ? UNASSIGNED_TOPIC_ID : topicId || "")}
              onSelectTrace={traceId=>{ if (traceId) openTrace(traceId); }} selectedTraceId={null}
              matchingTraceIds={search ? new Set(shown.map(row=>row.traceId)) : undefined} fillContainer/>}
        </div>}
      <div className="section-heading">
        <h2>{topic?.name || (activeTopic===UNASSIGNED_TOPIC_ID ? "Unassigned" : "Traces")}</h2>
        <div className="actions">
          {topic && <button className="subtle" onClick={()=>filterByTopic(topic.stableId,topic.name)}>Open in Tracing<ArrowRight size={14}/></button>}
          {activeTopic && <button className="subtle" onClick={()=>setActiveTopic("")}>Clear selection</button>}
        </div>
      </div>
      {topic?.description && <p className="muted topic-description">{topic.description}</p>}
      {data.assignments.length>0 && <div className="search-field topic-search">
        <SearchInput placeholder="Search topic traces" value={search} onChange={setSearch} onSubmit={setSearch}/>
      </div>}
      <DataTable tableName="Topic traces" columns={traceColumns} data={{status:"success",data:shown.slice(0,100)}}
        onRowClick={row=>openTrace(row.traceId)} getRowLabel={row=>row.summary ?? row.traceId}/>
    </>}
    {history && <Dialog title="Run history" close={()=>setHistory(false)} wide>
      <Table className="data-table"><TableHeader><TableRow><TableHead>Run</TableHead><TableHead>Traces</TableHead><TableHead>Finished</TableHead></TableRow></TableHeader>
        <TableBody>{runs.map(run=><TableRow key={run.id}><TableCell>{run.status}</TableCell><TableCell>{count(run.traceCount)}</TableCell><TableCell>{time(run.finishedAt)}</TableCell></TableRow>)}</TableBody>
      </Table>
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
