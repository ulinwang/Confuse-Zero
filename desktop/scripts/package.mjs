import { packager } from "@electron/packager";
import { execFileSync } from "node:child_process";
import { access, mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

const out = path.resolve("..", "dist");
const stage = await mkdtemp(path.join(tmpdir(), "confuse-dmg-"));
const backend = path.resolve("..", "target", "release", "confuse");
const version = "0.1.0";
const destination = path.join(out, "Confuse-Zero-" + version + "-arm64.dmg");
const signIdentity = process.env.CONFUSE_SIGN_IDENTITY || "-";
const signing = signIdentity === "-" ? ["--sign", "-"] : ["--sign", signIdentity, "--options", "runtime", "--timestamp"];

function run(command, args) {
  execFileSync(command, args, { stdio: "inherit" });
}
try {
  await access(backend);
  await mkdir(out, { recursive: true });
  const [packagePath] = await packager({
    dir: ".",
    out: path.join(stage, "package"),
    name: "Confuse Zero",
    appBundleId: "io.github.ulinwang.confuse-zero",
    appVersion: version,
    electronVersion: "45.5.0",
    platform: "darwin",
    arch: "arm64",
    asar: true,
    prune: true,
    ignore: /^\/(renderer|scripts|tests|backend.rs)(\/|$)/,
    extraResource: [backend]
  });
  const bundle = path.join(packagePath, "Confuse Zero.app");
  const resources = path.join(bundle, "Contents", "Resources");
  const { rename, symlink } = await import("node:fs/promises");
  await rename(path.join(resources, "confuse"), path.join(resources, "confuse-backend"));
  run("/usr/libexec/PlistBuddy", ["-c", "Set :CFBundleDisplayName Confuse", path.join(bundle, "Contents", "Info.plist")]);
  run("/usr/bin/codesign", ["--force", "--deep", ...signing, bundle]);
  run("/usr/bin/codesign", ["--verify", "--deep", "--strict", bundle]);
  const volume = path.join(stage, "volume");
  await mkdir(volume);
  await rename(bundle, path.join(volume, "Confuse Zero.app"));
  await symlink("/Applications", path.join(volume, "Applications"));
  run("/usr/bin/hdiutil", ["create", "-ov", "-format", "UDZO", "-volname", "Confuse", "-srcfolder", volume, destination]);
  run("/usr/bin/hdiutil", ["verify", destination]);
  if (process.env.CONFUSE_NOTARY_PROFILE && signIdentity !== "-") {
    run("/usr/bin/xcrun", ["notarytool", "submit", destination, "--keychain-profile", process.env.CONFUSE_NOTARY_PROFILE, "--wait"]);
    run("/usr/bin/xcrun", ["stapler", "staple", destination]);
  }
} finally {
  await rm(stage, { force: true, recursive: true });
}
