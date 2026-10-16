export function messageFrom(cause: unknown): string {
  if (cause instanceof Error) return cause.message;
  if (typeof cause === 'object' && cause !== null && 'message' in cause) {
    const value = cause as { message: unknown; code?: unknown };
    return `${String(value.message)}${value.code ? ` (${String(value.code)})` : ''}`;
  }
  return String(cause);
}

