import { useCallback, useEffect, useRef, useState } from 'react';
import { roleBApi } from '../lib/api';
import type { DashboardData } from '../types';

export function useIntents() {
  const [data, setData] = useState<DashboardData | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef<Promise<DashboardData> | null>(null);

  const load = useCallback(async () => {
    if (inFlight.current) return inFlight.current;
    setLoading(true);
    setError(null);
    const next = roleBApi.dashboard()
      .then((dashboard) => {
        setData(dashboard);
        return dashboard;
      })
      .catch((reason: unknown) => {
        const message = reason instanceof Error ? reason.message : 'Role B is unavailable.';
        setError(message);
        throw reason;
      })
      .finally(() => {
        setLoading(false);
        inFlight.current = null;
      });
    inFlight.current = next;
    return next;
  }, []);

  useEffect(() => {
    void load().catch(() => undefined);
  }, [load]);

  return { data, loading, error, load, setData };
}
