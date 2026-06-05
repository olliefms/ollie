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

  it('matches terminal new/edit/detail in the right precedence', () => {
    expect(matchRoute('/fleet/terminals')).toEqual({ name: 'terminals', params: {} });
    expect(matchRoute('/fleet/terminals/new')).toEqual({ name: 'terminal-new', params: {} });
    expect(matchRoute('/fleet/terminals/t-1/edit')).toEqual({ name: 'terminal-edit', params: { id: 't-1' } });
    expect(matchRoute('/fleet/terminals/t-1')).toEqual({ name: 'terminal-detail', params: { id: 't-1' } });
  });

  it('matches truck new/edit/detail in the right precedence', () => {
    expect(matchRoute('/fleet/trucks/new')).toEqual({ name: 'truck-new', params: {} });
    expect(matchRoute('/fleet/trucks/abc/edit')).toEqual({ name: 'truck-edit', params: { id: 'abc' } });
    expect(matchRoute('/fleet/trucks/abc')).toEqual({ name: 'truck-detail', params: { id: 'abc' } });
  });

  it('matches trailer new/edit/detail in the right precedence', () => {
    expect(matchRoute('/fleet/trailers/new')).toEqual({ name: 'trailer-new', params: {} });
    expect(matchRoute('/fleet/trailers/xy-9/edit')).toEqual({ name: 'trailer-edit', params: { id: 'xy-9' } });
    expect(matchRoute('/fleet/trailers/xy-9')).toEqual({ name: 'trailer-detail', params: { id: 'xy-9' } });
  });

  it('matches driver new/edit/detail in the right precedence', () => {
    expect(matchRoute('/fleet/drivers/new')).toEqual({ name: 'driver-new', params: {} });
    expect(matchRoute('/fleet/drivers/d-1/edit')).toEqual({ name: 'driver-edit', params: { id: 'd-1' } });
    expect(matchRoute('/fleet/drivers/d-1')).toEqual({ name: 'driver-detail', params: { id: 'd-1' } });
  });
});
