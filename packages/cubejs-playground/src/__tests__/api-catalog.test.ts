import { readFileSync } from 'fs';
import { resolve } from 'path';
import { describe, expect, it } from 'vitest';

import {
  endpointKey, fillPath, filterSections, pathParams, SECTIONS,
} from '../pages/Api/catalog';

// The server's routers; /playground/* (the Playground's own helpers) is left out.
const SERVER = resolve(__dirname, '../../../../rust/cube/cubeserver/src');
const ROUTERS = ['app.rs', 'rbac/handlers/mod.rs'];

/** `.route(path, method_router)` calls: each method and path, keyed like `endpointKey`. */
export function serverRoutes(source: string): string[] {
  const keys: string[] = [];
  let at = source.indexOf('.route(');
  while (at >= 0) {
    // The call's arguments, up to its closing parenthesis.
    let depth = 0;
    let end = at + '.route'.length;
    for (; end < source.length; end += 1) {
      if (source[end] === '(') depth += 1;
      if (source[end] === ')') {
        depth -= 1;
        if (depth === 0) break;
      }
    }
    const call = source.slice(at, end);
    const path = call.match(/"([^"]+)"/)?.[1];
    if (path) {
      const relative = path.startsWith('{base}');
      const clean = path.replace('{base}', '').replace(/\{\{(\w+)\}\}/g, ':$1');
      const methods = new Set([...call.matchAll(/\b(get|post|put|patch|delete)\(/g)].map((m) => m[1].toUpperCase()));
      methods.forEach((m) => keys.push(`${m} ${relative ? '~' : ''}${clean}`));
    }
    at = source.indexOf('.route(', end);
  }
  return keys;
}

describe('the API page catalog', () => {
  const server = [...new Set(ROUTERS.flatMap((f) => serverRoutes(readFileSync(resolve(SERVER, f), 'utf8'))))].sort();
  const catalog = SECTIONS.flatMap((s) => s.endpoints.map(endpointKey)).sort();

  it('reads the server routes', () => {
    expect(server).toContain('POST ~/v1/load');
    expect(server).toContain('GET /readyz');
    expect(server.length).toBeGreaterThan(100);
  });

  it('documents every route of the server', () => {
    expect(server.filter((k) => !catalog.includes(k))).toEqual([]);
  });

  it('documents no route the server lacks, and none twice', () => {
    expect(catalog.filter((k) => !server.includes(k))).toEqual([]);
    expect(catalog.filter((k, i) => catalog.indexOf(k) !== i)).toEqual([]);
  });

  it('gives every section a description and every endpoint a summary and who', () => {
    SECTIONS.forEach((s) => {
      expect(s.description, s.key).not.toBe('');
      s.endpoints.forEach((e) => {
        expect(e.summary, endpointKey(e)).not.toBe('');
        expect(e.who, endpointKey(e)).not.toBe('');
      });
    });
  });
});

describe('catalog helpers', () => {
  it('fills path parameters, leaving the empty ones', () => {
    expect(pathParams('/v1/groups/:id/rules/:rule')).toEqual(['id', 'rule']);
    expect(fillPath('/v1/groups/:id/rules/:rule', { id: 'g 1' })).toBe('/v1/groups/g%201/rules/:rule');
  });

  it('filters endpoints by any of their texts, dropping empty sections', () => {
    const found = filterSections(SECTIONS, 'GROUPS/:id/members');
    expect(found.map((s) => s.key)).toEqual(['groups']);
    expect(found[0].endpoints.map(endpointKey)).toEqual([
      'GET ~/v1/groups/:id/members',
      'PUT ~/v1/groups/:id/members/:user',
      'DELETE ~/v1/groups/:id/members/:user',
    ]);
    expect(filterSections(SECTIONS, '  ')).toBe(SECTIONS);
    expect(filterSections(SECTIONS, 'readyz')[0].key).toBe('system');
  });
});
