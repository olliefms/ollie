import { describe, it, expect, vi, afterEach } from 'vitest';
import { confirmDelete } from '../../static/fleet/components/confirm.js';

afterEach(() => vi.restoreAllMocks());

describe('confirmDelete', () => {
  it('returns true when the user confirms', () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(true));
    expect(confirmDelete('a driver')).toBe(true);
    expect(globalThis.confirm).toHaveBeenCalledWith('Delete a driver? This can be undone by reactivating.');
  });
  it('returns false when the user cancels', () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(false));
    expect(confirmDelete('a driver')).toBe(false);
  });
});
