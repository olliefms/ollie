export function renderAppBar({ title, right = null }) {
  const bar = document.createElement('header');
  bar.className = 'app-bar';
  const h = document.createElement('h1');
  h.className = 'app-bar__title';
  h.textContent = title;
  bar.appendChild(h);
  if (right) {
    const slot = document.createElement('div');
    slot.className = 'app-bar__right';
    slot.appendChild(right);
    bar.appendChild(slot);
  }
  return bar;
}
