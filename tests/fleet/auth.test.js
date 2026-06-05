import { describe, it, expect, beforeEach } from 'vitest';
import {
  getToken, saveToken, clearToken,
  decodeJwtPayload, isTokenExpired, isAuthenticated, TOKEN_KEY,
} from '../../static/fleet/utils/auth.js';

// base64url-encode a JS object as a fake JWT payload
function makeJwt(payloadObj) {
  const b64 = btoa(JSON.stringify(payloadObj)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  return `header.${b64}.sig`;
}

beforeEach(() => {
  localStorage.clear();
});

describe('token storage', () => {
  it('save/get/clear round-trip', () => {
    saveToken('abc');
    expect(getToken()).toBe('abc');
    clearToken();
    expect(getToken()).toBe(null);
  });
});

describe('decodeJwtPayload', () => {
  it('decodes the payload segment', () => {
    const tok = makeJwt({ sub: 'u1', exp: 9999999999 });
    expect(decodeJwtPayload(tok)).toMatchObject({ sub: 'u1' });
  });
  it('returns null for malformed token', () => {
    expect(decodeJwtPayload('not-a-jwt')).toBe(null);
  });
});

describe('isTokenExpired', () => {
  it('false for a future exp', () => {
    expect(isTokenExpired(makeJwt({ exp: Math.floor(Date.now() / 1000) + 3600 }))).toBe(false);
  });
  it('true for a past exp', () => {
    expect(isTokenExpired(makeJwt({ exp: Math.floor(Date.now() / 1000) - 10 }))).toBe(true);
  });
  it('true when exp missing', () => {
    expect(isTokenExpired(makeJwt({ sub: 'u1' }))).toBe(true);
  });
});

describe('isAuthenticated', () => {
  it('false with no token', () => {
    expect(isAuthenticated()).toBe(false);
  });
  it('true with an unexpired token', () => {
    saveToken(makeJwt({ exp: Math.floor(Date.now() / 1000) + 3600 }));
    expect(isAuthenticated()).toBe(true);
  });
  it('clears and returns false for an expired token', () => {
    saveToken(makeJwt({ exp: Math.floor(Date.now() / 1000) - 10 }));
    expect(isAuthenticated()).toBe(false);
    expect(getToken()).toBe(null); // expired token is cleared
  });
});
