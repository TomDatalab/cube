import { describe, expect, it } from 'vitest';

import {
  EditorState, reduce, problemLines, readOnlyReason, fileHint, offersJavaScript,
} from '../pages/Schema/editor';

const view: EditorState = {
  mode: 'view', draft: '', base: 3, error: null, problems: [], stale: false,
};

describe('model file editor state', () => {
  it('edit then cancel restores', () => {
    const editing = reduce(reduce(view, { type: 'edit' }), { type: 'change', text: 'cubes: []' });
    expect(editing.mode).toBe('edit');
    expect(editing.draft).toBe('cubes: []');
    expect(reduce(editing, { type: 'cancel' })).toEqual(view);
  });

  it('400 keeps draft and shows problems', () => {
    const problems = [{ file: 'a.yml', cube: 'x', message: 'bad' }];
    let s = reduce(view, { type: 'edit' });
    s = reduce(reduce(s, { type: 'change', text: 'abc' }), { type: 'save' });
    expect(s.mode).toBe('saving');
    s = reduce(s, { type: 'failed', status: 400, body: { error: 'invalid', problems } });
    expect(s).toMatchObject({ mode: 'edit', draft: 'abc', error: 'invalid', problems, stale: false });
    expect(problemLines(s.problems)).toEqual(['a.yml: bad']);
    expect(problemLines([{ file: '', cube: '', message: 'm' }])).toEqual(['m']);
  });

  it('409 marks stale and keeps draft', () => {
    let s = reduce(reduce(view, { type: 'edit' }), { type: 'change', text: 'abc' });
    s = reduce(reduce(s, { type: 'save' }), { type: 'failed', status: 409, body: { error: 'x' } });
    expect(s).toMatchObject({ mode: 'edit', draft: 'abc', stale: true });
  });

  it('saved returns to view with new base', () => {
    let s = reduce(reduce(view, { type: 'edit' }), { type: 'change', text: 'abc' });
    s = reduce(reduce(s, { type: 'save' }), { type: 'saved', version: 4, content: 'abc' });
    expect(s).toMatchObject({ mode: 'view', base: 4, error: null, problems: [], stale: false });
  });

  it('explains why a file is read-only', () => {
    expect(readOnlyReason({ managedBy: 'sofiadb-sync', path: 'a.yml' })).toMatch(/^Este arquivo é gerado a partir do catálogo do SofiaDB/);
    expect(readOnlyReason({ path: 'a/b.JINJA' })).toBe('Arquivos .jinja e .j2 não são editáveis aqui');
    expect(readOnlyReason({ path: 'a.yml' })).toBeNull();
  });
});

describe('file panel and generate menu by RBAC', () => {
  it('without RBAC the file panel says where the file is', () => {
    expect(fileHint(false, false)).toBe('path');
    expect(fileHint(false, true)).toBe('docker');
  });

  it('with RBAC the file panel shows no disk path', () => {
    expect(fileHint(true, false)).toBe(null);
    expect(fileHint(true, true)).toBe('docker');
  });

  it('JavaScript is offered only without RBAC', () => {
    expect(offersJavaScript(false)).toBe(true);
    expect(offersJavaScript(true)).toBe(false);
  });
});
