import { useLayoutEffect, useSyncExternalStore } from "react";

const query = window.matchMedia("(prefers-color-scheme: dark)");

function subscribe(callback) {
  query.addEventListener("change", callback);
  return () => query.removeEventListener("change", callback);
}

export function useTheme() {
  const dark = useSyncExternalStore(subscribe, () => query.matches);
  return { resolvedTheme: dark ? "dark" : "light" };
}

export function DesktopTheme({ children }) {
  const { resolvedTheme } = useTheme();
  useLayoutEffect(() => {
    document.documentElement.classList.toggle("dark", resolvedTheme === "dark");
  }, [resolvedTheme]);
  return children;
}
