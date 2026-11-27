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

[Acceptance rules](docs/acceptance.md) · [Verification matrix](docs/acceptance.tsv)
