import { describe, it, expect, vi } from 'vitest';
import { buildPayload, renderForm } from '../../static/fleet/components/form.js';

const FIELDS = [
  { key: 'name', label: 'Name', type: 'text', required: true },
  { key: 'year', label: 'Year', type: 'int' },
  { key: 'rate', label: 'Rate', type: 'number' },
  { key: 'is_default', label: 'Default', type: 'checkbox' },
  { key: 'status', label: 'Status', type: 'select', options: ['active', 'inactive'] },
  { key: 'loaded_rate_per_mile', label: 'Loaded $/mi', type: 'inheritable', inheritedValue: 0.55, inheritedFrom: 'Terminal: Dallas' },
];

describe('buildPayload coercion + omission', () => {
  it('coerces by type and omits blanks', () => {
    const { payload, errors } = buildPayload(FIELDS, {
      name: 'Unit 1', year: '2022', rate: '', is_default: true, status: 'active',
      loaded_rate_per_mile: '',
    }, new Set());
    expect(errors).toEqual([]);
    expect(payload).toEqual({ name: 'Unit 1', year: 2022, is_default: true, status: 'active' });
    expect('rate' in payload).toBe(false);
    expect('loaded_rate_per_mile' in payload).toBe(false);
  });

  it('flags a required blank field', () => {
    const { payload, errors } = buildPayload(FIELDS, { name: '' }, new Set());
    expect(errors).toContain('Name is required.');
    expect('name' in payload).toBe(false);
  });
});

describe('buildPayload inherited-value rule', () => {
  it('inherited + typed value → sent as override', () => {
    const { payload } = buildPayload(FIELDS, { name: 'x', loaded_rate_per_mile: '0.80' }, new Set());
    expect(payload.loaded_rate_per_mile).toBe(0.80);
  });
  it('inherited + blank → omitted (never bakes in inherited number)', () => {
    const { payload } = buildPayload(FIELDS, { name: 'x', loaded_rate_per_mile: '' }, new Set());
    expect('loaded_rate_per_mile' in payload).toBe(false);
  });
  it('revert clicked → explicit null', () => {
    const { payload } = buildPayload(FIELDS, { name: 'x', loaded_rate_per_mile: '0.80' }, new Set(['loaded_rate_per_mile']));
    expect(payload.loaded_rate_per_mile).toBe(null);
  });
  it('inherited + value 0 → sent as 0 override (0 is meaningful, not blank)', () => {
    const { payload } = buildPayload(FIELDS, { name: 'x', loaded_rate_per_mile: '0' }, new Set());
    expect(payload.loaded_rate_per_mile).toBe(0);
  });
  it('inherited + non-numeric garbage → omitted (NaN never sent as override)', () => {
    const { payload } = buildPayload(FIELDS, { name: 'x', loaded_rate_per_mile: 'abc' }, new Set());
    expect('loaded_rate_per_mile' in payload).toBe(false);
  });
});

describe('renderForm', () => {
  it('renders inputs and submits the built payload', async () => {
    const container = document.createElement('div');
    const onSubmit = vi.fn().mockResolvedValue({ ok: true });
    renderForm(container, {
      title: 'Edit',
      fields: [{ key: 'name', label: 'Name', type: 'text', required: true }],
      values: { name: 'Start' },
      submitLabel: 'Save',
      onSubmit,
    });
    const input = container.querySelector('[data-field="name"]');
    expect(input).not.toBe(null);
    expect(input.value).toBe('Start');
    input.value = 'Changed';
    container.querySelector('[data-form-submit]').click();
    await Promise.resolve(); await Promise.resolve();
    expect(onSubmit).toHaveBeenCalledWith({ name: 'Changed' });
  });

  it('blocks submit and shows an error when a required field is blank', async () => {
    const container = document.createElement('div');
    const onSubmit = vi.fn();
    renderForm(container, {
      title: 'New',
      fields: [{ key: 'name', label: 'Name', type: 'text', required: true }],
      values: {},
      submitLabel: 'Save',
      onSubmit,
    });
    container.querySelector('[data-form-submit]').click();
    await Promise.resolve();
    expect(onSubmit).not.toHaveBeenCalled();
    expect(container.querySelector('[data-form-error]').hidden).toBe(false);
  });
});
