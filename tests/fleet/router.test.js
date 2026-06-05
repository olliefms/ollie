import { describe, it, expect } from 'vitest';
import { matchRoute, ROUTES } from '../../static/fleet/router.js';

describe('matchRoute', () => {
  it('matches a bare list route', () => {
    expect(matchRoute('/fleet/home')).toEqual({ name: 'home', params: {} });
    expect(matchRoute('/fleet/loads')).toEqual({ name: 'loads', params: {} });
  });
  it('matches a detail route and captures the id', () => {
    expect(matchRoute('/fleet/loads/abc-123')).toEqual({ name: 'load-detail', params: { id: 'abc-123' } });
    expect(matchRoute('/fleet/documents/doc-9')).toEqual({ name: 'document-detail', params: { id: 'doc-9' } });
  });
  it('treats bare /fleet and /fleet/ as home', () => {
    expect(matchRoute('/fleet')).toEqual({ name: 'home', params: {} });
    expect(matchRoute('/fleet/')).toEqual({ name: 'home', params: {} });
  });
  it('maps placeholder entity routes', () => {
    expect(matchRoute('/fleet/trucks')).toEqual({ name: 'trucks', params: {} });
    expect(matchRoute('/fleet/facilities')).toEqual({ name: 'facilities', params: {} });
  });
  it('returns notfound for an unknown path', () => {
    expect(matchRoute('/fleet/nope/x/y')).toEqual({ name: 'notfound', params: {} });
  });
  it('ignores a trailing query string', () => {
    expect(matchRoute('/fleet/loads?status=planned')).toEqual({ name: 'loads', params: { query: 'status=planned' } });
  });
});
