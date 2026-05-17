import { isAuthenticated } from '../utils/auth.js';
import { renderAppBar } from '../components/app-bar.js';
import { renderBottomNav } from '../components/bottom-nav.js';

export function renderPay(container) {
  if (!isAuthenticated()) {
    window.location.replace('/driver');
    return;
  }
  container.replaceChildren();
  const page = document.createElement('div');
  page.className = 'page-with-nav';
  page.appendChild(renderAppBar({ title: 'Pay' }));
  const empty = document.createElement('div');
  empty.className = 'empty-state';
  empty.textContent = 'Pay periods coming soon.';
  page.appendChild(empty);
  page.appendChild(renderBottomNav('pay'));
  container.appendChild(page);
}
