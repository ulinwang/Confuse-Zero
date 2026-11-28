const path = require("node:path");
const aliases = require("./confuse-aliases.json");

/** @type {import("next").NextConfig} */
module.exports = {
  poweredByHeader: false,
  reactStrictMode: true,
  generateBuildId: async () => process.env.NEXT_PUBLIC_BUILD_ID || null,
  images: { unoptimized: true },
  // The bundled application is served without a metrics/release server.
  env: { NEXT_PUBLIC_LANGFUSE_PLAYGROUND_STREAMING_ENABLED_DEFAULT: "true" },
  experimental: { cpus: 3, turbopackFileSystemCacheForBuild: false, turbopackWorkerAssetPrefix: "" },
  outputFileTracingRoot: path.resolve(__dirname, "../.."),
  // Upstream's type-only imports describe the original server. The desktop
  // build keeps these UI contracts and verifies local operations separately.
  typescript: { ignoreBuildErrors: true },
  compiler: { define: { "import.meta.vitest": "undefined" } },
  turbopack: {
    root: path.resolve(__dirname, "../.."),
    resolveAlias: aliases,
  },
};
