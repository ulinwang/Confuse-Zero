const procedures = require("./procedures.json");
const organization = "local";
const maxBodyBytes = 8 * 1024 * 1024;
const maxBatch = 24;
const trpcCodes = {
  BAD_REQUEST: [-32600, 400],
  FORBIDDEN: [-32003, 403],
  NOT_FOUND: [-32004, 404],
  METHOD_NOT_SUPPORTED: [-32005, 405],
  CONFLICT: [-32009, 409],
  PAYLOAD_TOO_LARGE: [-32013, 413],
  INTERNAL_SERVER_ERROR: [-32603, 500],
  NOT_IMPLEMENTED: [-32603, 501],
};

class ProcedureError extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

async function requestBody(request) {
  if (!request.body) throw new ProcedureError("BAD_REQUEST", "Request body is required");
  const reader = request.body.getReader();
  const chunks = [];
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > maxBodyBytes) {
        await reader.cancel();
        throw new ProcedureError("PAYLOAD_TOO_LARGE", "Workspace request is too large");
      }
      chunks.push(Buffer.from(value));
    }
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch (error) {
    if (error instanceof ProcedureError) throw error;
    throw new ProcedureError("BAD_REQUEST", "Invalid JSON request");
  } finally {
    reader.releaseLock();
  }
}

function sessionData(state, access) {
  const flags = { aiGateway: true, topics: true };
  const tenantId = "organization/" + organization;
  const policy = (actions, resource) => ({
    tenantId, actions, resources: [resource], effect: "ALLOW", roleId: "system/OWNER",
  });
  return {
    expires: new Date(Date.now() + 86_400_000).toISOString(),
    policies: [[policy(access.organization, tenantId), policy(access.project, "project/*")]],
    environment: {
      v4WriteMode: "dual", selfHostedInstancePlan: "oss", publicSharingDisabledByInstance: true,
      accessAuditLogMode: "none",
    },
    user: {
      id: "confuse-local", name: "Local", email: "", image: null, admin: false, hasPassword: false,
      featureFlags: flags, canCreateOrganizations: false, v4BetaEnabled: true,
      organizations: [{
        id: organization, name: "Confuse",
        plan: "oss", metadata: {}, featureFlags: flags,
        aiFeaturesEnabled: true, aiTelemetryEnabled: false, publicSharingDisabled: true,
        projects: state.projects,
      }],
    },
  };
}

function statusAndError(error, procedure) {
  const name = error instanceof ProcedureError ? error.code : "INTERNAL_SERVER_ERROR";
  const [code, httpStatus] = trpcCodes[name];
  const message = error.message || "Workspace operation failed";
  return {
    httpStatus,
    body: { error: { json: { code, message, data: { code: name, httpStatus, path: procedure } } } },
  };
}

async function execute(backend, name, method, payload) {
  try {
    const procedure = Object.hasOwn(procedures, name) ? procedures[name] : null;
    if (!procedure) throw new ProcedureError("NOT_IMPLEMENTED", "Workspace procedure is unavailable: " + name);
    const [type, scope, output] = procedure;
    if (type === "mutation" && method !== "POST") {
      throw new ProcedureError("METHOD_NOT_SUPPORTED", "Use POST for workspace changes");
    }
    const input = payload?.json ?? {};
    if (!input || typeof input !== "object" || Array.isArray(input)) {
      throw new ProcedureError("BAD_REQUEST", "Expected an object");
    }
    if (scope === "project" && (typeof input.projectId !== "string" || input.projectId.length > 128)) {
      throw new ProcedureError("BAD_REQUEST", "Project ID is required");
    }
    if (scope === "organization" && input.orgId !== organization && input.organizationId !== organization) {
      throw new ProcedureError("FORBIDDEN", "Invalid local workspace");
    }
    const result = await backend.request("langfuse.request", { procedure: name, input });
    if (result.error) throw new ProcedureError(Object.hasOwn(trpcCodes, result.error.code) ? result.error.code : "INTERNAL_SERVER_ERROR", result.error.message);
    const { data, dates } = result;
    const values = output === "map" ? ["map"] : dates.length ? Object.fromEntries(dates.map(key => [key, ["Date"]])) : undefined;
    return {
      httpStatus: 200,
      body: { result: { data: { json: data, ...(values ? { meta: { values, v: 1 } } : {}) } } },
    };
  } catch (error) {
    return statusAndError(error, name);
  }
}

function workspaceApi(backend, access) {
  return async (request, url) => {
    const origin = request.headers.get("origin");
    if (origin && origin !== "confuse-ui://workspace" && origin !== "null") {
      return new Response(null, { status: 403 });
    }
    if (request.headers.get("sec-fetch-site") === "cross-site") {
      return new Response(null, { status: 403 });
    }
    const headers = { "cache-control": "no-store", "x-content-type-options": "nosniff" };
    if (url.pathname === "/api/auth/session" && ["GET", "POST"].includes(request.method)) {
      const state = await backend.request("langfuse.session", {});
      return Response.json(sessionData(state, access), { headers });
    }
    if (url.pathname === "/api/chatCompletion" && request.method === "POST") {
      try {
        const input = await requestBody(request);
        if (typeof input?.projectId !== "string") throw new ProcedureError("BAD_REQUEST", "Project ID is required");
        return await backend.playground(input, request.signal);
      } catch (error) {
        if (request.signal.aborted) throw error;
        return Response.json({ message: error.message }, { status: 400, headers });
      }
    }
    const visit = /^\/api\/project\/([a-zA-Z0-9_-]{1,128})\/visit$/.exec(url.pathname);
    if (visit && request.method === "POST") {
      const result = await execute(backend, "projects.visit", "POST", { json: { projectId: visit[1] } });
      return new Response(null, { status: result.httpStatus === 200 ? 204 : result.httpStatus, headers });
    }
    if (url.pathname === "/api/dashboard/execute-query-stream" && request.method === "POST") {
      try {
        const input = await requestBody(request);
        const result = await execute(backend, "dashboard.executeQuery", "POST", { json: input });
        if (result.httpStatus !== 200) {
          return Response.json({ message: result.body.error.json.message }, { status: result.httpStatus, headers });
        }
        const rows = result.body.result.data.json[Symbol.iterator]();
        const encoder = new TextEncoder();
        const body = new ReadableStream({
          pull(controller) {
            const { value, done } = rows.next();
            if (done) {
              controller.enqueue(encoder.encode("event: done\ndata: {}\n\n"));
              controller.close();
            } else {
              controller.enqueue(encoder.encode("event: row\ndata: " + JSON.stringify(value) + "\n\n"));
            }
          },
        });
        return new Response(body, { headers: { ...headers, "content-type": "text/event-stream" } });
      } catch {
        return Response.json({ message: "Invalid dashboard query" }, { status: 400, headers });
      }
    }
    const match = /^\/api\/trpc\/([a-zA-Z0-9_.,]{1,2048})$/.exec(url.pathname);
    if (!match || !["GET", "POST"].includes(request.method)) return new Response(null, { status: 404 });
    const names = match[1].split(",");
    const batch = url.searchParams.get("batch") === "1";
    if (names.length > maxBatch || (!batch && names.length !== 1) ||
        names.some(name => !/^[a-zA-Z][a-zA-Z0-9_]*\.[a-zA-Z][a-zA-Z0-9_]*$/.test(name))) {
      return new Response(null, { status: 400 });
    }
    try {
      const payload = request.method === "POST" ? await requestBody(request) : JSON.parse(url.searchParams.get("input") || "{}");
      const results = await Promise.all(names.map((name, index) => execute(backend, name, request.method, batch ? payload[index] : payload)));
      const statuses = new Set(results.map(item => item.httpStatus));
      return Response.json(batch ? results.map(item => item.body) : results[0].body, {
        headers,
        status: statuses.size > 1 ? 207 : results[0].httpStatus,
      });
    } catch (error) {
      const failure = statusAndError(
        error instanceof ProcedureError ? error : new ProcedureError("BAD_REQUEST", "Invalid tRPC input"),
        match[1],
      );
      return Response.json(batch ? names.map(() => failure.body) : failure.body, {
        headers, status: failure.httpStatus,
      });
    }
  };
}

module.exports = { workspaceApi };
