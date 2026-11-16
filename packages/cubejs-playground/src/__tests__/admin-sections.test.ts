import { describe, expect, it } from 'vitest';

import { adminPath, adminSectionOf } from '../pages/Admin/sections';

describe('admin sections', () => {
  it('reads the section of a path, the first one by default', () => {
    expect(adminSectionOf('/admin/users')).toBe('users');
    expect(adminSectionOf('/admin/workspaces')).toBe('workspaces');
    expect(adminSectionOf('/admin')).toBe('organizations');
    expect(adminSectionOf('/admin/nope')).toBe('organizations');
    expect(adminPath('users')).toBe('/admin/users');
  });
});
