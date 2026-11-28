const APP_SCHEME = "confuse-ui:";
const APP_HOST = "workspace";

function segments(pathname) {
  if (!pathname.startsWith("/") || pathname.length > 8_192) return null;
  try {
    const parts = pathname.split("/").slice(1).filter((part, index, all) => part || index < all.length - 1);
    const decoded = parts.map(part => decodeURIComponent(part));
    return decoded.every(part => part && part !== "." && part !== ".." && !/[\/\\\u0000-\u001f\u007f]/.test(part))
      ? decoded
      : null;
  } catch {
    return null;
  }
}

function pageMatcher(entry) {
  const pattern = entry.replace(/\.(?:tsx?|jsx?)$/, "").replace(/(^|\/)index$/, "").split("/").filter(Boolean);
  return parts => {
    for (let i = 0; i < pattern.length; i++) {
      if (pattern[i].startsWith("[[...")) return true;
      if (pattern[i].startsWith("[...")) return parts.length > i;
      if (parts[i] === undefined || (!pattern[i].startsWith("[") && parts[i] !== pattern[i])) return false;
    }
    return parts.length === pattern.length;
  };
}

function workspacePaths(catalog, buildId) {
  const pages = Object.keys(catalog.entries).filter(entry => !entry.startsWith("_")).map(pageMatcher);
  const publicAssets = new Set(catalog.publicAssets);
  return {
    page(pathname) {
      const parts = segments(pathname);
      return parts !== null && (parts.length === 0 || pages.some(match => match(parts)));
    },
    render(pathname) {
      if (this.page(pathname)) return true;
      const parts = segments(pathname);
      if (!parts) return false;
      if (publicAssets.has(parts.join("/"))) return true;
      if (parts[0] !== "_next") return false;
      if (parts[1] === "static" && parts.length > 2) {
        return parts.slice(2).every(part => /^[a-zA-Z0-9._~\[\]@()+-]+$/.test(part));
      }
      if (parts[1] !== "data" || parts[2] !== buildId || !parts.at(-1)?.endsWith(".json")) return false;
      const dataPage = parts.slice(3);
      if (dataPage.length === 0) return false;
      dataPage[dataPage.length - 1] = dataPage.at(-1).slice(0, -5);
      return dataPage.join("/") === "index" || pages.some(match => match(dataPage));
    },
  };
}

function appUrl(url) {
  try {
    const parsed = new URL(url);
    if (parsed.protocol === APP_SCHEME && parsed.host === APP_HOST && !parsed.username && !parsed.password) return parsed;
  } catch {}
  return null;
}

module.exports = { appUrl, workspacePaths };
