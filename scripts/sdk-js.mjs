import assert from "node:assert/strict";
import { NodeTracerProvider } from "@opentelemetry/sdk-trace-node";
import { LangfuseSpanProcessor } from "@langfuse/otel";
import { LangfuseClient } from "@langfuse/client";
import { propagateAttributes, startActiveObservation, setLangfuseTracerProvider } from "@langfuse/tracing";

const provider = new NodeTracerProvider({ spanProcessors: [new LangfuseSpanProcessor()] });
provider.register();
setLangfuseTracerProvider(provider);
const client = new LangfuseClient();
try {
  const traceId = startActiveObservation("js-sdk-root", trace => {
    propagateAttributes({ sessionId: "js-sdk-session" }, () => {
      startActiveObservation("js-sdk-model", model => {
        model.update({ model: "fixture-model", input: "fixture", output: "completed",
          usageDetails: { input: 6, output: 3 } });
      }, { asType: "generation" });
    });
    return trace.traceId;
  });
  assert(traceId);
  await provider.forceFlush();
  const result = await client.api.observations.getMany({ traceId, fields: "basic,model,usage" });
  const root = result.data.find(item => item.name === "js-sdk-root");
  const generation = result.data.find(item => item.name === "js-sdk-model");
  assert.equal(generation.parentObservationId, root.id);
  assert.equal(generation.model, "fixture-model");
  assert.equal(generation.usageDetails.input + generation.usageDetails.output, 9);
  console.log("Langfuse JS/TS 6.1.1 received and read back trace and child usage");
} finally {
  await provider.shutdown();
  await client.shutdown();
}
