type NativeBridge = {
  invoke: (channel: string, payload: object) => Promise<unknown>;
  onReopenSetup: (listener: () => void) => () => void;
  onUninstallRequested: (listener: () => void) => () => void;
};

declare global {
  interface Window {
    confuse?: NativeBridge;
  }
}

export type DesktopState = {
  onboarded: boolean;
  sdkEndpoint: string | null;
  listenerError: string | null;
};

export function invoke<T>(channel: string, payload: object = {}): Promise<T> {
  if (typeof window === "undefined" || !window.confuse) {
    return Promise.reject(new Error("Confuse desktop connection is unavailable"));
  }
  return window.confuse.invoke(channel, payload) as Promise<T>;
}
