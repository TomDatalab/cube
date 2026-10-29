import { useCallback, useEffect, useRef, useState } from 'react';

import { PAGE_SIZE } from './api';
import type { Page } from './api';

const DEBOUNCE_MS = 300;

// A search box over a paged list: `q` is debounced, changing it goes back to
// page 1, and a response that arrives after a newer request is dropped.
// `deps` (e.g. an organization filter) re-run the search and go back to page 1.
export function useSearch<T>(
  fetch: (q: string, page: number) => Promise<Page<T>>,
  deps: unknown[]
) {
  const [q, setQ] = useState('');
  const [debounced, setDebounced] = useState('');
  const [page, setPage] = useState(1);
  const [result, setResult] = useState<Page<T> | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [version, setVersion] = useState(0);
  const latest = useRef(0);
  const fetchRef = useRef(fetch);
  fetchRef.current = fetch;

  useEffect(() => {
    const timer = setTimeout(() => setDebounced(q), DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [q]);

  // A new term or filter starts over at page 1.
  useEffect(() => {
    setPage(1);
  }, [debounced, ...deps]);

  useEffect(() => {
    const request = ++latest.current;
    setLoading(true);
    setError(null);
    fetchRef.current(debounced, page).then((loaded) => {
      if (request === latest.current) {
        setResult(loaded);
        setLoading(false);
        // Deleting the last row of the last page leaves that page empty:
        // go to what is now the last page.
        const last = Math.max(1, Math.ceil(loaded.total / PAGE_SIZE));
        if (!loaded.items.length && page > last) {
          setPage(last);
        }
      }
    }, (e: any) => {
      if (request === latest.current) {
        setError(e?.message || String(e));
        setLoading(false);
      }
    });
    // Drops the response if the search changes or the component unmounts.
    return () => {
      latest.current++;
    };
  }, [debounced, page, version, ...deps]);

  const reload = useCallback(() => setVersion((v) => v + 1), []);

  return { q, setQ, page, setPage, result, loading, error, reload };
}
