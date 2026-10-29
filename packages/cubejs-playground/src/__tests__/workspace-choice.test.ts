import { describe, expect, it } from 'vitest';

import { nextChoice } from '../components/Header/workspaceChoice';

const w1 = [{ value: 'w1' }];

describe('nextChoice', () => {
  it('keeps a current workspace that is still visible', () => {
    expect(nextChoice({ current: 'w9', currentVisible: true, firstPage: w1, isAdmin: false })).toBeNull();
  });

  it('picks the first workspace when the current one is gone (non-admin)', () => {
    expect(nextChoice({ current: 'w9', currentVisible: false, firstPage: w1, isAdmin: false }))
      .toEqual({ choose: 'w1' });
  });

  it('falls back to Todos when the current one is gone (admin)', () => {
    expect(nextChoice({ current: 'w9', currentVisible: false, firstPage: w1, isAdmin: true }))
      .toEqual({ choose: null });
  });

  it('does nothing without a current workspace and an empty first page (non-admin)', () => {
    expect(nextChoice({ current: null, currentVisible: false, firstPage: [], isAdmin: false })).toBeNull();
  });

  it('picks the first workspace when nothing is chosen yet (non-admin)', () => {
    expect(nextChoice({ current: null, currentVisible: false, firstPage: w1, isAdmin: false }))
      .toEqual({ choose: 'w1' });
  });
});
