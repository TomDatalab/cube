import { Select } from 'antd';
import { CSSProperties, useEffect, useRef, useState } from 'react';

import type { AdminApi } from './api';

type Option = { value: string; label: string };

type Props = {
  value?: string;
  onChange?: (v?: string) => void;
  fetchOptions: (q: string) => Promise<Option[]>;
  placeholder?: string;
  allowClear?: boolean;
  style?: CSSProperties;
  size?: 'small' | 'middle';
  // The label of `value` when it may not be among the first options.
  valueLabel?: string;
};

const DEBOUNCE_MS = 300;

// A search-as-you-type select: it asks `fetchOptions` for the first matches
// 300 ms after the last key, and remembers the label of the chosen value.
export function RemoteSelect({
  value, onChange, fetchOptions, placeholder, allowClear, style, size, valueLabel,
}: Props) {
  const [options, setOptions] = useState<Option[]>([]);
  const [loading, setLoading] = useState(false);
  const [labels, setLabels] = useState<Record<string, string>>({});
  const fetchRef = useRef(fetchOptions);
  fetchRef.current = fetchOptions;
  const latest = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout>>();

  function run(q: string) {
    const request = ++latest.current;
    setLoading(true);
    fetchRef.current(q).then((loaded) => {
      if (request === latest.current) {
        setOptions(loaded);
        setLoading(false);
      }
    }, () => {
      if (request === latest.current) {
        setOptions([]);
        setLoading(false);
      }
    });
  }

  useEffect(() => {
    run('');
    return () => {
      latest.current++;
      clearTimeout(timer.current);
    };
  }, []);

  const label = value ? (labels[value] ?? valueLabel) : undefined;
  const shown = value && label && !options.some((o) => o.value === value)
    ? [{ value, label }, ...options]
    : options;

  return (
    <Select
      showSearch
      filterOption={false}
      allowClear={allowClear}
      placeholder={placeholder}
      style={style}
      size={size}
      loading={loading}
      value={value}
      options={shown}
      onSearch={(q) => {
        clearTimeout(timer.current);
        timer.current = setTimeout(() => run(q), DEBOUNCE_MS);
      }}
      onChange={(v: string | undefined) => {
        if (v) {
          const chosen = shown.find((o) => o.value === v);
          if (chosen) {
            setLabels((l) => ({ ...l, [v]: chosen.label }));
          }
        }
        onChange?.(v);
      }}
      onDropdownVisibleChange={(open) => {
        // Back to the first matches once the search box is closed.
        if (!open) {
          clearTimeout(timer.current);
          run('');
        }
      }}
    />
  );
}

export const userOptions = (api: AdminApi) => async (q: string): Promise<Option[]> => (
  (await api.searchUsers(q, 1)).items
    .filter((u) => !u.disabled)
    .map((u) => ({ value: u.id, label: u.name ? `${u.id} (${u.name})` : u.id }))
);

export const organizationOptions = (api: AdminApi) => async (q: string): Promise<Option[]> => (
  (await api.searchOrganizations(q, 1)).items.map((o) => ({ value: o.id, label: o.name }))
);
