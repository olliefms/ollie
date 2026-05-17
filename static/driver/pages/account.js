import { isAuthenticated, clearAuth, getDriverId } from '../utils/auth.js';
import { apiFetch } from '../utils/api.js';
import { renderAppBar } from '../components/app-bar.js';
import { renderBottomNav } from '../components/bottom-nav.js';
import { navigate } from '../app.js';

const APP_VERSION = 'v1.13.0';

function base64urlToBuffer(base64url) {
  const base64 = base64url.replace(/-/g, '+').replace(/_/g, '/');
  const padded = base64.padEnd(base64.length + (4 - base64.length % 4) % 4, '=');
  const binary = atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes.buffer;
}

function bufferToBase64url(buffer) {
  const bytes = new Uint8Array(buffer);
  let str = '';
  for (const b of bytes) str += String.fromCharCode(b);
  return btoa(str).replace(/\+/g, '-').replace(/\//g, '_').replace(/=/g, '');
}

function credentialToJSON(cred) {
  return {
    id: cred.id,
    rawId: bufferToBase64url(cred.rawId),
    type: cred.type,
    response: {
      attestationObject: bufferToBase64url(cred.response.attestationObject),
      clientDataJSON: bufferToBase64url(cred.response.clientDataJSON),
    },
  };
}

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

  // Security section (passkey enrollment)
  const securityList = document.createElement('div');
  securityList.className = 'account-settings';

  const passkeyRow = document.createElement('div');
  passkeyRow.className = 'account-row';

  if (!window.PublicKeyCredential) {
    passkeyRow.textContent = 'Passkeys not supported on this device';
    securityList.appendChild(passkeyRow);
  } else {
    const passkeyLabel = document.createElement('span');
    passkeyLabel.textContent = 'Passkey';
    passkeyRow.appendChild(passkeyLabel);

    const passkeyAction = document.createElement('div');

    const addPasskeyBtn = document.createElement('button');
    addPasskeyBtn.type = 'button';
    addPasskeyBtn.className = 'btn btn-primary';
    addPasskeyBtn.textContent = 'Add Passkey';

    const passkeyStatusMsg = document.createElement('div');
    passkeyStatusMsg.className = 'passkey-status';

    addPasskeyBtn.addEventListener('click', async () => {
      const driverId = getDriverId();
      if (!driverId) {
        passkeyStatusMsg.textContent = 'Session expired — please log in again';
        passkeyStatusMsg.className = 'error-msg';
        return;
      }

      addPasskeyBtn.disabled = true;
      passkeyStatusMsg.textContent = '';
      passkeyStatusMsg.className = 'passkey-status';

      try {
        const beginResp = await apiFetch('/auth/register-passkey', {
          method: 'POST',
          body: { phase: 'start' },
        });

        const pkOptions = beginResp.challenge.publicKey;

        pkOptions.challenge = base64urlToBuffer(pkOptions.challenge);
        pkOptions.user.id = base64urlToBuffer(pkOptions.user.id);

        const credential = await navigator.credentials.create({ publicKey: pkOptions });

        await apiFetch('/auth/register-passkey', {
          method: 'POST',
          body: {
            phase: 'finish',
            response: credentialToJSON(credential),
          },
        });

        addPasskeyBtn.style.display = 'none';
        passkeyStatusMsg.textContent = 'Passkey registered! ✓';
        passkeyStatusMsg.className = 'passkey-status--success';
      } catch (err) {
        addPasskeyBtn.disabled = false;
        passkeyStatusMsg.textContent = err.message || 'Failed to register passkey';
        passkeyStatusMsg.className = 'error-msg';
      }
    });

    passkeyAction.appendChild(addPasskeyBtn);
    passkeyAction.appendChild(passkeyStatusMsg);
    passkeyRow.appendChild(passkeyAction);
    securityList.appendChild(passkeyRow);
  }

  body.appendChild(securityList);

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
