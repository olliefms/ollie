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

  // Picker
  const picker = document.createElement('div');
  picker.className = 'equipment-picker';

  const filterLabel = document.createElement('label');
  filterLabel.textContent = 'Find trailer';
  filterLabel.className = 'form-label';
  picker.appendChild(filterLabel);

  const filter = document.createElement('input');
  filter.type = 'text';
  filter.placeholder = 'Type unit number or filter list';
  filter.className = 'form-input';
  picker.appendChild(filter);

  const listWrap = document.createElement('div');
  listWrap.className = 'equipment-list';
  picker.appendChild(listWrap);

  const selectedSet = new Set(current.map(t => t.id));
  // Unit numbers the driver typed that aren't known trailers yet — created on
  // submit. Tracked separately because they have no id until the server makes one.
  const pendingNewUnits = new Set();
  const unitById = new Map([...current, ...allTrailers].map(t => [t.id, t.unit_number]));

  const renderList = () => {
    listWrap.replaceChildren();

    // Queued new trailers show as checked rows; unchecking removes them.
    pendingNewUnits.forEach(unit => {
      const row = document.createElement('label');
      row.className = 'equipment-pick-row';
      const cb = document.createElement('input');
      cb.type = 'checkbox';
      cb.checked = true;
      cb.addEventListener('change', () => {
        pendingNewUnits.delete(unit);
        renderList();
      });
      const text = document.createElement('span');
      const u = document.createElement('strong');
      u.textContent = unit;
      const m = document.createElement('span');
      m.className = 'muted';
      m.textContent = ' · new trailer';
      text.appendChild(u);
      text.appendChild(m);
      row.appendChild(cb);
      row.appendChild(text);
      listWrap.appendChild(row);
    });

    const q = filter.value.trim().toLowerCase();
    const filtered = allTrailers.filter(t =>
      !q || t.unit_number.toLowerCase().includes(q)
        || (t.owner_name && t.owner_name.toLowerCase().includes(q))
    );
    if (filtered.length === 0) {
      const raw = filter.value.trim();
      if (raw && !pendingNewUnits.has(raw)) {
        // No known trailer matches — let the driver hook a brand-new one.
        const add = document.createElement('button');
        add.type = 'button';
        add.className = 'btn btn-secondary';
        add.textContent = `Hook new trailer “${raw}”`;
        add.addEventListener('click', () => {
          pendingNewUnits.add(raw);
          filter.value = '';
          renderList();
        });
        listWrap.appendChild(add);
      } else {
        const empty = document.createElement('div');
        empty.className = 'muted';
        empty.textContent = 'No trailers match.';
        listWrap.appendChild(empty);
      }
      return;
    }
    filtered.slice(0, 50).forEach(t => {
      const row = document.createElement('label');
      row.className = 'equipment-pick-row';
      const cb = document.createElement('input');
      cb.type = 'checkbox';
      cb.checked = selectedSet.has(t.id);
      cb.addEventListener('change', () => {
        if (cb.checked) selectedSet.add(t.id);
        else selectedSet.delete(t.id);
      });
      const text = document.createElement('span');
      const unit = document.createElement('strong');
      unit.textContent = t.unit_number;
      text.appendChild(unit);
      const meta = [];
      if (t.owner_name) meta.push(t.owner_name);
      if (t.trailer_type) meta.push(t.trailer_type);
      if (meta.length) {
        const m = document.createElement('span');
        m.className = 'muted';
        m.textContent = ' · ' + meta.join(' · ');
        text.appendChild(m);
      }
      row.appendChild(cb);
      row.appendChild(text);
      listWrap.appendChild(row);
    });
  };
  filter.addEventListener('input', renderList);
  renderList();

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

async function submitTrailerChange(body) {
  const result = await apiFetch('/equipment/trailer', {
    method: 'PUT',
    body,
  });
  return result;
}
