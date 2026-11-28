# Confuse

Confuse runs on Apple Silicon macOS. The Electron desktop shell starts a Rust backend that stores Agent and Langfuse SDK telemetry in SQLite. The desktop renderer uses Langfuse v4.88.0 source components for navigation, tables, trace details and Topics. Kimi Code, Codex, Claude Code, DeepSeek Harness, and Pi can be connected from the first-run dialog or Settings. [Renderer source and license](desktop/renderer/langfuse/UPSTREAM.md)

## Install

Mount the arm64 DMG and drag **Confuse Zero** into Applications. In the app, choose which Agents to connect and whether to import each Agent's existing sessions. Closing the window keeps collection active. Quit Confuse to stop it. On the next start, the backend reads the Agent records appended while Confuse was closed.

Settings shows the local SDK address and creates project keys. Configure model connections for Topics, Playground, evaluations, and AI Gateway. Topics uses the selected project model endpoints to summarize traces and embed and name the summaries.

Use **Confuse → Uninstall Confuse…** to clear the installed app's data and model keys. The app uses its own data root and reads selected Agent directories.

## Build and verify

On an Apple Silicon Mac with Xcode command line tools and Node.js 24, run:

```sh
cargo build --locked
npm ci --prefix desktop
npm run build --prefix desktop
npm start --prefix desktop
```

Run the Rust and desktop tests with `cargo test --locked` and `npm test --prefix desktop`. Run `./scripts/verify-sdk.sh` to exercise official Python 5.2.0 and JS/TS 6.1.1 SDKs against a temporary local project. SDK verification uses Python 3.12 and Node.js 24.

Run `cargo build --release --locked`, then `npm run package --prefix desktop` to build the local-install DMG under `dist`. By default the bundle is ad hoc signed for local validation. For distribution, set `CONFUSE_SIGN_IDENTITY` to a Developer ID Application identity and `CONFUSE_NOTARY_PROFILE` to the configured notarytool Keychain profile.

### Next.js workspace preview

The preview builds the Langfuse v4.88.0 workspace from commit `9eea2e60a1222936d78a28a0f0b2cafe690fc2a5`. The importer reads the pinned Git objects, excludes enterprise source directories and writes the original page hashes into the build manifest. Set `CONFUSE_LANGFUSE_SOURCE` to reuse a local checkout containing that commit. The build uses Node 24. The Electron preview uses its bundled Node runtime.

```sh
cargo build --locked
npm ci --prefix desktop
npm ci --prefix desktop/next --ignore-scripts --legacy-peer-deps
npm run build:next --prefix desktop
npm run audit:next --prefix desktop
npm run start:next --prefix desktop
```

Set `CONFUSE_DATA_DIR` to an isolated directory for fixture testing. The audit reports page entry coverage and tRPC references by route. Add `-- --json` for the complete report.

The preview is development-only while the workspace operation and installation checks in the acceptance matrix are open.

[Acceptance rules](docs/acceptance.md) · [Verification matrix](docs/acceptance.tsv)
