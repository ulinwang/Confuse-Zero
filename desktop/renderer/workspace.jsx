import React, { useState } from "react";
import { ArrowRight, KeyRound, Plus, Trash2 } from "lucide-react";
import { Dialog, Empty, ValueBlock, count, time, useQuery } from "./shared.jsx";

const def = {
  prompts: { singular:"prompt", fields:[["name","Name"],["prompt","Prompt","long"],["labels","Labels","list"],["tags","Tags","list"]] },
  skills: { singular:"skill", fields:[["name","Name"],["content","SKILL.md","long"],["files","Supporting files (JSON)","json"],["labels","Labels","list"]] },
  datasets: { singular:"dataset", fields:[["name","Name"],["description","Description","long"],["metadata","Metadata (JSON)","json"]] },
  "dataset-items": { singular:"item", fields:[["input","Input","jsonText"],["expectedOutput","Expected output","jsonText"],["metadata","Metadata (JSON)","json"]] },
  "llm-connections": { singular:"connection", fields:[["name","Name"],["format","Format","options",["openai","anthropic"]],
    ["baseUrl","Endpoint URL"],["model","Chat model"],["embeddingModel","Embedding model","optional"],["apiKey","API key","password"],
    ["priceInput","Input price / 1M tokens (USD)","number"],["priceOutput","Output price / 1M tokens (USD)","number"]] },
  evaluators: { singular:"evaluator", fields:[["name","Name"],["type","Type","options",["llm","code"]],["provider","Model connection","providers"],
    ["prompt","Judge prompt","longOptional"],["code","JavaScript function","longOptional"]] },
  dashboards: { singular:"dashboard", fields:[["name","Name"],["view","View","options",["observations","traces","scores"]],
    ["dimensions","Group by","list"],["measure","Measure","options",["count","latency","totalTokens","totalCost","value"]],
    ["aggregation","Aggregation","options",["count","sum","avg","p50","p95","max","min"]]] },
  alerts: { singular:"alert", fields:[["name","Name"],["measure","Measure","options",["count","latency","totalTokens","totalCost"]],
    ["aggregation","Aggregation","options",["count","sum","avg","p95"]],["threshold","Threshold","number"],
    ["windowMinutes","Window (minutes)","number"],["comparator","Condition","options",["above","below"]]] },
  "evaluation-rules": { singular:"rule", fields:[["name","Name"],["evaluator","Evaluator","evaluators"],["source","Source","optional"]] },
  "annotation-queues": { singular:"queue", fields:[["name","Name"],["description","Description","longOptional"]] },
  "annotation-items": { singular:"item", fields:[["traceId","Trace ID"],["status","Status","options",["PENDING","COMPLETED"]]] },
  "score-configs": { singular:"score configuration", fields:[["name","Name"],["dataType","Type","options",["NUMERIC","CATEGORICAL","BOOLEAN","TEXT"]],
    ["categories","Categories (JSON)","json"],["minValue","Minimum","number"],["maxValue","Maximum","number"]] },
};

function jsonValue(value, strict) {
  if (!value) return null;
  try { return JSON.parse(value); }
  catch (error) { if (strict) throw error; return value; }
}

function Field({ spec, value, providers, choices }) {
  const [key,label,type,options] = spec;
  const formatted = value && ["json","jsonText"].includes(type) && typeof value!=="string" ? JSON.stringify(value,null,2)
    : Array.isArray(value) ? value.join(", ") : value ?? "";
  const optional = ["optional","password","list","json","jsonText","longOptional","number","providers"].includes(type) || key==="description";
  return <label className="field">{label}
    {["long","longOptional","json","jsonText"].includes(type) ? <textarea rows={type==="long"?6:3} name={key} defaultValue={formatted} required={!optional}/>
      : type==="options" || type==="providers" || type==="evaluators" ? <select name={key} defaultValue={formatted || options?.[0] || ""} required={!optional}>
          {["providers","evaluators"].includes(type) && <option value="">{type==="providers"?"Select connection":"Select evaluator"}</option>}
          {(type==="providers" ? providers : type==="evaluators" ? choices : options.map(value=>({id:value,name:value}))).map(item=><option key={item.id} value={item.id}>{item.name}</option>)}
        </select>
      : <input name={key} autoComplete={type==="password" ? "new-password" : undefined} type={type==="password"?"password":type==="number"?"number":"text"}
        defaultValue={type==="password"?"":formatted} required={!optional} step="any"/>}
  </label>;
}

function ResourceForm({ kind, initial, defaults, providers, choices, close, save }) {
  const [fault,setFault]=useState("");
  const [busy,setBusy]=useState(false);
  const spec=def[kind];
  return <Dialog title={(initial?.id ? "Edit " : "New ") + spec.singular} close={close} wide>
    {fault && <p className="error" role="alert">{fault}</p>}
    <form onSubmit={async event=>{
      event.preventDefault();
      const values=new FormData(event.currentTarget);
      setBusy(true);
      try {
        const fields={...defaults};
        for (const [name,,type] of spec.fields) {
          const value=values.get(name);
          if (type==="list") fields[name]=value.split(",").map(v=>v.trim()).filter(Boolean);
          else if (type==="json" || type==="jsonText") fields[name]=jsonValue(value,type==="json");
          else if (type==="number") fields[name]=value===""?null:Number(value);
          else fields[name]=value;
        }
        if (initial?.id && !["prompts","skills"].includes(kind)) fields.id=initial.id;
        await save(fields);
        close();
      }catch(error){setFault(error.message);}
      finally{setBusy(false);}
    }}>
      <div className="resource-fields">
        {spec.fields.map(field=><Field key={field[0]} spec={field} value={kind==="skills" && field[0]==="files"
          ? initial?.files?.filter(file=>file.path!=="SKILL.md").map(file=>file.content!==undefined?{path:file.path,content:file.content}:{path:file.path,sha256Hash:file.sha256Hash})
          : initial?.[field[0]]} providers={providers} choices={choices}/>)}
      </div>
      <footer><button type="button" className="subtle" onClick={close}>Cancel</button><button className="primary" disabled={busy}>{initial?.id&&["prompts","skills"].includes(kind)?"Save new version":"Save"}</button></footer>
    </form>
  </Dialog>;
}

function Registry({ kind, api, tick, refresh, providers, choices=[], filters, defaults={}, onSelect, label }) {
  const [rows,error]=useQuery(api,"workspace.list",{kind,filters},[kind,tick,JSON.stringify(filters)],[]);
  const [editing,setEditing]=useState(null);
  const [removing,setRemoving]=useState(null);
  const [version,setVersion]=useState(null);
  const [fault,setFault]=useState("");
  const spec=def[kind];
  async function save(data){
    const result=await api("workspace.save",{kind,data});
    refresh();
    return result;
  }
  return <>
    <div className="section-heading registry-head">{label ? <h2>{label}</h2> : <div className="toolbar-spacer"/>}<button className="subtle" onClick={()=>setEditing({})}><Plus size={15}/> New {spec.singular}</button></div>
    {(fault||error) && <p className="error" role="alert">{fault||error}</p>}
    <table className="data-table"><thead><tr><th>{kind.endsWith("-items")?"Entry":"Name"}</th><th>Updated</th><th>{kind==="prompts"||kind==="skills"?"Labels":"Status"}</th><th></th></tr></thead>
      <tbody>{rows.map(row=><tr key={row.id}>
        <td><button className="table-link" onClick={()=>onSelect ? onSelect(row) : setVersion(row)}>
          {row.name || row.traceId || (typeof row.input==="string"?row.input:JSON.stringify(row.input)) || row.id}
        </button></td>
        <td>{time(row.updatedAt)}</td><td>{row.labels?.join(", ") || row.status || (row.version ? "v"+row.version : "—")}</td>
        <td className="row-action"><button className="icon ghost" aria-label={"Edit "+spec.singular} onClick={()=>setEditing(row)}>Edit</button>
          <button className="icon ghost" aria-label={"Delete "+spec.singular} onClick={()=>setRemoving(row)}><Trash2 size={15}/></button></td>
      </tr>)}</tbody>
    </table>
    {!rows.length && <Empty title={"No "+spec.singular+" records yet"}/>}
    {editing && <ResourceForm kind={kind} initial={editing} defaults={defaults} providers={providers} choices={choices} close={()=>setEditing(null)} save={save}/>}
    {removing && <Dialog title={"Delete "+spec.singular+"?"} close={()=>setRemoving(null)}>
      <footer><button className="subtle" onClick={()=>setRemoving(null)}>Cancel</button>
      <button className="primary" onClick={async()=>{
        try{await api("workspace.delete",{kind,id:removing.id});refresh();setRemoving(null);}
        catch(error){setFault(error.message);setRemoving(null);}
      }}>Delete</button></footer>
    </Dialog>}
    {version && <Dialog title={version.name||spec.singular} close={()=>setVersion(null)} wide>
      <ValueBlock name={spec.singular} value={version.prompt??version.content??version}/>
      {version.version && <form onSubmit={async event=>{
        event.preventDefault();
        const labels=String(new FormData(event.currentTarget).get("labels")).split(",").map(value=>value.trim()).filter(Boolean);
        try{setVersion(await api("workspace.labels",{kind,name:version.name,version:version.version,labels}));refresh();}
        catch(error){setFault(error.message);}
      }}>
        <label className="field">Labels<input name="labels" defaultValue={version.labels?.join(", ")}/></label>
        <footer><button className="primary">Update labels</button></footer>
      </form>}
    </Dialog>}
  </>;
}

function Playground({ api,tick,providers,openTrace,navigate }) {
  const [prompts]=useQuery(api,"workspace.list",{kind:"prompts"},[tick],[]);
  const [result,setResult]=useState(null);
  const [fault,setFault]=useState("");
  const [busy,setBusy]=useState(false);
  if (!providers.length) return <Empty title="No model connections" action="Model connections" onAction={()=>navigate("Model connections")}/>;
  return <>
    {fault && <p className="error" role="alert">{fault}</p>}
    <form className="workspace-form" onSubmit={async event=>{
      event.preventDefault();
      setBusy(true);
      const form=Object.fromEntries(new FormData(event.currentTarget));
      const prompt=prompts.find(value=>value.id===form.savedPrompt);
      try {
        setResult(await api("workspace.playground",{provider:form.provider,prompt:form.prompt,
          promptName:prompt?.name,promptVersion:prompt?.version,variables:jsonValue(form.variables,true)}));
        setFault("");
      } catch(error){setFault(error.message);}
      finally{setBusy(false);}
    }}>
      <div className="fields-grid"><Field spec={["provider","Model connection","providers"]} providers={providers}/>
        <label className="field">Saved prompt<select name="savedPrompt"><option value="">Enter text</option>
          {prompts.map(row=><option key={row.id} value={row.id}>{row.name} · v{row.version}</option>)}</select></label></div>
      <Field spec={["prompt","Prompt","longOptional"]} providers={providers}/>
      <Field spec={["variables","Variables (JSON)","json"]} providers={providers}/>
      <button className="primary" disabled={busy}>{busy?"Running…":"Run"}</button>
    </form>
    {result && <div className="workspace-form"><ValueBlock name="Output" value={result.output}/>
      <button className="subtle" onClick={()=>openTrace(result.traceId)}>Open trace <ArrowRight size={14}/></button></div>}
  </>;
}

function Datasets({ api,tick,refresh,providers }) {
  const [dataset,setDataset]=useState(null);
  if (!dataset) return <Registry kind="datasets" api={api} tick={tick} refresh={refresh} providers={providers} onSelect={setDataset}/>;
  return <>
    <div className="toolbar"><button className="subtle" onClick={()=>setDataset(null)}>Back to datasets</button><h2>{dataset.name}</h2></div>
    <Registry kind="dataset-items" label="Items" api={api} tick={tick} refresh={refresh} providers={providers}
      filters={[{column:"datasetId",operator:"=",value:dataset.id}]} defaults={{datasetId:dataset.id}}/>
  </>;
}

function Evaluation({ api,tick,refresh,providers,openTrace }) {
  const [tab,setTab]=useState("Evaluators");
  const [datasets]=useQuery(api,"workspace.list",{kind:"datasets"},[tick],[]);
  const [evaluators]=useQuery(api,"workspace.list",{kind:"evaluators"},[tick],[]);
  const [runs]=useQuery(api,"workspace.list",{kind:"experiments"},[tick],[]);
  const [experiment,setExperiment]=useState(null);
  const [items]=useQuery(api,experiment?"workspace.list":null,{kind:"experiment-items",
    filters:[{column:"experimentId",operator:"=",value:experiment}]},[experiment,tick],[]);
  const [queue,setQueue]=useState(null);
  const [dialog,setDialog]=useState("");
  const [fault,setFault]=useState("");
  const [busy,setBusy]=useState(false);

  return <>
    <div className="toolbar"><div className="tabs">
      {["Evaluators","Experiments","Rules","Annotation queues","Score configurations"].map(label=><button key={label} aria-pressed={tab===label} onClick={()=>{setTab(label);setQueue(null);setExperiment(null);}}>{label}</button>)}
    </div><div className="toolbar-spacer"></div>
      {tab==="Evaluators" && <button className="subtle" onClick={()=>setDialog("evaluate")}>Evaluate trace</button>}
      {tab==="Experiments" && <button className="subtle" onClick={()=>setDialog("experiment")}><Plus size={15}/> New experiment</button>}
    </div>
    {fault && <p className="error" role="alert">{fault}</p>}
    {tab==="Evaluators" && <Registry kind="evaluators" label="Evaluators" api={api} tick={tick} refresh={refresh} providers={providers}/>}
    {tab==="Rules" && <Registry kind="evaluation-rules" label="Evaluation rules" api={api} tick={tick} refresh={refresh} providers={providers} choices={evaluators}/>}
    {tab==="Score configurations" && <Registry kind="score-configs" label="Score configurations" api={api} tick={tick} refresh={refresh} providers={providers}/>}
    {tab==="Annotation queues" && (queue
      ? <><button className="subtle" onClick={()=>setQueue(null)}>Back to queues</button>
          <Registry kind="annotation-items" label={queue.name} api={api} tick={tick} refresh={refresh} providers={providers}
            filters={[{column:"queueId",operator:"=",value:queue.id}]} defaults={{queueId:queue.id}}
            onSelect={row=>row.traceId&&openTrace(row.traceId)}/></>
      : <Registry kind="annotation-queues" label="Queues" api={api} tick={tick} refresh={refresh} providers={providers} onSelect={setQueue}/>)}
    {tab==="Experiments" && <>
      <table className="data-table"><thead><tr><th>Name</th><th>Status</th><th>Complete</th><th>Failed</th></tr></thead>
        <tbody>{runs.map(row=><tr key={row.id}><td><button className="table-link" onClick={()=>setExperiment(row.id)}>{row.name}</button></td>
          <td>{row.status}</td><td>{count(row.completed)}</td><td>{count(row.failed)}</td></tr>)}</tbody></table>
      {!runs.length && <Empty title="No experiments yet" action="New experiment" onAction={()=>setDialog("experiment")}/>}
      {experiment && <div className="workspace-form"><h2>Experiment items</h2>
        <table className="data-table"><thead><tr><th>Item</th><th>Status</th><th>Trace</th></tr></thead>
          <tbody>{items.map(row=><tr key={row.id}><td>{row.datasetItemId}</td><td>{row.status}</td>
            <td>{row.traceId?<button className="table-link" onClick={()=>openTrace(row.traceId)}>{row.traceId}</button>:row.error}</td></tr>)}</tbody></table></div>}
    </>}
    {dialog && <Dialog title={dialog==="evaluate"?"Evaluate trace":"New experiment"} close={()=>setDialog("")}>
      <form onSubmit={async event=>{
        event.preventDefault();setBusy(true);
        const data=Object.fromEntries(new FormData(event.currentTarget));
        try {
          if (dialog==="experiment") await api("workspace.run-experiment",{data});
          else {await api("workspace.evaluate",data);openTrace(data.traceId);}
          setFault("");setDialog("");refresh();
        } catch(error){setFault(error.message);setDialog("");} finally{setBusy(false);}
      }}>
        {dialog==="experiment" ? <>
          <Field spec={["name","Name"]} providers={providers}/>
          <Field spec={["provider","Model connection","providers"]} providers={providers}/>
          <label className="field">Dataset<select name="datasetId" required><option value="">Select dataset</option>
            {datasets.map(row=><option key={row.id} value={row.id}>{row.name}</option>)}</select></label>
        </> : <Field spec={["traceId","Trace ID"]} providers={providers}/>}
        <label className="field">Evaluator<select name="evaluator" required={dialog==="evaluate"}>
          <option value="">{dialog==="evaluate"?"Select evaluator":"None"}</option>
          {evaluators.map(row=><option key={row.id} value={row.id}>{row.name}</option>)}</select></label>
        <footer><button className="primary" disabled={busy}>{busy?"Running…":"Run"}</button></footer>
      </form>
    </Dialog>}
  </>;
}

function Dashboards({api,tick,refresh,providers}) {
  const [chosen,setChosen]=useState(null);
  const dimensions=(chosen?.dimensions??[]).map(field=>({field}));
  const key=chosen?.aggregation+"_"+chosen?.measure;
  const [data,error]=useQuery(api,chosen?"workspace.metrics":null,{query:{view:chosen?.view,dimensions,
    metrics:[{measure:chosen?.measure,aggregation:chosen?.aggregation}]}},[chosen,tick],{data:[]});
  return <>
    <Registry kind="dashboards" label="Saved dashboards" api={api} tick={tick} refresh={refresh} providers={providers} onSelect={setChosen}/>
    <Registry kind="alerts" label="Alerts" api={api} tick={tick} refresh={refresh} providers={providers}/>
    {chosen && <div className="workspace-form"><h2>{chosen.name}</h2>
      {error && <p className="error" role="alert">{error}</p>}
      <table className="data-table"><thead><tr>{dimensions.map(row=><th key={row.field}>{row.field}</th>)}<th>{chosen.measure}</th></tr></thead>
        <tbody>{data.data.map((row,index)=><tr key={index}>{dimensions.map(({field})=><td key={field}>{row[field]}</td>)}<td>{count(row[key])}</td></tr>)}</tbody></table>
    </div>}
  </>;
}

function Exports({ api,tick,refresh }) {
  const [directory,setDirectory]=useState("");
  const [result,setResult]=useState(null);
  const [fault,setFault]=useState("");
  const [schedules]=useQuery(api,"workspace.list",{kind:"exports"},[tick],[]);
  return <>
    {fault && <p className="error" role="alert">{fault}</p>}
    <form className="workspace-form" onSubmit={async event=>{
      event.preventDefault();
      const form=new FormData(event.currentTarget);
      const values={directory,format:form.get("format"),source:form.get("source"),intervalMinutes:Number(form.get("interval"))};
      try{
        const scheduled=event.nativeEvent.submitter?.value==="schedule";
        setResult(scheduled?await api("workspace.save",{kind:"exports",data:{...values,name:"Scheduled "+values.format}}):await api("workspace.export",{data:values}));
        setFault("");refresh();
      } catch(error){setFault(error.message);}
    }}>
      <div className="fields-grid">
        <Field spec={["format","Format","options",["jsonl","csv"]]}/>
        <Field spec={["source","Source","optional"]}/>
        <label className="field">Interval (minutes)<input type="number" name="interval" min="1" max="43200" defaultValue="60"/></label>
        <label className="field">Directory<button type="button" className="subtle" onClick={async()=>setDirectory(await api("desktop.pick-directory",{})||directory)}>
          {directory||"Choose directory"}</button></label>
      </div>
      <div className="actions"><button className="primary" disabled={!directory}>Export</button>
        <button className="subtle" disabled={!directory} value="schedule">Schedule</button></div>
    </form>
    {result?.path && <p className="padded" role="status">{result.count} traces exported to {result.path}</p>}
    <div className="section-heading"><h2>Schedules</h2></div>
    <table className="data-table"><thead><tr><th>Name</th><th>Directory</th><th>Interval</th><th></th></tr></thead>
      <tbody>{schedules.map(row=><tr key={row.id}><td>{row.name}</td><td>{row.directory}</td><td>{row.intervalMinutes} min</td>
        <td className="row-action"><button className="icon ghost" aria-label="Remove schedule" onClick={async()=>{await api("workspace.delete",{kind:"exports",id:row.id});refresh();}}><Trash2 size={15}/></button></td>
      </tr>)}</tbody></table>
    {!schedules.length && <p className="muted padded">No scheduled exports</p>}
  </>;
}

export function Workspace({ page, api, tick, refresh, navigate, openTrace, state, createKey }) {
  const [providers]=useQuery(api,"workspace.list",{kind:"llm-connections"},[tick],[]);
  const common={api,tick,refresh,providers};
  if (page==="Topics") return null;
  if (page==="Playground") return <Playground {...common} openTrace={openTrace} navigate={navigate}/>;
  if (page==="Datasets") return <Datasets {...common}/>;
  if (page==="Evaluations") return <Evaluation {...common} openTrace={openTrace}/>;
  if (page==="Dashboards") return <Dashboards {...common}/>;
  if (page==="Exports") return <Exports {...common}/>;
  if (page==="AI Gateway") return <>
    <div className="toolbar"><button className="subtle" onClick={()=>createKey("gateway")}><KeyRound size={15}/> Create gateway key</button>
      <button className="subtle" onClick={()=>navigate("Model connections")}>Model connections</button></div>
    <table className="data-table"><thead><tr><th>Provider format</th><th>Gateway URL</th></tr></thead><tbody>
      {["openai","anthropic"].map(format=><tr key={format}><td>{format}</td><td><button className="table-link" onClick={()=>
        state?.sdkEndpoint&&api("desktop.copy",{text:state.sdkEndpoint+"/"+format+"/v1"})}>{state?.sdkEndpoint}/{format}/v1</button></td></tr>)}
    </tbody></table>
  </>;
  const kind={Prompts:"prompts",Skills:"skills","Model connections":"llm-connections"}[page];
  return <Registry kind={kind} {...common}/>;
}
