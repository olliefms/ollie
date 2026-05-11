import { clearAuth, getDriverId } from '../utils/auth.js';
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

  // Profile section
  const profileSection = document.createElement('div');
  profileSection.className = 'stop-detail-section';

  const profileLabel = document.createElement('div');
  profileLabel.className = 'stop-detail-section-label';
  profileLabel.textContent = 'Profile';
  profileSection.appendChild(profileLabel);

  const profileLoading = document.createElement('div');
  profileLoading.className = 'stop-detail-row';
  profileLoading.textContent = 'Loading…';
  profileSection.appendChild(profileLoading);

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
  passkeyStatusMsg.className = 'passkey-status';

  // Use window.PublicKeyCredential as the correct WebAuthn capability check
  if (!window.PublicKeyCredential) {
    passkeyRow.textContent = 'Passkeys not supported on this device';
    securitySection.appendChild(passkeyRow);
  } else {
    const addPasskeyBtn = document.createElement('button');
    addPasskeyBtn.className = 'btn btn-primary';
    addPasskeyBtn.textContent = 'Add Passkey';

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
        // Step 1: Begin registration — single endpoint, phase="start"
        const beginResp = await apiFetch('/auth/register-passkey', {
          method: 'POST',
          body: { phase: 'start' },
        });

        // Backend returns { challenge: <CCR JSON with publicKey: {...}> }
        const pkOptions = beginResp.challenge.publicKey;

        // Step 2: Decode binary fields for WebAuthn API
        pkOptions.challenge = base64urlToBuffer(pkOptions.challenge);
        pkOptions.user.id = base64urlToBuffer(pkOptions.user.id);

        // Step 3: Create credential via browser WebAuthn API
        const credential = await navigator.credentials.create({ publicKey: pkOptions });

        // Step 4: Finish registration — phase="finish", response=credential JSON
        await apiFetch('/auth/register-passkey', {
          method: 'POST',
          body: {
            phase: 'finish',
            response: credentialToJSON(credential),
          },
        });

        // Success
        addPasskeyBtn.style.display = 'none';
        passkeyStatusMsg.textContent = 'Passkey registered! ✓';
        passkeyStatusMsg.className = 'passkey-status--success';
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

  // Account / logout section
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

  // Load driver profile asynchronously
  try {
    const driver = await apiFetch('/me');
    profileLoading.remove();

    const nameRowEl = document.createElement('div');
    nameRowEl.className = 'stop-detail-row';
    nameRowEl.textContent = `Name: ${driver.name}`;
    profileSection.appendChild(nameRowEl);

    if (driver.phone) {
      const phoneRowEl = document.createElement('div');
      phoneRowEl.className = 'stop-detail-row';
      phoneRowEl.textContent = `Phone: ${driver.phone}`;
      profileSection.appendChild(phoneRowEl);
    }
  } catch (err) {
    profileLoading.textContent = err.message || 'Failed to load profile';
    profileLoading.style.color = 'var(--color-danger)';
  }
}
