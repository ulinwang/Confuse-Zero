import React, { useEffect, useState } from "react";
import { Root as DialogRoot } from "@radix-ui/react-dialog";
import { Activity, ArrowRight } from "lucide-react";
import { Dialog as LangfuseDialog } from "@/src/components/design-system/Dialog/Dialog";
import { CodeSection } from "@/src/components/design-system/CodeSection/CodeSection";

export const count = value => typeof value === "number" ? value.toLocaleString() : "—";
export const time = value => value ? new Date(value).toLocaleString() : "—";
export const duration = value => typeof value === "number" ? (value < 1 ? Math.round(value*1000)+" ms" : value.toFixed(2)+" s") : "—";

export function useQuery(api, channel, payload, dependencies, initial, sameValue) {
  const [result, setResult] = useState(null);
  useEffect(() => {
    if (!channel) return;
    let current = true;
    api(channel, payload).then(value => {
      if (current) setResult(previous =>
        previous?.api === api && previous.channel === channel && !previous.error && sameValue?.(previous.value, value)
          ? previous : { api, channel, value, error: "" }
      );
    }, reason => {
      if (current) setResult(previous => ({
        api, channel,
        value: previous?.api === api && previous.channel === channel ? previous.value : initial,
        error: reason.message,
      }));
    });
    return () => { current = false; };
  }, [api, channel, ...dependencies]);
  return result?.api === api && result.channel === channel
    ? [result.value, result.error, false]
    : [initial, "", Boolean(channel)];
}


export function Dialog({ title, children, close, wide = false }) {
  return <DialogRoot open onOpenChange={open => { if (!open) close?.(); }}>
    <LangfuseDialog title={title} size={wide ? "md" : "default"} hideCloseButton requireExplicitClose={!close} noCapture>
      <LangfuseDialog.Body>{children}</LangfuseDialog.Body>
    </LangfuseDialog>
  </DialogRoot>;
}


export function Empty({ title, action, onAction }) {
  return <div className="empty"><span className="empty-icon"><Activity size={23}/></span><strong>{title}</strong>
    {action && <button className="subtle" onClick={onAction}>{action}<ArrowRight size={14}/></button>}</div>;
}


export function ValueBlock({ name, value }) {
  if (value==null) return null;
  const text = typeof value === "string";
  return <div className="value-block">
    <CodeSection title={name} content={text ? value : JSON.stringify(value, null, 2)} language={text ? undefined : "json"}/>
  </div>;
}

