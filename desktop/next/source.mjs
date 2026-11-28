import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const revision = "9eea2e60a1222936d78a28a0f0b2cafe690fc2a5";
export const tag = "v4.88.0";
const remote = "https://github.com/langfuse/langfuse.git";
const root = path.dirname(fileURLToPath(import.meta.url));

export function source() {
  const checkout = process.env.CONFUSE_LANGFUSE_SOURCE || path.join(root, ".source");
  if (!existsSync(checkout)) {
    if (process.env.CONFUSE_LANGFUSE_SOURCE) throw new Error("Langfuse checkout does not exist");
    mkdirSync(checkout);
    execFileSync("git", ["-C", checkout, "init", "--quiet"]);
  }
  const hasRevision = () => {
    try {
      execFileSync("git", ["-C", checkout, "cat-file", "-e", revision + "^{commit}"], { stdio: "ignore" });
      return true;
    } catch {
      return false;
    }
  };
  if (!hasRevision()) {
    if (process.env.CONFUSE_LANGFUSE_SOURCE) throw new Error("Langfuse checkout is missing " + revision);
    execFileSync("git", ["-C", checkout, "fetch", "--quiet", "--depth=1", remote, "refs/tags/" + tag]);
    if (!hasRevision()) throw new Error("Langfuse tag does not identify the pinned revision");
  }
  return checkout;
}

const projectRoots = new Set([
  "alerts", "annotation-queues", "dashboards", "datasets", "evals", "experiments",
  "index.tsx", "models.tsx", "observations", "playground.tsx", "prompts",
  "scores", "sessions", "settings", "skills", "topics.tsx", "traces", "users", "widgets",
]);
const projectPrefix = "project/[projectId]/";
const organization = "organization/[organizationId]";

export function isWorkspacePage(file) {
  if (/^_(app|document|error)\.(tsx|ts)$/.test(file)) return true;
  if (file === organization + "/index.tsx" || file.startsWith(organization + "/ai-gateway/")) return true;
  if (!file.startsWith(projectPrefix)) return false;
  const projectPath = file.slice(projectPrefix.length);
  return projectRoots.has(projectPath.split("/")[0]) &&
    projectPath !== "traces/setup.tsx" &&
    !projectPath.startsWith("settings/integrations/") &&
    projectPath !== "settings/billing.ts";
}

const excluded = [
  "web/src/ee/", "packages/shared/src/server/ee/", "web/src/__e2e__/",
  "web/src/__tests__/", "web/src/__mocks__/", "web/src/pages/api/", "web/src/app/",
];
const included = [
  "LICENSE", "web/src", "web/public", "web/postcss.config.cjs",
  "packages/shared/src", "packages/shared/prisma/schema.prisma", "packages/shared/package.json",
  "packages/icons/src", "packages/icons/scripts/generate.cjs", "packages/icons/package.json",
  "packages/langfuse-skills/src",
];

function includedFile(file) {
  if (excluded.some(prefix => file.startsWith(prefix))) return false;
  if (file.startsWith("web/src/pages/")) return isWorkspacePage(file.slice("web/src/pages/".length));
  if (/\.(?:test|clienttest|servertest|stories|spec|gatewaye2e)\.[cm]?[jt]sx?$/.test(file)) return false;
  return ![
    "web/src/instrumentation.ts", "web/src/instrumentation-client.ts", "web/src/initialize.ts",
    "web/src/middleware.ts", "web/src/proxy.ts",
  ].includes(file);
}

export function stageSource(destination) {
  const checkout = source();
  const all = execFileSync("git", ["-C", checkout, "ls-tree", "-r", "-z", "--name-only", revision, "--", ...included], {
    encoding: "utf8",
  }).split("\0").filter(Boolean);
  const files = all.filter(includedFile);
  if (!files.includes("web/src/pages/_app.tsx")) throw new Error("Invalid upstream app entry");
  const scratch = mkdtempSync(path.join(root, ".workspace-source-"));
  try {
    // Read the git object rather than the working copy. Local edits to the
    // optional checkout cannot silently change the imported release.
    for (let i = 0; i < files.length; i += 350) {
      const archive = execFileSync("git", ["-C", checkout, "archive", "--format=tar", revision, "--", ...files.slice(i, i + 350)], {
        maxBuffer: 64 * 1024 * 1024,
      });
      execFileSync("tar", ["-xf", "-", "-C", scratch], { input: archive });
    }
    const entries = Object.fromEntries(files
      .filter(file => file.startsWith("web/src/pages/"))
      .map(file => [file.slice("web/src/pages/".length), createHash("sha256").update(readFileSync(path.join(scratch, file))).digest("hex")]));
    mkdirSync(destination, { recursive: true });
    const publicAssets = files.filter(file => file.startsWith("web/public/"))
      .map(file => file.slice("web/public/".length));
    writeFileSync(path.join(scratch, "web/confuse-upstream.json"), JSON.stringify({ tag, revision, entries, publicAssets }, null, 2) + "\n");
    // Populate the stage only after all files were read from the pinned commit.
    rmSync(path.join(destination, "web"), { recursive: true, force: true });
    rmSync(path.join(destination, "packages"), { recursive: true, force: true });
    execFileSync("cp", ["-R", scratch + "/.", destination]);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}
