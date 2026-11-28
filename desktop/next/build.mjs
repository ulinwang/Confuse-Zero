import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { stageSource } from "./source.mjs";

const root = path.dirname(fileURLToPath(import.meta.url));
const workspace = path.join(root, ".workspace");
const web = path.join(workspace, "web");
const shared = path.join(workspace, "packages/shared");
const icons = path.join(workspace, "packages/icons");
const src = path.join(web, "src");
const overlay = path.join(root, "overlay");

function read(file) { return readFileSync(file, "utf8"); }
function json(file) { return JSON.parse(read(file)); }
function write(file, data) {
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, data);
}
function replace(file, before, after) {
  const original = read(file);
  if (original.indexOf(before) === -1 || original.indexOf(before) !== original.lastIndexOf(before)) {
    throw new Error("Langfuse source adaptation did not match once in " + path.relative(src, file));
  }
  write(file, original.replace(before, after));
}
function removeBetween(file, start, end) {
  const original = read(file);
  const from = original.indexOf(start);
  const to = original.indexOf(end, from + start.length);
  if (from === -1 || to === -1) throw new Error("Missing Langfuse source boundary in " + path.relative(src, file));
  write(file, original.slice(0, from) + original.slice(to));
}

stageSource(workspace);
const dependencies = path.join(root, "node_modules");
const moduleSearch = path.join(web, "node_modules");
symlinkSync(dependencies, moduleSearch, "dir");

if (existsSync(overlay)) cpSync(overlay, web, { recursive: true });
cpSync(path.join(root, "next.config.js"), path.join(web, "next.config.js"));
write(path.join(web, "tsconfig.json"), JSON.stringify({
  compilerOptions: {
    target: "es2022", module: "esnext", moduleResolution: "bundler", jsx: "preserve",
    isolatedModules: true, resolveJsonModule: true, noEmit: true,
    paths: { "@/*": ["./*"] },
  },
}, null, 2));
// Next detects TypeScript by the manifest in the app directory.
cpSync(path.join(root, "package.json"), path.join(web, "package.json"));

replace(path.join(src, "utils/api.ts"), "return window.location.origin;", 'return window.location.protocol === "confuse-ui:" ? "" : window.location.origin;');
replace(path.join(src, "env.mjs"), 'skipValidation: process.env.DOCKER_BUILD === "1",', "skipValidation: true,");
replace(path.join(src, "env.mjs"), "process.env.NEXT_PUBLIC_LANGFUSE_PLAYGROUND_STREAMING_ENABLED_DEFAULT,", '"true",');

const app = path.join(src, "pages/_app.tsx");
replace(app, 'import prexit from "prexit";\n', "");
replace(app, read(app).slice(read(app).lastIndexOf('\nif (\n  process.env.NEXT_RUNTIME')), "\n");
replace(app, 'import { type AppType } from "next/app";',
  'import App, { type AppType } from "next/app";');
replace(app, 'const MyApp: AppType<{ session: Session | null }>', 'const MyApp: AppType<{ session: Session | null; cspNonce?: string }>');
replace(app, 'pageProps: { session, ...pageProps },', 'pageProps: { session, cspNonce, ...pageProps },');
replace(app, '                          attribute="class"', '                          nonce={cspNonce}\n                          attribute="class"');
replace(app, 'export default api.withTRPC(MyApp);', `MyApp.getInitialProps = async (context) => {
  const props = await App.getInitialProps(context);
  const cspNonce = context.ctx.req?.headers["x-confuse-nonce"];
  return { ...props, pageProps: { ...props.pageProps, ...(typeof cspNonce === "string" && { cspNonce }) } };
};

export default api.withTRPC(MyApp);`);

const document = path.join(src, "pages/_document.tsx");
replace(document, 'import { Html, Head, Main, NextScript } from "next/document";',
  'import NextDocument, { Html, Head, Main, NextScript, type DocumentContext } from "next/document";');
replace(document, 'function Document()', 'function Document({ nonce }: { nonce?: string })');
replace(document, "<Head />", "<Head nonce={nonce} />");
replace(document, "<NextScript />", "<NextScript nonce={nonce} />");
write(document, read(document) + `
Document.getInitialProps = async (context: DocumentContext) => {
  const props = await NextDocument.getInitialProps(context);
  const nonce = context.req?.headers["x-confuse-nonce"];
  return { ...props, ...(typeof nonce === "string" && { nonce }) };
};
`);

const evals = path.join(src, "pages/project/[projectId]/evals/index.tsx");
replace(
  evals,
  read(evals).split('export { default } from "@/src/features/evals/v2/pages/EvaluatorsPage";')[0],
  "",
);
const gateway = path.join(src, "pages/organization/[organizationId]/ai-gateway/[page].tsx");
replace(gateway, 'import { prisma } from "@langfuse/shared/src/db";\n', "");
replace(gateway, 'import { getGatewaySetupRedirect } from "@/src/features/ai-gateway/server/gatewaySetupRedirect";\n', "");
replace(gateway, 'import { getServerAuthSession } from "@/src/server/auth";\n', "");
replace(gateway, read(gateway).slice(read(gateway).indexOf("  const session = await ")), "  return { props: {} };\n};\n");

const configRedirect = path.join(src, "features/evals/server/getEvalConfigsConfigRedirectPageServerSideProps.ts");
replace(configRedirect, 'import { prisma } from "@langfuse/shared/src/db";\n', "");
removeBetween(configRedirect, "  const evaluator = await ", "  return {\n    redirect:");

const settings = path.join(src, "features/projects/ProjectSettingsPage.tsx");
for (const prefix of [
  "import { AuditLogsSettingsPage }",
  "import { useExternalMediaStorage }",
  "import { ExternalMediaStorageIntegrationCard }",
]) {
  const line = read(settings).split("\n").find(value => value.startsWith(prefix));
  if (!line) throw new Error("Missing project settings import " + prefix);
  replace(settings, line + "\n", "");
}
removeBetween(settings, '  {\n    title: "Members",', '  {\n    title: "Exports",');
removeBetween(settings, '  {\n    title: "Audit Logs",', '];\n');
const integrations = read(settings).indexOf("\nconst Integrations = ");
if (integrations === -1) throw new Error("Missing project settings integrations");
write(settings, read(settings).slice(0, integrations) + "\n");
write(settings, 'import { LocalExport } from "@/src/confuse/DesktopControls";\n' + read(settings));
replace(settings, "    content: <BatchExportsSettingsPage projectId={project.id} />,", `    content: (
      <div className="flex flex-col gap-6">
        <LocalExport project={project.id} />
        <BatchExportsSettingsPage projectId={project.id} />
      </div>
    ),`);

const classicSidebar = path.join(src, "components/nav/AppSidebar/AppSidebar.tsx");
replace(classicSidebar, 'import * as React from "react";',
  'import * as React from "react";\nimport { DesktopControls } from "@/src/confuse/DesktopControls";');
replace(classicSidebar, "  SidebarRail,\n", "  SidebarRail,\n  useSidebar,\n");
replace(classicSidebar, "  const activeNotifications =", '  const isCollapsed = useSidebar().state === "collapsed";\n  const activeNotifications =');
replace(classicSidebar, "      <SidebarFooter>\n", "      <SidebarFooter>\n        <DesktopControls collapsed={isCollapsed && !isMobile} />\n");
const sidebar = path.join(src, "features/app-shell/components/AppShellSidebarPanel.tsx");
write(sidebar, 'import { DesktopControls } from "@/src/confuse/DesktopControls";\n' + read(sidebar));
replace(sidebar, "      <SidebarPanel.Footer>\n", "      <SidebarPanel.Footer>\n        <DesktopControls collapsed={isCollapsed} />\n");
const navigationFilters = path.join(src, "components/layouts/app-layout/utils/navigationFilters.ts");
replace(navigationFilters, "      ctx.uiCustomization &&\n", "      ctx.uiCustomization?.visibleModules &&\n");
for (const [file, fragment] of [
  [classicSidebar, '  const activeNotifications = !v4UpgradeUiEnabled\n    ? getActiveSidebarNotifications(notificationState.dismissedIds)\n    : [];'],
  [sidebar, '  const activeNotifications = v4UpgradeUiEnabled\n    ? []\n    : getActiveSidebarNotifications(notificationState.dismissedIds);'],
]) replace(file, fragment, "  const activeNotifications: ReturnType<typeof getActiveSidebarNotifications> = [];");

const playgroundProvider = path.join(src, "features/playground/page/context/PlaygroundProvider.tsx");
replace(playgroundProvider, "  const [isStreaming, setIsStreaming] = useState(false);", `  const [isStreaming, setIsStreaming] = useState(false);
  const activeCompletion = useRef<AbortController | null>(null);
  useEffect(() => () => activeCompletion.current?.abort(), [projectId]);`);
replace(playgroundProvider, "    async (streaming = true) => {\n      try {", `    async (streaming = true) => {
      if (activeCompletion.current) {
        activeCompletion.current.abort();
        return;
      }
      const controller = new AbortController();
      activeCompletion.current = controller;
      const desktopRequest = { signal: controller.signal, sourcePrompt };
      try {`);
for (const fragment of [
  "            tools,\n            streaming,",
  "            structuredOutputSchema,\n            streaming,",
  "            const completionStream = getChatCompletionStream(\n              projectId,\n              finalMessages,\n              modelParams,",
  "            const result = await getChatCompletionNonStreaming(\n              projectId,\n              finalMessages,\n              modelParams,",
]) {
  replace(playgroundProvider, fragment, fragment + (fragment.startsWith("            const") ? "\n              desktopRequest," : "\n            desktopRequest,"));
}
replace(playgroundProvider, '      } catch (err) {\n        const errorMessage =', '      } catch (err) {\n        if (controller.signal.aborted) return;\n        const errorMessage =');
replace(playgroundProvider, '      } finally {\n        setIsStreaming(false);', '      } finally {\n        activeCompletion.current = null;\n        setIsStreaming(false);');
let calls = read(playgroundProvider);
const origin = calls.indexOf("async function getChatCompletionWithTools(");
if (origin === -1) throw new Error("Missing upstream Playground completion helper");
const helpers = calls.slice(origin)
  .replaceAll("  streaming = false,\n", "  streaming: boolean,\n  desktopRequest: DesktopCompletionRequest,\n")
  .replaceAll("  modelParams: UIModelParams,\n) {", "  modelParams: UIModelParams,\n  desktopRequest: DesktopCompletionRequest,\n) {")
  .replace("  modelParams: UIModelParams,\n): Promise<{ content: string;", "  modelParams: UIModelParams,\n  desktopRequest: DesktopCompletionRequest,\n): Promise<{ content: string;")
  .replaceAll("    modelParams: getFinalModelParams(modelParams),", "    modelParams: getFinalModelParams(modelParams),\n    sourcePrompt: desktopRequest.sourcePrompt,")
  .replaceAll('method: "POST",\n      headers:', 'method: "POST",\n      signal: desktopRequest.signal,\n      headers:')
  .replace("const token = decoder.decode(value);", "const token = decoder.decode(value, { stream: true });")
  .replace("      yield token;\n    }", "      yield token;\n    }\n    const tail = decoder.decode();\n    if (tail) yield tail;");
write(playgroundProvider, calls.slice(0, origin) + "type DesktopCompletionRequest = { signal: AbortSignal; sourcePrompt: PlaygroundSourcePrompt | null };\n\n" + helpers);
const submitButton = path.join(src, "features/playground/page/components/Messages.tsx");
replace(submitButton, "        loading={isStreaming}\n", "");
replace(submitButton, "<p>Submit</p>", '<p>{isStreaming ? "Stop" : "Submit"}</p>');

const prismaSchema = read(path.join(shared, "prisma/schema.prisma"));
const enums = Array.from(prismaSchema.matchAll(/^enum (\w+) \{\n([^}]+)^\}/gm), ([, name, body]) => {
  const variants = Array.from(body.matchAll(/^\s*([A-Z_a-z]\w*)\s*(?:@[^\n]+|\/\/[^\n]*)?$/gm), match => match[1]);
  return "export const " + name + " = " + JSON.stringify(Object.fromEntries(variants.map(variant => [variant, variant]))) + " as const;";
});
if (enums.length === 0) throw new Error("Upstream enum declarations missing");
write(path.join(shared, "src/db.ts"), enums.join("\n") + "\nexport { default as Decimal } from 'decimal.js';\n");

const aliases = {};
for (const [name, spec] of Object.entries(json(path.join(shared, "package.json")).exports)) {
  if (!spec?.import?.startsWith("./dist/src/")) continue;
  const destination = path.join(shared, spec.import.replace("./dist/src/", "src/").replace(/\.js$/, ""));
  aliases["@langfuse/shared" + name.slice(1) + "$"] = destination;
}
aliases["@prisma/client$"] = path.join(shared, "src/db.ts");
aliases["@repo/langfuse-skills$"] = path.join(workspace, "packages/langfuse-skills/src/index.js");
for (const entry of ["index", "si", "brand", "tb"]) {
  aliases["@langfuse/icons" + (entry === "index" ? "" : "/" + entry) + "$"] = path.join(icons, "dist", entry + ".js");
}
const adapterAliases = path.join(root, "aliases.json");
if (existsSync(adapterAliases)) {
  for (const [name, file] of Object.entries(json(adapterAliases))) aliases[name + "$"] = path.join(src, "confuse", file);
}
const relativeAliases = Object.fromEntries(Object.entries(aliases).map(([name, target]) => [
  name.slice(0, -1), "./" + path.relative(web, target),
]));
write(path.join(web, "confuse-aliases.json"), JSON.stringify(relativeAliases, null, 2));

if (!existsSync(path.join(workspace, "node_modules"))) symlinkSync(dependencies, path.join(workspace, "node_modules"), "dir");
await build({
  entryPoints: [path.join(icons, "src/create-icon.tsx"), path.join(icons, "src/brand.tsx")],
  outdir: path.join(icons, "dist"), format: "esm", jsx: "automatic",
});
execFileSync(process.execPath, [path.join(icons, "scripts/generate.cjs")], { stdio: "inherit" });

const catalog = json(path.join(web, "confuse-upstream.json"));
const sha = await import("node:crypto").then(({ createHash }) => filename => createHash("sha256").update(readFileSync(filename)).digest("hex"));
catalog.localAdaptations = Object.fromEntries(Object.entries(catalog.entries).flatMap(([file, original]) => {
  const staged = sha(path.join(web, "src/pages", file));
  return original === staged ? [] : [[file, staged]];
}));
catalog.access = Object.fromEntries(["project", "organization"].map(scope => {
  const rights = read(path.join(shared, "src/features/rbac", scope + "AccessRights.ts"));
  const list = rights.match(new RegExp("export const " + scope + "Scopes = \\[([^\\]]+)\\]"));
  if (!list) throw new Error("Invalid upstream " + scope + " access vocabulary");
  return [scope, Array.from(list[1].matchAll(/^\s*"([^"]+)",/gm), match => match[1])];
}));
write(path.join(web, "confuse-upstream.json"), JSON.stringify(catalog, null, 2) + "\n");

if (!process.argv.includes("--stage")) {
  const cleanEnv = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("NEXT_PUBLIC_")));
  execFileSync(process.execPath, [path.join(dependencies, "next/dist/bin/next"), "build"], {
    cwd: web,
    env: { ...cleanEnv, NEXT_TELEMETRY_DISABLED: "1", NEXT_PUBLIC_BUILD_ID: "confuse-" + catalog.tag },
    stdio: "inherit",
  });
}
