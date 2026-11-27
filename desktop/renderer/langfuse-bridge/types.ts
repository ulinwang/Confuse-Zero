export function assertUnreachable(value: never): never {
  throw new Error(`Unsupported Langfuse control value: ${String(value)}`);
}
