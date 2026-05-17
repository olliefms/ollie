import { isAuthenticated, clearAuth } from '../utils/auth.js';
import { apiFetch } from '../utils/api.js';
import { renderAppBar } from '../components/app-bar.js';
import { renderBottomNav } from '../components/bottom-nav.js';
import { navigate } from '../app.js';

const APP_VERSION = 'v1.13.0';

export async function renderAccount(container) {
  if (!isAuthenticated()) {
    window.location.replace('/driver');
    return;
  }
  container.replaceChildren();

  const page = document.createElement('div');
  page.className = 'page-with-nav';
  page.appendChild(renderAppBar({ title: 'Account' }));

  const body = document.createElement('div');
  body.className = 'account-body';

  // Profile card
  const profileCard = document.createElement('div');
  profileCard.className = 'account-card';

  const nameEl = document.createElement('div');
  nameEl.className = 'account-name';
  nameEl.textContent = 'Loading…';
  profileCard.appendChild(nameEl);

  const phoneEl = document.createElement('div');
  phoneEl.className = 'account-phone';
  profileCard.appendChild(phoneEl);

  const statusEl = document.createElement('span');
  statusEl.className = 'badge';
  profileCard.appendChild(statusEl);

  body.appendChild(profileCard);

  // Settings rows
  const settingsList = document.createElement('div');
  settingsList.className = 'account-settings';

  const logoutRow = document.createElement('button');
  logoutRow.type = 'button';
  logoutRow.className = 'account-row account-row--button';
  logoutRow.textContent = 'Log Out';
  logoutRow.addEventListener('click', () => {
    clearAuth();
    navigate('/driver');
  });
  settingsList.appendChild(logoutRow);

  const versionRow = document.createElement('div');
  versionRow.className = 'account-row';
  const versionLabel = document.createElement('span');
  versionLabel.textContent = 'Version';
  const versionValue = document.createElement('span');
  versionValue.className = 'account-row__value';
  versionValue.textContent = APP_VERSION;
  versionRow.appendChild(versionLabel);
  versionRow.appendChild(versionValue);
  settingsList.appendChild(versionRow);

  body.appendChild(settingsList);
  page.appendChild(body);
  page.appendChild(renderBottomNav('account'));
  container.appendChild(page);

  // Load profile
  try {
    const driver = await apiFetch('/me');
    nameEl.textContent = driver.name || 'Driver';
    if (driver.phone) {
      phoneEl.textContent = driver.phone;
    } else {
      phoneEl.remove();
    }
    if (driver.status) {
      statusEl.classList.add('badge--' + driver.status);
      statusEl.textContent = driver.status;
    } else {
      statusEl.remove();
    }
  } catch (err) {
    nameEl.textContent = err.message || 'Failed to load profile';
    nameEl.classList.add('account-name--error');
    phoneEl.remove();
    statusEl.remove();
  }
}
