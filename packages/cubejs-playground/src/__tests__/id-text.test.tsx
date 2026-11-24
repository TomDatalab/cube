import { describe, expect, it } from 'vitest';

import { shortId } from '../pages/Admin/IdText';

describe('shortId', () => {
  it('shows a UUID as its first 8 characters', () => {
    expect(shortId('3f2b8c1e-9a4d-4e2f-8b7a-1c2d3e4f5a6b')).toBe('3f2b8c1e…');
  });

  it('shows any other id whole', () => {
    expect(shortId('viewer')).toBe('viewer');
    expect(shortId('acme')).toBe('acme');
  });
});
