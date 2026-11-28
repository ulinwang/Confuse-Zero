import { ErrorPage } from "@/src/components/ErrorPage";

export function SupportOrUpgradePage() {
  return <ErrorPage title="Feature unavailable" message="This feature is unavailable in this workspace." />;
}

export function BedrockRoleSetupInstructions() {
  return <ErrorPage title="Bedrock IAM" message="This connection type requires a Bedrock role." />;
}
