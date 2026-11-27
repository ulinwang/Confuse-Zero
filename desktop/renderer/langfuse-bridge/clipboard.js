export function copyTextToClipboard(text) {
  return window.confuse.invoke("desktop.copy", { text });
}

export function copyRichTextToClipboard({ text }) {
  return copyTextToClipboard(text);
}
