import { describe, it, expect } from 'vitest';
import { scopeGranted, gate } from '../../static/fleet/components/scope-gate.js';

describe('scopeGranted', () => {
  it('grants on exact match', () => {
    expect(scopeGranted(['loads:write'], 'loads:write')).toBe(true);
  });
  it('grants on per-resource wildcard', () => {
    expect(scopeGranted(['loads:*'], 'loads:write')).toBe(true);
  });
  it('grants on global superuser', () => {
    expect(scopeGranted(['*'], 'anything:delete')).toBe(true);
  });
  it('denies when absent', () => {
    expect(scopeGranted(['loads:read'], 'loads:write')).toBe(false);
  });
  it('denies cross-resource wildcard', () => {
    expect(scopeGranted(['trucks:*'], 'loads:write')).toBe(false);
  });
  it('denies on empty/missing scopes', () => {
    expect(scopeGranted([], 'loads:write')).toBe(false);
    expect(scopeGranted(null, 'loads:write')).toBe(false);
  });
});

describe('gate', () => {
  it('hides the element when not granted', () => {
    const el = document.createElement('button');
    gate(el, false);
    expect(el.hidden).toBe(true);
  });
  it('shows the element when granted', () => {
    const el = document.createElement('button');
    el.hidden = true;
    gate(el, true);
    expect(el.hidden).toBe(false);
  });
  it('no-ops on null element', () => {
    expect(() => gate(null, true)).not.toThrow();
  });
});
