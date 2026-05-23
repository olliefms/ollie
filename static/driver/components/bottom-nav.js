import { replaceNavigate } from '../app.js';
import { truckIcon, dollarIcon, userIcon, trailerIcon } from './icons.js';

const ITEMS = [
  { id: 'trips',     label: 'Trips',     path: '/driver/trips',     make: truckIcon },
  { id: 'equipment', label: 'Equipment', path: '/driver/equipment', make: trailerIcon },
  { id: 'pay',       label: 'Pay',       path: '/driver/pay',       make: dollarIcon },
  { id: 'account',   label: 'Account',   path: '/driver/account',   make: userIcon },
];

export function renderBottomNav(activeItem) {
  const nav = document.createElement('nav');
  nav.className = 'bottom-nav';
  nav.setAttribute('aria-label', 'Primary');

  ITEMS.forEach(item => {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'bottom-nav__item' + (item.id === activeItem ? ' bottom-nav__item--active' : '');
    btn.setAttribute('aria-label', item.label);

    const iconWrap = document.createElement('span');
    iconWrap.className = 'bottom-nav__icon';
    iconWrap.appendChild(item.make());

    const label = document.createElement('span');
    label.className = 'bottom-nav__label';
    label.textContent = item.label;

    btn.appendChild(iconWrap);
    btn.appendChild(label);
    btn.addEventListener('click', () => {
      if (item.id !== activeItem) replaceNavigate(item.path);
    });
    nav.appendChild(btn);
  });

  return nav;
}
