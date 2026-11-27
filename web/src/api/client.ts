/** Error with the server's `{"error": "..."}` message and HTTP status. */
export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

export async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method,
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }));
    throw new ApiError(err.error ?? `HTTP ${res.status}`, res.status);
  }
  if (res.status === 202 || res.status === 204) return undefined as T;
  return res.json();
}

export const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));
