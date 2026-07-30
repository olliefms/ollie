/** Non-blocking confirm / prompt dialogs.
 *
 *  These replace native `confirm()` / `prompt()`, which block the renderer's main
 *  thread for as long as they are open: while one is up the tab cannot repaint,
 *  run script, or be closed, so a stuck action looks like a hung page rather than
 *  a pending request. Every dialog here resolves a Promise and leaves the event
 *  loop free.
 *
 *  All copy is inserted via textContent, so caller-supplied names never parse as
 *  HTML.
 */

const FOCUSABLE = 'button, input, [href], select, textarea, [tabindex]:not([tabindex="-1"])';

/** Build the overlay + panel shell and resolve via `settle`. */
function openDialog({ title, body, actions, onOpen }) {
  return new Promise(resolve => {
    const previouslyFocused = document.activeElement;
    const overlay = document.createElement('div');
    overlay.className = 'dialog-overlay';
    overlay.setAttribute('data-testid', 'dialog-overlay');

    const panel = document.createElement('div');
    panel.className = 'dialog';
    panel.setAttribute('role', 'dialog');
    panel.setAttribute('aria-modal', 'true');

    const heading = document.createElement('h2');
    heading.className = 'dialog__title';
    heading.textContent = title;
    panel.appendChild(heading);
    panel.appendChild(body);

    const footer = document.createElement('div');
    footer.className = 'dialog__actions';
    panel.appendChild(footer);

    overlay.appendChild(panel);

    let done = false;
    const settle = value => {
      if (done) return;
      done = true;
      document.removeEventListener('keydown', onKeydown, true);
      overlay.remove();
      if (previouslyFocused && typeof previouslyFocused.focus === 'function') {
        previouslyFocused.focus();
      }
      resolve(value);
    };

    function onKeydown(e) {
      if (e.key === 'Escape') {
        e.preventDefault();
        settle(null);
        return;
      }
      if (e.key !== 'Tab') return;
      // Keep focus inside the dialog so the page behind it stays untouchable.
      const items = [...panel.querySelectorAll(FOCUSABLE)].filter(el => !el.disabled);
      if (!items.length) return;
      const first = items[0];
      const last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }

    for (const { label, variant, value, primary } of actions) {
      const btn = document.createElement('button');
      btn.className = `btn btn--${variant}`;
      btn.textContent = label;
      if (primary) btn.setAttribute('data-dialog-primary', '');
      btn.addEventListener('click', () => {
        const resolved = typeof value === 'function' ? value() : value;
        // A `value` fn returns undefined to reject its own input and stay open.
        if (resolved !== undefined) settle(resolved);
      });
      footer.appendChild(btn);
    }

    // Clicking the backdrop dismisses, same as Escape.
    overlay.addEventListener('mousedown', e => { if (e.target === overlay) settle(null); });

    document.addEventListener('keydown', onKeydown, true);
    document.body.appendChild(overlay);
    if (onOpen) onOpen(panel, settle);
    else panel.querySelector('[data-dialog-primary]')?.focus();
  });
}

/** Run a caller's `validate` and normalize its answer to an error string or null.
 *
 *  Guarded because `read()` runs inside a DOM event handler: an exception thrown
 *  by `validate` would escape, leave the dialog's promise unresolved, and kill both
 *  OK and Enter — a permanently frozen dialog with nothing on screen explaining it.
 *  That is the failure this component exists to prevent, so a broken validator
 *  becomes a visible message rather than a dead dialog. A non-string truthy return
 *  is treated as "invalid" rather than rendered as "[object Object]". */
function runValidate(validate, values) {
  let problem;
  try {
    problem = validate(values);
  } catch (err) {
    // `err` need not be an Error — `throw null` would make `err.message` throw
    // again from inside this catch, escaping the guard entirely.
    return `Could not validate: ${err?.message ?? String(err)}`;
  }
  if (!problem) return null;
  return typeof problem === 'string' ? problem : 'That value is not valid.';
}

function messageNode(message) {
  const p = document.createElement('p');
  p.className = 'dialog__message';
  p.textContent = message;
  return p;
}

/** Yes/no confirmation. Resolves true only if the user confirms; Escape,
 *  backdrop click and Cancel all resolve false. */
export async function confirmAction({ title, message, confirmLabel = 'Confirm', danger = false }) {
  const choice = await openDialog({
    title,
    body: messageNode(message),
    actions: [
      { label: 'Cancel', variant: 'secondary', value: false },
      { label: confirmLabel, variant: danger ? 'danger' : 'primary', value: true, primary: true },
    ],
  });
  return choice === true;
}

/** Standard destructive-action confirm. Soft delete is reversible, so the copy
 *  says so. Resolves to the user's choice as a boolean. */
export function confirmDelete(what) {
  return confirmAction({
    title: 'Delete',
    message: `Delete ${what}? This can be undone by reactivating.`,
    confirmLabel: 'Delete',
    danger: true,
  });
}

/** Collect one or more text fields in a single dialog. `fields` are
 *  `{ name, label, value, placeholder, required }`. Resolves an object keyed by
 *  field name (values trimmed), or `null` if the user dismissed — matching
 *  `prompt()`'s "null means cancelled" contract so callers keep that check.
 *  One dialog for the whole form beats a chain of one-field prompts. */
export function promptFields({
  title, message = '', fields, confirmLabel = 'OK', danger = false, validate,
}) {
  const body = document.createElement('div');
  if (message) body.appendChild(messageNode(message));

  const inputs = fields.map(f => {
    const wrap = document.createElement('label');
    wrap.className = 'form-label';
    wrap.textContent = f.label || f.name;

    const input = document.createElement('input');
    input.type = f.type || 'text';
    input.className = 'form-input';
    input.value = f.value || '';
    input.placeholder = f.placeholder || '';
    input.setAttribute('data-field', f.name);
    wrap.appendChild(input);
    body.appendChild(wrap);
    return { spec: f, input };
  });

  const error = document.createElement('p');
  error.className = 'dialog__error';
  error.hidden = true;
  body.appendChild(error);

  // Returns the collected values, or undefined to reject and stay open.
  const read = () => {
    const out = {};
    for (const { spec, input } of inputs) {
      const text = input.value.trim();
      if (spec.required && !text) {
        error.textContent = `${spec.label || spec.name} is required.`;
        error.hidden = false;
        input.focus();
        return undefined;
      }
      out[spec.name] = text;
    }
    const problem = validate ? runValidate(validate, out) : null;
    if (problem) {
      error.textContent = problem;
      error.hidden = false;
      inputs[0]?.input.focus();
      return undefined;
    }
    return out;
  };

  return openDialog({
    title,
    body,
    actions: [
      { label: 'Cancel', variant: 'secondary', value: null },
      { label: confirmLabel, variant: danger ? 'danger' : 'primary', value: read, primary: true },
    ],
    onOpen: (panel, settle) => {
      for (const { input } of inputs) {
        input.addEventListener('keydown', e => {
          if (e.key !== 'Enter') return;
          e.preventDefault();
          const values = read();
          if (values !== undefined) settle(values);
        });
      }
      inputs[0]?.input.focus();
      inputs[0]?.input.select();
    },
  });
}

/** Single free-text input. Resolves the entered string (trimmed), `''` when left
 *  blank and optional, or `null` if dismissed. */
export async function promptText({
  title,
  message = '',
  label = '',
  value = '',
  placeholder = '',
  confirmLabel = 'OK',
  required = false,
  danger = false,
}) {
  const result = await promptFields({
    title,
    message,
    confirmLabel,
    danger,
    fields: [{ name: 'value', label, value, placeholder, required }],
  });
  return result === null ? null : result.value;
}

/** Type-the-name-to-confirm gate for irreversible deletes. Resolves true only when
 *  the typed text matches `expected` exactly.
 *
 *  A mismatch is reported inside the dialog and leaves it open, so the caller only
 *  ever sees `true` (matched) or `false` (dismissed) and needs no mismatch branch.
 *  Deliberately not "resolve false on mismatch": that made a typo and a deliberate
 *  cancel indistinguishable, so a user who mistyped got a silent no-op. */
export async function confirmTyped({ title, message, expected, confirmLabel = 'Delete' }) {
  const result = await promptFields({
    title,
    message,
    confirmLabel,
    danger: true,
    fields: [{ name: 'typed', label: `Type "${expected}" to confirm`, required: true }],
    validate: ({ typed }) =>
      (typed === expected ? null : `That doesn't match "${expected}" — check for typos.`),
  });
  return result !== null;
}

/** Pick one of `options` ({ value, label }) by name. Resolves the chosen value,
 *  or null if dismissed. Replaces the numbered-menu `prompt()` pattern. */
export function chooseOption({ title, message = '', label = 'Select', options, confirmLabel = 'Select' }) {
  const body = document.createElement('div');
  if (message) body.appendChild(messageNode(message));

  const field = document.createElement('label');
  field.className = 'form-label';
  field.textContent = label;

  const select = document.createElement('select');
  select.className = 'form-select';
  for (const opt of options) {
    const el = document.createElement('option');
    el.value = String(opt.value);
    el.textContent = opt.label;
    select.appendChild(el);
  }
  field.appendChild(select);
  body.appendChild(field);

  return openDialog({
    title,
    body,
    actions: [
      { label: 'Cancel', variant: 'secondary', value: null },
      { label: confirmLabel, variant: 'primary', value: () => select.value, primary: true },
    ],
    onOpen: () => select.focus(),
  });
}
