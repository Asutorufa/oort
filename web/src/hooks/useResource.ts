import { useCallback, useEffect, useRef, useState } from "react";
import { errorText } from "../api";

export interface Resource<T> {
  data: T | null;
  error: string | null;
  loading: boolean;
  reload: () => Promise<void>;
}

/** Loads `fetcher()` on mount (and every `pollMs` if given); `reload` refetches on demand. */
export function useResource<T>(fetcher: () => Promise<T>, pollMs?: number): Resource<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const fetchRef = useRef(fetcher);
  fetchRef.current = fetcher;

  const reload = useCallback(async () => {
    try {
      setData(await fetchRef.current());
      setError(null);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    reload();
    if (!pollMs) return;
    const t = window.setInterval(reload, pollMs);
    return () => window.clearInterval(t);
  }, [reload, pollMs]);

  return { data, error, loading, reload };
}
