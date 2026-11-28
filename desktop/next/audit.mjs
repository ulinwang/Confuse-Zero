import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const root = path.dirname(fileURLToPath(import.meta.url));
const web = path.join(root, ".workspace/web");
const read = file => JSON.parse(readFileSync(file, "utf8"));
const catalog = read(path.join(web, "confuse-upstream.json"));
const pages = read(path.join(web, ".next/server/pages-manifest.json"));
const aliases = read(path.join(web, "confuse-aliases.json"));
const implemented = read(path.join(root, "procedures.json"));
const parsed = new Map();
const roots = new Set(["api", "utils", "apiUtils", "trpc", "trpcClient", "queryUtils"]);
const methods = new Set(["useQuery", "useQueries", "useInfiniteQuery", "useMutation", "fetch", "fetchInfinite", "prefetch", "mutate", "query"]);

function routeName(file) {
  return "/" + file.replace(/\.(?:tsx?|jsx?)$/, "").replace(/(^|\/)index$/, "");
}

function resolveModule(name, importer) {
  let base;
  if (Object.hasOwn(aliases, name)) base = path.resolve(web, aliases[name]);
  else if (name.startsWith("@/")) base = path.resolve(web, name.slice(2));
  else if (name.startsWith(".")) base = path.resolve(path.dirname(importer), name);
  else return null;
  return [
    ...(/\.[cm]?[jt]sx?$/.test(base) ? [base] : []),
    ...[".ts", ".tsx", ".mjs", ".js", "/index.ts", "/index.tsx", "/index.js"].map(ext => base + ext),
  ].find(file => existsSync(file)) ?? null;
}

function analyze(file) {
  const cached = parsed.get(file);
  if (cached) return cached;
  const source = ts.createSourceFile(file, readFileSync(file, "utf8"), ts.ScriptTarget.Latest, true);
  const dependencies = new Set();
  const procedures = new Set();
  function add(module) {
    if (!module || !ts.isStringLiteralLike(module)) return;
    const resolved = resolveModule(module.text, file);
    if (resolved) dependencies.add(resolved);
  }
  function visit(node) {
    if (ts.isImportDeclaration(node) && !node.importClause?.isTypeOnly) {
      const bindings = node.importClause?.namedBindings;
      if (!bindings || !ts.isNamedImports(bindings) ||
        node.importClause.name || bindings.elements.some(binding => !binding.isTypeOnly)) add(node.moduleSpecifier);
    } else if (ts.isExportDeclaration(node) && !node.isTypeOnly) {
      add(node.moduleSpecifier);
    } else if (ts.isCallExpression(node)) {
      if (node.expression.kind === ts.SyntaxKind.ImportKeyword) add(node.arguments[0]);
      if (ts.isPropertyAccessExpression(node.expression)) {
        const parts = node.expression.getText(source).split(".").map(part => part.trim());
        if (parts.length === 4 && roots.has(parts[0]) && methods.has(parts[3])) {
          procedures.add(parts[1] + "." + parts[2]);
        }
      }
    }
    ts.forEachChild(node, visit);
  }
  visit(source);
  const result = { dependencies, procedures };
  parsed.set(file, result);
  return result;
}

function reachable(entrypoints) {
  const pending = [...entrypoints];
  const visited = new Set();
  const procedures = new Set();
  while (pending.length) {
    const file = pending.pop();
    if (visited.has(file)) continue;
    visited.add(file);
    const module = analyze(file);
    for (const dependency of module.dependencies) pending.push(dependency);
    for (const name of module.procedures) procedures.add(name);
  }
  return [...procedures].sort();
}

const routes = Object.keys(catalog.entries).filter(file => !file.startsWith("_"));
const omitted = routes.filter(file => !Object.hasOwn(pages, routeName(file)));
if (omitted.length) throw new Error("Page entry missing from Next.js build: " + omitted.join(", "));
const shell = reachable([path.join(web, "src/pages/_app.tsx")]);
const report = Object.fromEntries(routes.map(file => {
  const calls = reachable([path.join(web, "src/pages", file)]);
  return [routeName(file), {
    source: file,
    procedures: calls,
    unregistered: calls.filter(name => !Object.hasOwn(implemented, name)),
  }];
}));
const shellUnregistered = shell.filter(name => !Object.hasOwn(implemented, name));
const missing = [...new Set([...shellUnregistered, ...Object.values(report).flatMap(route => route.unregistered)])].sort();
if (process.argv.includes("--json")) {
  console.log(JSON.stringify({ revision: catalog.revision, shell, shellUnregistered, routes: report, unregistered: missing }, null, 2));
} else {
  console.log(`${routes.length} upstream workspace routes found in the production build.`);
  console.log(`${Object.keys(implemented).length} Rust procedures registered. ${missing.length} statically referenced procedures need review.`);
  for (const name of missing) console.log(name);
}
if (process.argv.includes("--require-coverage") && missing.length) process.exitCode = 1;
