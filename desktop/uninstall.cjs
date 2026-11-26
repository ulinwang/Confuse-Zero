const { rm, rmdir } = require("node:fs/promises");
const path = require("node:path");

function verifyBundle(bundle) {
  if (!path.isAbsolute(bundle) || path.basename(bundle) !== "Confuse Zero.app") {
    throw new Error("Confuse installation path could not be verified");
  }
}

async function removeInstallation({ bundle, data, trash }) {
  verifyBundle(bundle);
  for (const filename of ["confuse.sqlite", "confuse.sqlite-shm", "confuse.sqlite-wal", "Desktop"]) {
    await rm(path.join(data, filename), { recursive: filename === "Desktop", force: true });
  }
  await rmdir(data).catch(error => {
    if (!["ENOTEMPTY","ENOENT"].includes(error.code)) throw error;
  });
  await trash(bundle);
}

module.exports = { verifyBundle, removeInstallation };
