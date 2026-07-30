import { describe, it, expect, afterEach } from 'vitest';
import {
  confirmAction, confirmDelete, confirmTyped, promptText, promptFields, chooseOption,
} from '../../static/fleet/components/confirm.js';

afterEach(() => { document.body.replaceChildren(); });

const overlay = () => document.querySelector('[data-testid="dialog-overlay"]');
const buttonLabelled = text =>
  [...document.querySelectorAll('.dialog__actions .btn')].find(b => b.textContent === text);
const inputFor = name => document.querySelector(`.dialog input[data-field="${name}"]`);
const press = key =>
  document.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));

describe('dialogs are non-blocking', () => {
  // The whole point of this component: native confirm()/prompt() freeze the
  // renderer's main thread, so a stuck action looks like a hung tab. These return
  // a pending Promise and leave the event loop free.
  it('returns a pending promise instead of blocking', async () => {
    const pending = confirmAction({ title: 'T', message: 'M' });
    expect(pending).toBeInstanceOf(Promise);
    // The event loop still runs while the dialog is open.
    let ticked = false;
    await Promise.resolve().then(() => { ticked = true; });
    expect(ticked).toBe(true);
    expect(overlay()).not.toBeNull();

    buttonLabelled('Cancel').click();
    await pending;
  });

  it('removes the overlay once settled', async () => {
    const pending = confirmAction({ title: 'T', message: 'M', confirmLabel: 'Go' });
    buttonLabelled('Go').click();
    await expect(pending).resolves.toBe(true);
    expect(overlay()).toBeNull();
  });
});

describe('confirmAction', () => {
  it('resolves true only when confirmed', async () => {
    const yes = confirmAction({ title: 'T', message: 'M', confirmLabel: 'Yes' });
    buttonLabelled('Yes').click();
    await expect(yes).resolves.toBe(true);

    const no = confirmAction({ title: 'T', message: 'M', confirmLabel: 'Yes' });
    buttonLabelled('Cancel').click();
    await expect(no).resolves.toBe(false);
  });

  it('treats Escape as a decline', async () => {
    const pending = confirmAction({ title: 'T', message: 'M' });
    press('Escape');
    await expect(pending).resolves.toBe(false);
  });

  it('marks the destructive variant', async () => {
    const pending = confirmAction({ title: 'T', message: 'M', confirmLabel: 'Del', danger: true });
    expect(buttonLabelled('Del').className).toContain('btn--danger');
    buttonLabelled('Cancel').click();
    await pending;
  });

  it('renders caller text as text, never as markup', async () => {
    const pending = confirmAction({ title: 'T', message: '<img src=x onerror=1>' });
    expect(document.querySelector('.dialog__message').querySelector('img')).toBeNull();
    expect(document.querySelector('.dialog__message').textContent).toBe('<img src=x onerror=1>');
    buttonLabelled('Cancel').click();
    await pending;
  });
});

describe('confirmDelete', () => {
  it('resolves true when the user confirms', async () => {
    const pending = confirmDelete('a driver');
    expect(document.querySelector('.dialog__message').textContent)
      .toBe('Delete a driver? This can be undone by reactivating.');
    buttonLabelled('Delete').click();
    await expect(pending).resolves.toBe(true);
  });

  it('resolves false when the user cancels', async () => {
    const pending = confirmDelete('a driver');
    buttonLabelled('Cancel').click();
    await expect(pending).resolves.toBe(false);
  });
});

describe('promptText', () => {
  it('resolves the trimmed value', async () => {
    const pending = promptText({ title: 'Reason', label: 'Why' });
    inputFor('value').value = '  customer cancelled  ';
    buttonLabelled('OK').click();
    await expect(pending).resolves.toBe('customer cancelled');
  });

  it('resolves null when dismissed, matching prompt()', async () => {
    const pending = promptText({ title: 'Reason', label: 'Why' });
    press('Escape');
    await expect(pending).resolves.toBeNull();
  });

  it('resolves an empty string when an optional field is left blank', async () => {
    const pending = promptText({ title: 'Reason', label: 'Why' });
    buttonLabelled('OK').click();
    await expect(pending).resolves.toBe('');
  });

  it('keeps a required field open until filled', async () => {
    const pending = promptText({ title: 'PIN', label: 'New PIN', required: true });
    buttonLabelled('OK').click();
    expect(overlay()).not.toBeNull();
    expect(document.querySelector('.dialog__error').hidden).toBe(false);

    inputFor('value').value = '1234';
    buttonLabelled('OK').click();
    await expect(pending).resolves.toBe('1234');
  });
});

describe('promptFields', () => {
  it('collects every field in one dialog', async () => {
    const pending = promptFields({
      title: 'Check call',
      confirmLabel: 'Record',
      fields: [
        { name: 'location', label: 'Location', required: true },
        { name: 'notes', label: 'Notes' },
      ],
    });
    inputFor('location').value = 'Tulsa, OK';
    inputFor('notes').value = 'running 30 late';
    buttonLabelled('Record').click();
    await expect(pending).resolves.toEqual({ location: 'Tulsa, OK', notes: 'running 30 late' });
  });

  it('keeps the dialog open when a custom validate rejects', async () => {
    const pending = promptFields({
      title: 'Pick a number',
      fields: [{ name: 'n', label: 'Number' }],
      validate: ({ n }) => (/^\d+$/.test(n) ? null : 'Digits only.'),
    });
    inputFor('n').value = 'abc';
    buttonLabelled('OK').click();
    expect(overlay()).not.toBeNull();
    expect(document.querySelector('.dialog__error').textContent).toBe('Digits only.');

    inputFor('n').value = '42';
    buttonLabelled('OK').click();
    await expect(pending).resolves.toEqual({ n: '42' });
  });

  it('names the offending field when a required one is blank', async () => {
    const pending = promptFields({
      title: 'Check call',
      fields: [{ name: 'location', label: 'Location', required: true }],
    });
    buttonLabelled('OK').click();
    expect(document.querySelector('.dialog__error').textContent).toContain('Location');
    press('Escape');
    await expect(pending).resolves.toBeNull();
  });
});

describe('confirmTyped', () => {
  it('resolves true on an exact match', async () => {
    const ok = confirmTyped({ title: 'Delete', message: 'M', expected: 'LD-2026-0001' });
    inputFor('typed').value = 'LD-2026-0001';
    buttonLabelled('Delete').click();
    await expect(ok).resolves.toBe(true);
  });

  // A mismatch used to resolve false, which the caller could not tell apart from a
  // deliberate cancel — so a mistyped load number produced a silent no-op. It now
  // reports inside the dialog and stays open.
  it('reports a mismatch in the dialog instead of silently resolving false', async () => {
    const pending = confirmTyped({ title: 'Delete', message: 'M', expected: 'LD-2026-0001' });
    inputFor('typed').value = 'ld-2026-0001';
    buttonLabelled('Delete').click();

    expect(overlay()).not.toBeNull();
    const err = document.querySelector('.dialog__error');
    expect(err.hidden).toBe(false);
    expect(err.textContent).toContain('LD-2026-0001');

    // Correcting the typo then succeeds, without reopening the dialog.
    inputFor('typed').value = 'LD-2026-0001';
    buttonLabelled('Delete').click();
    await expect(pending).resolves.toBe(true);
  });

  it('resolves false when dismissed', async () => {
    const pending = confirmTyped({ title: 'Delete', message: 'M', expected: 'X' });
    press('Escape');
    await expect(pending).resolves.toBe(false);
  });
});

describe('chooseOption', () => {
  it('resolves the selected value', async () => {
    const pending = chooseOption({
      title: 'Select a driver',
      options: [{ value: 'a', label: 'Ann' }, { value: 'b', label: 'Bo' }],
    });
    document.querySelector('.dialog select').value = 'b';
    buttonLabelled('Select').click();
    await expect(pending).resolves.toBe('b');
  });

  it('resolves null when dismissed', async () => {
    const pending = chooseOption({ title: 'Select', options: [{ value: 'a', label: 'Ann' }] });
    buttonLabelled('Cancel').click();
    await expect(pending).resolves.toBeNull();
  });
});
