import { keyIcon, chevronUpIcon, logoutIcon } from './icons.js';
import { scopeGranted } from './scope-gate.js';
import { getTheme, setTheme } from '../utils/theme.js';

const ROLE_LABELS = {
  owner: 'Owner',
  fleet_manager: 'Fleet Manager',
  dispatcher: 'Dispatcher',
};

export function initials(name, email) {
  const n = (name || '').trim();
  if (n) {
    return n.split(/\s+/).slice(0, 2).map(w => w[0].toUpperCase()).join('');
  }
  const e = (email || '').trim();
  return e ? e[0].toUpperCase() : '?';
}

export function roleLabel(role) {
  return ROLE_LABELS[role] || (role || '');
}

const THEME_CHOICES = [
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
  { value: 'system', label: 'System' },
];

function buildThemeSwitch() {
  const wrap = document.createElement('div');
  wrap.className = 'sidebar__theme';
  const current = getTheme();
  const buttons = [];
  for (const choice of THEME_CHOICES) {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'sidebar__theme-btn' + (choice.value === current ? ' is-active' : '');
    btn.dataset.themeChoice = choice.value;
    btn.textContent = choice.label;
    btn.addEventListener('click', () => {
      setTheme(choice.value);
      buttons.forEach(b => b.classList.toggle('is-active', b === btn));
    });
    buttons.push(btn);
    wrap.appendChild(btn);
  }
  return wrap;
}

export function renderAccountFooter(container, { identity, scopes = [], onSignOut } = {}) {
  const id = identity || {};
  container.replaceChildren();

  const menu = document.createElement('div');
  menu.className = 'sidebar__menu';
  menu.hidden = true;

  if (scopeGranted(scopes, 'api_keys:read')) {
    const account = document.createElement('a');
    account.className = 'sidebar__menu-item';
    account.dataset.link = '';
    account.setAttribute('href', '/fleet/account');
    account.appendChild(keyIcon());
    account.appendChild(document.createTextNode('Account'));
    menu.appendChild(account);
  }

  menu.appendChild(buildThemeSwitch());

  const signOut = document.createElement('button');
  signOut.type = 'button';
  signOut.className = 'sidebar__menu-item';
  signOut.dataset.action = 'sign-out';
  signOut.appendChild(logoutIcon());
  signOut.appendChild(document.createTextNode('Sign out'));
  signOut.addEventListener('click', () => { if (onSignOut) onSignOut(); });
  menu.appendChild(signOut);

  const chip = document.createElement('button');
  chip.type = 'button';
  chip.className = 'sidebar__account';

  const avatar = document.createElement('span');
  avatar.className = 'sidebar__avatar';
  avatar.textContent = initials(id.name, id.email);

  const meta = document.createElement('span');
  meta.className = 'sidebar__account-meta';
  const nameEl = document.createElement('span');
  nameEl.className = 'sidebar__account-name';
  nameEl.textContent = id.name || id.email || 'Signed in';
  const roleEl = document.createElement('span');
  roleEl.className = 'sidebar__account-role';
  roleEl.textContent = roleLabel(id.role);
  meta.append(nameEl, roleEl);

  const chev = document.createElement('span');
  chev.className = 'sidebar__account-chev';
  chev.appendChild(chevronUpIcon());

  chip.append(avatar, meta, chev);

  let justOpened = false;
  const close = () => {
    menu.hidden = true;
    document.removeEventListener('keydown', onKey);
    document.removeEventListener('click', onOutside);
  };
  const onKey = (e) => { if (e.key === 'Escape') close(); };
  const onOutside = (e) => {
    if (justOpened) { justOpened = false; return; }
    if (!container.contains(e.target)) close();
  };
  const open = () => {
    menu.hidden = false;
    justOpened = true;
    document.addEventListener('keydown', onKey);
    document.addEventListener('click', onOutside);
  };

  chip.addEventListener('click', () => {
    if (menu.hidden) open(); else close();
  });

  container.append(menu, chip);
}
