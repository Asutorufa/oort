import type { CreateSessionRequest, InputRequest, SessionDetail, SessionSummary } from "./types";

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method,
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }));
    throw new Error(err.error ?? `HTTP ${res.status}`);
  }
  return res.status === 202 || res.status === 204 ? (undefined as T) : res.json();
}

export const api = {
  listSessions: () => request<SessionSummary[]>("GET", "/sessions"),
  getSession: (id: string) => request<SessionDetail>("GET", `/sessions/${id}`),
  createSession: (req: CreateSessionRequest) => request<SessionDetail>("POST", "/sessions", req),
  sendInput: (id: string, input: InputRequest) => request<void>("POST", `/sessions/${id}/input`, input),
  cancel: (id: string) => request<void>("POST", `/sessions/${id}/cancel`),
  eventsUrl: (id: string) => `/api/sessions/${id}/events`,
};
