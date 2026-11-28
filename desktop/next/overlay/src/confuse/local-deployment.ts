import { useEffect, useState } from "react";
import { type DesktopState, invoke } from "./native";

export function useUiCustomization() {
  const [hostname, setHostname] = useState<string>();
  useEffect(() => {
    let active = true;
    invoke<DesktopState>("desktop.get-state")
      .then((state) => { if (active) setHostname(state.sdkEndpoint ?? undefined); })
      .catch(() => {});
    return () => { active = false; };
  }, []);
  return hostname ? { hostname } : undefined;
}

export function findCurrentInstance() {
  return undefined;
}

export function isCloudBilling() {
  return false;
}

export const useIsCloudBillingAvailable = isCloudBilling;

export function useOrganizationSettingsPages() {
  return [];
}

export function useBedrockExternalId() {
  return { reveal: undefined, externalId: null, isLoading: false };
}

export function usePublicSharingDisabledReason() {
  return "Public links are unavailable in a local workspace";
}

export function useHasExternalMediaStorageAccess() {
  return false;
}
