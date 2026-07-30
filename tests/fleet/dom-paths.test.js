import { describe, it, expect } from 'vitest';
import { VIEW_PATHS, withPending } from '../../static/fleet/utils/dom.js';

describe('VIEW_PATHS filter/pagination serialization', () => {
  it('serializes the documents name + offset params', () => {
    expect(VIEW_PATHS.documents({})).toBe('/fleet/documents');
    expect(VIEW_PATHS.documents({ name: 'rate con', offset: 20 }))
      .toBe('/fleet/documents?name=rate+con&offset=20');
  });

  it('serializes the loads status filter', () => {
    expect(VIEW_PATHS.loads({})).toBe('/fleet/loads');
    expect(VIEW_PATHS.loads({ status: 'planned' })).toBe('/fleet/loads?status=planned');
  });

  it('serializes the trips status filter', () => {
    expect(VIEW_PATHS.trips({})).toBe('/fleet/trips');
    expect(VIEW_PATHS.trips({ status: 'dispatched' })).toBe('/fleet/trips?status=dispatched');
  });

  it('serializes the expenses filters', () => {
    expect(VIEW_PATHS.expenses({
      status: 'submitted', category: 'fuel', driver_id: 'd1',
      from: '2026-07-01', to: '2026-07-31',
    })).toBe('/fleet/expenses?status=submitted&category=fuel&driver_id=d1&from=2026-07-01&to=2026-07-31');
  });

  it('drops empty/undefined values and tolerates a missing params object', () => {
    expect(VIEW_PATHS.loads({ status: '' })).toBe('/fleet/loads');
    expect(VIEW_PATHS.expenses({ status: undefined, category: 'fuel' }))
      .toBe('/fleet/expenses?category=fuel');
    expect(VIEW_PATHS.loads()).toBe('/fleet/loads');
    expect(VIEW_PATHS.documents()).toBe('/fleet/documents');
  });
});

// Pending state is the other half of "a stuck request must never look like a
// frozen page": the button reports that it is working, and cannot be re-fired.
describe('withPending', () => {
  function actionRow() {
    document.body.innerHTML =
      '<div><button id="a">Cancel Load</button><button id="b">Delete</button></div>';
    return [document.getElementById('a'), document.getElementById('b')];
  }

  it('disables the row and shows a spinner while the action runs', async () => {
    const [btn, sibling] = actionRow();
    let seen;
    await withPending(btn, async () => {
      seen = {
        disabled: btn.disabled,
        siblingDisabled: sibling.disabled,
        spinner: !!btn.querySelector('.spinner--inline'),
        busy: btn.getAttribute('aria-busy'),
      };
    });
    expect(seen).toEqual({
      disabled: true, siblingDisabled: true, spinner: true, busy: 'true',
    });
  });

  it('restores the original label and enabled state on success', async () => {
    const [btn, sibling] = actionRow();
    await withPending(btn, async () => {});
    expect(btn.textContent).toBe('Cancel Load');
    expect(btn.disabled).toBe(false);
    expect(sibling.disabled).toBe(false);
    expect(btn.hasAttribute('aria-busy')).toBe(false);
  });

  it('restores state when the action throws, so a failure is retryable', async () => {
    const [btn] = actionRow();
    await expect(withPending(btn, async () => { throw new Error('timed out'); }))
      .rejects.toThrow('timed out');
    expect(btn.textContent).toBe('Cancel Load');
    expect(btn.disabled).toBe(false);
  });

  it('leaves an already-disabled sibling disabled', async () => {
    const [btn, sibling] = actionRow();
    sibling.disabled = true;
    await withPending(btn, async () => {});
    expect(sibling.disabled).toBe(true);
  });

  it('still runs the action when there is no button', async () => {
    await expect(withPending(null, async () => 'ran')).resolves.toBe('ran');
  });
});
