import { isAuthenticated } from '../utils/auth.js';
import { apiFetch } from '../utils/api.js';
import { renderAppBar } from '../components/app-bar.js';
import { renderBottomNav } from '../components/bottom-nav.js';

export async function renderEquipment(container) {
  if (!isAuthenticated()) {
    window.location.replace('/driver');
    return;
  }

  container.replaceChildren();
  const page = document.createElement('div');
  page.className = 'page-with-nav';
  page.appendChild(renderAppBar({ title: 'Equipment' }));

  const main = document.createElement('main');
  main.className = 'equipment-page';

  const loading = document.createElement('div');
  loading.className = 'empty-state';
  loading.textContent = 'Loading…';
  main.appendChild(loading);

  page.appendChild(main);
  page.appendChild(renderBottomNav('equipment'));
  container.appendChild(page);

  let equipment, trailers;
  try {
    [equipment, trailers] = await Promise.all([
      apiFetch('/equipment'),
      apiFetch('/trailers'),
    ]);
  } catch (e) {
    loading.remove();
    const err = document.createElement('div');
    err.className = 'empty-state';
    err.textContent = `Failed to load equipment: ${e.message}`;
    main.appendChild(err);
    return;
  }

  main.replaceChildren();
  main.appendChild(buildTruckCard(equipment.truck));
  main.appendChild(buildTrailerSection(equipment.trailers, trailers.items, async (body) => {
    return await submitTrailerChange(body);
  }));
}

function buildTruckCard(truck) {
  const card = document.createElement('section');
  card.className = 'card';

  const h = document.createElement('h2');
  h.textContent = 'Truck';
  card.appendChild(h);

  if (!truck) {
    const p = document.createElement('p');
    p.className = 'muted';
    p.textContent = 'No truck assigned. Contact dispatch to update.';
    card.appendChild(p);
    return card;
  }

  const unit = document.createElement('div');
  unit.className = 'equipment-unit';
  unit.textContent = truck.unit_number;
  card.appendChild(unit);

  if (truck.plate) {
    const plate = document.createElement('div');
    plate.className = 'muted';
    plate.textContent = `Plate: ${truck.plate}`;
    card.appendChild(plate);
  }

  const note = document.createElement('div');
  note.className = 'muted small';
  note.textContent = 'Truck assignment is managed by dispatch.';
  card.appendChild(note);

  return card;
}

function buildTrailerSection(current, allTrailers, onSubmit) {
  const card = document.createElement('section');
  card.className = 'card';

  const h = document.createElement('h2');
  h.textContent = 'Trailer(s)';
  card.appendChild(h);

  const currentList = document.createElement('div');
  currentList.className = 'equipment-current';
  const renderCurrentList = (trailers) => {
    currentList.replaceChildren();
    if (trailers.length === 0) {
      const p = document.createElement('p');
      p.className = 'muted';
      p.textContent = 'No trailer currently attached.';
      currentList.appendChild(p);
      return;
    }
    trailers.forEach(t => {
      const row = document.createElement('div');
      row.className = 'equipment-row';
      const unit = document.createElement('span');
      unit.className = 'equipment-unit';
      unit.textContent = t.unit_number;
      const meta = document.createElement('span');
      meta.className = 'muted';
      const parts = [];
      if (t.owner_name) parts.push(t.owner_name);
      if (t.trailer_type) parts.push(t.trailer_type);
      meta.textContent = parts.join(' · ');
      row.appendChild(unit);
      if (parts.length) row.appendChild(meta);
      currentList.appendChild(row);
    });
  };
  renderCurrentList(current);
  card.appendChild(currentList);

  // Picker — type-to-complete with removable chips instead of a long list.
  const picker = document.createElement('div');
  picker.className = 'equipment-picker';

  const filterLabel = document.createElement('label');
  filterLabel.textContent = 'Add trailer';
  filterLabel.className = 'form-label';
  picker.appendChild(filterLabel);

  const selectedSet = new Set(current.map(t => t.id));
  // Unit numbers the driver typed that aren't known trailers yet — created on
  // submit. Tracked separately because they have no id until the server makes one.
  const pendingNewUnits = new Set();
  const unitById = new Map([...current, ...allTrailers].map(t => [t.id, t.unit_number]));

  // Selected trailers as chips.
  const chips = document.createElement('div');
  chips.className = 'trailer-chips';

  const renderChips = () => {
    chips.replaceChildren();
    const known = Array.from(selectedSet).map(id => ({
      id,
      unit: unitById.get(id) || (allTrailers.find(t => t.id === id) || {}).unit_number || '—',
    }));
    if (known.length === 0 && pendingNewUnits.size === 0) {
      const empty = document.createElement('span');
      empty.className = 'muted small';
      empty.textContent = 'No trailers selected.';
      chips.appendChild(empty);
      return;
    }
    known.forEach(({ id, unit }) => chips.appendChild(makeChip(unit, null, () => {
      selectedSet.delete(id);
      renderChips();
    })));
    pendingNewUnits.forEach(unit => chips.appendChild(makeChip(unit, 'new', () => {
      pendingNewUnits.delete(unit);
      renderChips();
    })));
  };

  // Typeahead input + suggestions dropdown.
  const typeahead = document.createElement('div');
  typeahead.className = 'trailer-typeahead';

  const filter = document.createElement('input');
  filter.type = 'text';
  filter.placeholder = 'Type a trailer number';
  filter.className = 'form-input';
  filter.autocomplete = 'off';
  typeahead.appendChild(filter);

  const suggestions = document.createElement('div');
  suggestions.className = 'trailer-suggestions';
  suggestions.hidden = true;
  typeahead.appendChild(suggestions);

  const addKnown = (t) => {
    selectedSet.add(t.id);
    unitById.set(t.id, t.unit_number);
    filter.value = '';
    renderSuggestions();
    renderChips();
  };
  const addNew = (raw) => {
    pendingNewUnits.add(raw);
    filter.value = '';
    renderSuggestions();
    renderChips();
  };

  const renderSuggestions = () => {
    suggestions.replaceChildren();
    const raw = filter.value.trim();
    const q = raw.toLowerCase();
    if (!q) { suggestions.hidden = true; return; }

    const matches = allTrailers.filter(t =>
      !selectedSet.has(t.id) && (
        t.unit_number.toLowerCase().includes(q)
        || (t.owner_name && t.owner_name.toLowerCase().includes(q))
      )
    ).slice(0, 8);

    matches.forEach(t => {
      const row = document.createElement('button');
      row.type = 'button';
      row.className = 'trailer-suggestion';
      const unit = document.createElement('strong');
      unit.textContent = t.unit_number;
      row.appendChild(unit);
      const meta = [];
      if (t.owner_name) meta.push(t.owner_name);
      if (t.trailer_type) meta.push(t.trailer_type);
      if (meta.length) {
        const m = document.createElement('span');
        m.className = 'muted';
        m.textContent = ' · ' + meta.join(' · ');
        row.appendChild(m);
      }
      row.addEventListener('click', () => addKnown(t));
      suggestions.appendChild(row);
    });

    // Offer to hook a brand-new trailer when the typed value isn't an exact match.
    const exact = allTrailers.some(t => t.unit_number.toLowerCase() === q);
    if (!exact && !pendingNewUnits.has(raw)) {
      const add = document.createElement('button');
      add.type = 'button';
      add.className = 'trailer-suggestion trailer-suggestion--new';
      add.textContent = `Hook new trailer “${raw}”`;
      add.addEventListener('click', () => addNew(raw));
      suggestions.appendChild(add);
    }

    suggestions.hidden = suggestions.childElementCount === 0;
  };

  filter.addEventListener('input', renderSuggestions);
  filter.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter') return;
    e.preventDefault();
    const first = suggestions.querySelector('.trailer-suggestion');
    if (first) first.click();
  });

  picker.appendChild(chips);
  picker.appendChild(typeahead);
  renderChips();

  // Keep renderList name for the submit handler below (re-renders selection UI).
  const renderList = () => { renderChips(); renderSuggestions(); };

  const submit = document.createElement('button');
  submit.type = 'button';
  submit.className = 'btn btn-primary';
  submit.textContent = 'Update trailer';
  picker.appendChild(submit);

  const status = document.createElement('div');
  status.className = 'equipment-status';
  picker.appendChild(status);

  submit.addEventListener('click', async () => {
    submit.disabled = true;
    status.textContent = 'Updating…';
    status.className = 'equipment-status muted';
    try {
      const newUnits = Array.from(pendingNewUnits);
      const body = newUnits.length
        ? { trailer_unit_numbers: [
            ...Array.from(selectedSet).map(id => unitById.get(id)).filter(Boolean),
            ...newUnits,
          ] }
        : { trailer_ids: Array.from(selectedSet) };
      const result = await onSubmit(body);
      const updatedTrailers = result.trailers || [];
      // Surface any freshly created trailers in the picker so they stay visible.
      updatedTrailers.forEach(t => {
        if (!allTrailers.some(a => a.id === t.id)) allTrailers.push(t);
        unitById.set(t.id, t.unit_number);
      });
      renderCurrentList(updatedTrailers);
      selectedSet.clear();
      updatedTrailers.forEach(t => selectedSet.add(t.id));
      pendingNewUnits.clear();
      renderList();
      status.className = 'equipment-status success';
      status.textContent = result.trip_cascade
        ? 'Updated. Active trip trailer also updated.'
        : 'Updated.';
    } catch (e) {
      status.className = 'equipment-status error';
      status.textContent = `Update failed: ${e.message}`;
    } finally {
      submit.disabled = false;
    }
  });

  card.appendChild(picker);
  return card;
}

function makeChip(unit, tag, onRemove) {
  const chip = document.createElement('span');
  chip.className = 'trailer-chip';
  const u = document.createElement('strong');
  u.textContent = unit;
  chip.appendChild(u);
  if (tag) {
    const t = document.createElement('span');
    t.className = 'muted';
    t.textContent = ` · ${tag}`;
    chip.appendChild(t);
  }
  const x = document.createElement('button');
  x.type = 'button';
  x.className = 'trailer-chip__remove';
  x.textContent = '×';
  x.setAttribute('aria-label', `Remove ${unit}`);
  x.addEventListener('click', onRemove);
  chip.appendChild(x);
  return chip;
}

async function submitTrailerChange(body) {
  const result = await apiFetch('/equipment/trailer', {
    method: 'PUT',
    body,
  });
  return result;
}
