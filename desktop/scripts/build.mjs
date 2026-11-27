import { build } from "esbuild";
import { execFileSync } from "node:child_process";
import { copyFile, mkdir } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";

const require = createRequire(import.meta.url);
const upstream = path.resolve("renderer/langfuse/web/src");
const bridge = path.resolve("renderer/langfuse-bridge");
const adapters = new Map(Object.entries({
  "next/link": "navigation.jsx",
  "next/router": "navigation.jsx",
  "next-themes": "theme.jsx",
  "@langfuse/icons": "icons.ts",
  "@/src/components/layouts/routes": "routes.js",
  "@/src/features/posthog-analytics/usePostHogClientCapture": "analytics.js",
  "@/src/utils/clipboard": "clipboard.js",
  "@/src/utils/types": "types.ts",
  "./useTopicsCapture": "analytics.js",
  "./facet-topics": "facet-topics.ts",
}));

await mkdir("build", { recursive: true });
await build({
  entryPoints: ["renderer/app.jsx"],
  outfile: "build/app.js",
  bundle: true,
  jsx: "automatic",
  minify: true,
  define: { "process.env.NODE_ENV": '"production"' },
  plugins: [{
    name: "langfuse-renderer",
    setup(build) {
      build.onResolve({ filter: /^(@\/src\/|@langfuse\/icons$|next(?:\/link|\/router|-themes)$|\.\/(?:facet-topics|useTopicsCapture)$)/ }, args => {
        const adapter = adapters.get(args.path);
        if (adapter) return { path: path.join(bridge, adapter) };
        return build.resolve(path.join(upstream, args.path.slice("@/src/".length)), {
          kind: args.kind,
          resolveDir: args.resolveDir,
        });
      });
    }
  }]
});
const tailwind = require("@tailwindcss/cli/package.json").bin.tailwindcss;
const cli = path.resolve(path.dirname(require.resolve("@tailwindcss/cli/package.json")), tailwind);
execFileSync(process.execPath, [cli, "-i", "renderer/langfuse.css", "-o", "build/langfuse.css", "--minify"], { stdio: "inherit" });

await Promise.all([
  copyFile("renderer/index.html", "build/index.html"),
  copyFile("renderer/app.css", "build/desktop.css"),
  copyFile("renderer/langfuse/LICENSE", "build/LANGFUSE-LICENSE.txt"),
]);
