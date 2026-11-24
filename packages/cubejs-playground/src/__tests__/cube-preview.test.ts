import { describe, expect, it, vi } from 'vitest';

import {
  formatValue, isNumeric, loadRows, previewHeaders, previewQueries,
} from '../pages/Schema/preview';

const cube = {
  name: 'cables',
  measures: [
    { name: 'cables.count', type: 'count' },
    { name: 'cables.secret_total', type: 'sum', public: false },
  ],
  dimensions: [
    { name: 'cables.id', type: 'string', primaryKey: true },
    { name: 'cables.maker', type: 'string' },
    { name: 'cables.cost', type: 'number', public: false },
  ],
};

describe('previewQueries', () => {
  it('asks for raw rows of every dimension, and the measures over everything', () => {
    expect(previewQueries(cube, { limit: 50, hidden: true })).toEqual({
      rows: { dimensions: ['cables.id', 'cables.maker', 'cables.cost'], ungrouped: true, limit: 50 },
      totals: { measures: ['cables.count', 'cables.secret_total'] },
    });
  });

  it('leaves hidden members out for those who cannot query them', () => {
    expect(previewQueries(cube, { limit: 10, hidden: false })).toEqual({
      rows: { dimensions: ['cables.id', 'cables.maker'], ungrouped: true, limit: 10 },
      totals: { measures: ['cables.count'] },
    });
  });

  it('has no query for a kind of member the cube lacks', () => {
    expect(previewQueries({ name: 'c', measures: [], dimensions: [] }, { limit: 10, hidden: true }))
      .toEqual({ rows: null, totals: null });
  });
});

describe('previewHeaders', () => {
  it('queries an admin in the organization of the cube', () => {
    expect(previewHeaders({ isAdmin: true, slug: 'bossa', workspace: null })).toEqual({ 'X-Cube-Organization': 'bossa' });
    expect(previewHeaders({ isAdmin: true, slug: '_platform', workspace: 'w1' })).toEqual({ 'X-Cube-Organization': '_platform' });
  });

  it('queries an owner in the chosen workspace, and says so when none is', () => {
    expect(previewHeaders({ isAdmin: false, slug: 'bossa', workspace: 'w1' })).toEqual({ 'X-Cube-Workspace': 'w1' });
    expect(previewHeaders({ isAdmin: false, slug: 'bossa', workspace: null })).toBeNull();
  });
});

describe('loadRows', () => {
  const answer = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

  it('returns the rows', async () => {
    const doFetch = vi.fn(async () => answer({ data: [{ 'cables.id': '1' }] }));
    expect(await loadRows('/cube/v1/load', { a: 'b' }, { measures: ['m'] }, doFetch)).toEqual([{ 'cables.id': '1' }]);
    const [url, init] = doFetch.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('/cube/v1/load');
    expect(init.method).toBe('POST');
    expect(JSON.parse(String(init.body))).toEqual({ query: { measures: ['m'] } });
    expect(init.headers).toMatchObject({ a: 'b', 'Content-Type': 'application/json' });
  });

  it('asks again while the server says to wait', async () => {
    const doFetch = vi.fn()
      .mockResolvedValueOnce(answer({ error: 'Continue wait' }))
      .mockResolvedValueOnce(answer({ data: [] }));
    expect(await loadRows('/x', {}, {}, doFetch, 0)).toEqual([]);
    expect(doFetch).toHaveBeenCalledTimes(2);
  });

  it('throws the server error', async () => {
    const doFetch = vi.fn(async () => answer({ error: 'Permission denied: cables.cost' }, 403));
    await expect(loadRows('/x', {}, {}, doFetch)).rejects.toThrow('Permission denied: cables.cost');
  });
});

describe('formatValue', () => {
  it('shows a time as its date, with the hour when there is one', () => {
    expect(formatValue('2025-08-13T00:00:00.000', 'time')).toBe('2025-08-13');
    expect(formatValue('2025-08-13T14:05:09.000', 'time')).toBe('2025-08-13 14:05:09');
    expect(formatValue('2025-08-13T14:05:00.000', 'time')).toBe('2025-08-13 14:05');
    expect(formatValue('not a date', 'time')).toBe('not a date');
  });

  it('groups the digits of a number and keeps its decimals', () => {
    expect(formatValue('1234567', 'number')).toBe('1,234,567');
    expect(formatValue('1234.5', 'number')).toBe('1,234.5');
    expect(formatValue(42, 'count')).toBe('42');
    expect(formatValue('12abc', 'number')).toBe('12abc');
  });

  it('leaves text and booleans as they are, and null as null', () => {
    expect(formatValue('OFS', 'string')).toBe('OFS');
    expect(formatValue(true, 'boolean')).toBe('true');
    expect(formatValue(null, 'string')).toBeNull();
    expect(formatValue(undefined, 'number')).toBeNull();
  });
});

describe('isNumeric', () => {
  it('is true for number dimensions and every measure that counts or sums', () => {
    expect(['number', 'count', 'count_distinct', 'countDistinct', 'sum', 'avg', 'min', 'max'].every(isNumeric)).toBe(true);
    expect(['string', 'time', 'boolean', undefined].some(isNumeric)).toBe(false);
  });
});
