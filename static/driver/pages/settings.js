import { clearAuth } from '../utils/auth.js';
import { apiFetch } from '../utils/api.js';
import { navigate } from '../app.js';

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

export async function renderSettings(container) {
  container.innerHTML = '';

  const page = document.createElement('div');
  page.className = 'stop-detail-page';

  // Header
  const header = document.createElement('div');
  header.className = 'stop-detail-header';

  const backBtn = document.createElement('button');
  backBtn.className = 'btn btn-secondary stop-detail-back';
  backBtn.textContent = '← Back';
  backBtn.addEventListener('click', () => navigate('/driver/trips'));

  const title = document.createElement('h1');
  title.className = 'stop-detail-title';
  title.textContent = 'Settings';

  header.appendChild(backBtn);
  header.appendChild(title);
  page.appendChild(header);

  // Profile section placeholder
  const profileSection = document.createElement('div');
  profileSection.className = 'stop-detail-section';

  const profileLabel = document.createElement('div');
  profileLabel.className = 'stop-detail-section-label';
  profileLabel.textContent = 'Profile';
  profileSection.appendChild(profileLabel);

  const nameRow = document.createElement('div');
  nameRow.className = 'stop-detail-row';
  nameRow.textContent = 'Loading…';
  profileSection.appendChild(nameRow);

  page.appendChild(profileSection);

  // Security section
  const securitySection = document.createElement('div');
  securitySection.className = 'stop-detail-section';

  const securityLabel = document.createElement('div');
  securityLabel.className = 'stop-detail-section-label';
  securityLabel.textContent = 'Security';
  securitySection.appendChild(securityLabel);

  const passkeyRow = document.createElement('div');
  passkeyRow.className = 'stop-detail-row';

  const passkeyStatusMsg = document.createElement('div');
  passkeyStatusMsg.style.marginTop = '0.5rem';

  if (!navigator.credentials) {
    passkeyRow.textContent = 'Passkeys not supported on this device';
    securitySection.appendChild(passkeyRow);
  } else {
    const addPasskeyBtn = document.createElement('button');
    addPasskeyBtn.className = 'btn btn-primary';
    addPasskeyBtn.textContent = 'Add Passkey';

    addPasskeyBtn.addEventListener('click', async () => {
      addPasskeyBtn.disabled = true;
      passkeyStatusMsg.textContent = '';
      passkeyStatusMsg.className = '';

      try {
        // Step 1: Begin registration
        const options = await apiFetch('/auth/passkey/register/begin', {
          method: 'POST',
          body: {},
        });

        // Step 2: Decode binary fields
        options.challenge = base64urlToBuffer(options.challenge);
        options.user.id = base64urlToBuffer(options.user.id);

        // Step 3: Create credential
        const credential = await navigator.credentials.create({ publicKey: options });

        // Step 4: Finish registration
        await apiFetch('/auth/passkey/register/finish', {
          method: 'POST',
          body: credentialToJSON(credential),
        });

        // Success
        addPasskeyBtn.style.display = 'none';
        passkeyStatusMsg.textContent = 'Passkey registered! ✓';
        passkeyStatusMsg.className = '';
        passkeyStatusMsg.style.color = 'var(--color-success)';
        passkeyStatusMsg.style.fontWeight = '600';
      } catch (err) {
        addPasskeyBtn.disabled = false;
        passkeyStatusMsg.textContent = err.message || 'Failed to register passkey';
        passkeyStatusMsg.className = 'error-msg';
      }
    });

    passkeyRow.appendChild(addPasskeyBtn);
    passkeyRow.appendChild(passkeyStatusMsg);
    securitySection.appendChild(passkeyRow);
  }

  page.appendChild(securitySection);

  // Danger / logout section
  const dangerSection = document.createElement('div');
  dangerSection.className = 'stop-detail-section';

  const dangerLabel = document.createElement('div');
  dangerLabel.className = 'stop-detail-section-label';
  dangerLabel.textContent = 'Account';
  dangerSection.appendChild(dangerLabel);

  const logoutRow = document.createElement('div');
  logoutRow.className = 'stop-detail-row';

  const logoutBtn = document.createElement('button');
  logoutBtn.className = 'btn btn-secondary';
  logoutBtn.textContent = 'Log Out';
  logoutBtn.addEventListener('click', () => {
    clearAuth();
    navigate('/driver');
  });

  logoutRow.appendChild(logoutBtn);
  dangerSection.appendChild(logoutRow);
  page.appendChild(dangerSection);

  container.appendChild(page);

  // Load driver profile
  try {
    const driver = await apiFetch('/me');
    nameRow.innerHTML = '';

    const nameRowEl = document.createElement('div');
    nameRowEl.className = 'stop-detail-row';
    nameRowEl.textContent = `Name: ${driver.name}`;
    profileSection.appendChild(nameRowEl);

    const phoneRowEl = document.createElement('div');
    phoneRowEl.className = 'stop-detail-row';
    phoneRowEl.textContent = `Phone: ${driver.phone}`;
    profileSection.appendChild(phoneRowEl);

    nameRow.remove();
  } catch (err) {
    nameRow.textContent = err.message || 'Failed to load profile';
    nameRow.style.color = 'var(--color-danger)';
  }
}
