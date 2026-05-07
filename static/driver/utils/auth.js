import { apiFetch } from './api.js';

const TOKEN_KEY = 'driver_token';
const DRIVER_ID_KEY = 'driver_id';

export function getToken() {
  return localStorage.getItem(TOKEN_KEY);
}

export function getDriverId() {
  return localStorage.getItem(DRIVER_ID_KEY);
}

export function isAuthenticated() {
  return !!getToken();
}

export function saveAuth(token, driverId) {
  localStorage.setItem(TOKEN_KEY, token);
  if (driverId) localStorage.setItem(DRIVER_ID_KEY, driverId);
}

export function clearAuth() {
  localStorage.removeItem(TOKEN_KEY);
  localStorage.removeItem(DRIVER_ID_KEY);
}

// PIN login
export async function loginWithPin(phone, pin) {
  const data = await apiFetch('/auth/pin', { method: 'POST', body: { phone, pin } });
  // Decode JWT to get driver_id (no verification — server already verified)
  let payload;
  try {
    payload = JSON.parse(atob(data.token.split('.')[1]));
  } catch {
    throw new Error('Invalid token received from server');
  }
  saveAuth(data.token, payload.driver_id);
  return data;
}

// Passkey: start challenge
export async function startPasskeyAuth(phone) {
  return apiFetch('/auth/challenge', { method: 'POST', body: { phone } });
}

// Passkey: finish verification
export async function finishPasskeyAuth(driverId, publicKeyCredential) {
  const data = await apiFetch('/auth/verify', {
    method: 'POST',
    body: { driver_id: driverId, response: credentialToJSON(publicKeyCredential) }
  });
  let payload;
  try {
    payload = JSON.parse(atob(data.token.split('.')[1]));
  } catch {
    throw new Error('Invalid token received from server');
  }
  saveAuth(data.token, payload.driver_id);
  return data;
}

// Convert PublicKeyCredential to JSON-serializable form
function credentialToJSON(cred) {
  return {
    id: cred.id,
    rawId: bufferToBase64url(cred.rawId),
    type: cred.type,
    response: {
      authenticatorData: bufferToBase64url(cred.response.authenticatorData),
      clientDataJSON: bufferToBase64url(cred.response.clientDataJSON),
      signature: bufferToBase64url(cred.response.signature),
      userHandle: cred.response.userHandle ? bufferToBase64url(cred.response.userHandle) : null,
    },
  };
}

function bufferToBase64url(buffer) {
  const bytes = new Uint8Array(buffer);
  let str = '';
  for (const b of bytes) str += String.fromCharCode(b);
  return btoa(str).replace(/\+/g, '-').replace(/\//g, '_').replace(/=/g, '');
}
