import { build } from "esbuild";
import { copyFile, mkdir } from "node:fs/promises";

await mkdir("build", { recursive: true });
await build({
  entryPoints: ["renderer/app.jsx"],
  outfile: "build/app.js",
  bundle: true,
  minify: true,
  define: { "process.env.NODE_ENV": '"production"' }
});
await copyFile("renderer/index.html", "build/index.html");
await copyFile("renderer/app.css", "build/app.css");
