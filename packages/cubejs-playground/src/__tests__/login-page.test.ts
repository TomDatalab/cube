import { describe, expect, it } from 'vitest';

import { missingFields } from '../pages/Login/LoginPage';

describe('login form', () => {
  it('asks for both fields while they are empty', () => {
    expect(missingFields('', '')).toEqual({
      username: 'Enter your username',
      password: 'Enter your password',
    });
    expect(missingFields('   ', 'x')).toEqual({ username: 'Enter your username' });
  });

  it('lets a filled form through, a password of spaces included', () => {
    expect(missingFields('ann', ' ')).toEqual({});
  });
});
