import { useCallback, useEffect, useRef, useState } from 'react';

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
    fetchRef.current(debounced, page).then((loaded) => {
      if (request === latest.current) {
        setResult(loaded);
        setLoading(false);
      }
    }, () => {
      if (request === latest.current) {
        setLoading(false);
      }
    });
  }, [debounced, page, version, ...deps]);

  const reload = useCallback(() => setVersion((v) => v + 1), []);

  return { q, setQ, page, setPage, result, loading, reload };
}
