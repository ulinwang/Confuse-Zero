const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const os = require("node:os");
const { removeInstallation } = require("../uninstall.cjs");

test("uninstall removes its own files and leaves Agent and old Confuse data intact", async () => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "confuse-uninstall-test-"));
  try {
    const own = path.join(root, "Confuse-Zero");
    const agent = path.join(root, "Pi");
    const legacy = path.join(root, "Confuse");
    const bundle = path.join(root, "Confuse Zero.app");
    for (const directory of [own, agent, legacy, bundle]) await fs.mkdir(directory);
    await fs.mkdir(path.join(own, "Desktop"));
    await fs.writeFile(path.join(own, "confuse.sqlite"), "fixture");
    await fs.writeFile(path.join(own, "Desktop", "settings"), "fixture");
    await fs.writeFile(path.join(agent, "settings.json"), '{"name":"fixture"}');
    await fs.writeFile(path.join(legacy, "data"), "old");
    await removeInstallation({ bundle, data:own, trash: source => fs.rename(source, path.join(root,"Trashed.app")) });
    await assert.rejects(fs.access(bundle), { code:"ENOENT" });
    await assert.rejects(fs.access(own), { code:"ENOENT" });
    assert.equal(await fs.readFile(path.join(agent,"settings.json"),"utf8"), '{"name":"fixture"}');
    assert.equal(await fs.readFile(path.join(legacy,"data"),"utf8"), "old");
    await assert.rejects(removeInstallation({ bundle:path.join(root,"Confuse.app"), data:legacy, trash:async()=>{} }), /could not be verified/);
    assert.equal(await fs.readFile(path.join(legacy,"data"),"utf8"), "old");
  } finally {
    await fs.rm(root, { recursive:true, force:true });
  }
});
