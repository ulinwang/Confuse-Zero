import React, { useEffect, useRef, useState } from "react";
import { Activity, ArrowRight, X } from "lucide-react";

export const count = value => typeof value === "number" ? value.toLocaleString() : "—";
export const time = value => value ? new Date(value).toLocaleString() : "—";
export const duration = value => typeof value === "number" ? (value < 1 ? Math.round(value*1000)+" ms" : value.toFixed(2)+" s") : "—";

export function useQuery(api, channel, payload, dependencies, initial) {
  const [state, setState] = useState(initial);
  const [error, setError] = useState("");
  useEffect(() => {
    if (!channel) { setState(initial); setError(""); return; }
    let current = true;
    api(channel, payload).then(value => {
      if (current) { setState(value); setError(""); }
    }, reason => { if (current) setError(reason.message); });
    return () => { current = false; };
  }, [api, channel, ...dependencies]);
  return [state, error];
}


export function Dialog({ title, children, close, wide = false }) {
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


export function Empty({ title, action, onAction }) {
  return <div className="empty"><span className="empty-icon"><Activity size={23}/></span><strong>{title}</strong>
    {action && <button className="subtle" onClick={onAction}>{action}<ArrowRight size={14}/></button>}</div>;
}


export function ValueBlock({ name, value }) {
  if (value==null) return null;
  return <div className="value-block"><h4>{name}</h4><pre>{typeof value==="string" ? value : JSON.stringify(value,null,2)}</pre></div>;
}

