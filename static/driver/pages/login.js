import { loginWithPin, startPasskeyAuth, finishPasskeyAuth } from '../utils/auth.js';
import { navigate } from '../app.js';

export function renderLogin(container) {
  const html = `
    <div class="login-screen">
      <div class="login-header">
        <h1>Driver Portal</h1>
        <p class="subtitle">Sign in to view your trips</p>
      </div>
      <div class="card login-card">
        <div id="login-step-phone">
          <label for="phone-input">Phone number</label>
          <input id="phone-input" type="tel" class="input-field"
                 placeholder="+1 (555) 000-0000" autocomplete="tel">
          <button id="phone-continue-btn" class="btn btn-primary">Continue</button>
          <p id="phone-error" class="error-msg" hidden></p>
        </div>
        <div id="login-step-auth" hidden>
          <p id="auth-phone-display" class="auth-phone"></p>
          <button id="passkey-btn" class="btn btn-primary">Sign in with Passkey</button>
          <div class="divider"><span>or</span></div>
          <div id="pin-section">
            <label for="pin-input">PIN</label>
            <input id="pin-input" type="password" inputmode="numeric" class="input-field"
                   placeholder="Enter PIN" maxlength="6" autocomplete="current-password">
            <button id="pin-btn" class="btn btn-secondary">Sign in with PIN</button>
          </div>
          <p id="auth-error" class="error-msg" hidden></p>
          <button id="back-btn" class="btn btn-secondary back-btn">← Different number</button>
        </div>
        <div id="login-loading" hidden>
          <div class="spinner"></div>
          <p>Signing in...</p>
        </div>
      </div>
    </div>
  `;

  container.innerHTML = html;

  // Wire up state
  const phoneInput = container.querySelector('#phone-input');
  const phoneContinueBtn = container.querySelector('#phone-continue-btn');
  const phoneError = container.querySelector('#phone-error');
  const stepPhone = container.querySelector('#login-step-phone');
  const stepAuth = container.querySelector('#login-step-auth');
  const authPhoneDisplay = container.querySelector('#auth-phone-display');
  const passkeyBtn = container.querySelector('#passkey-btn');
  const pinInput = container.querySelector('#pin-input');
  const pinBtn = container.querySelector('#pin-btn');
  const authError = container.querySelector('#auth-error');
  const backBtn = container.querySelector('#back-btn');
  const loadingDiv = container.querySelector('#login-loading');

  let currentPhone = '';

  function showError(el, msg) {
    el.textContent = msg;
    el.hidden = false;
  }

  function hideError(el) {
    el.hidden = true;
  }

  function setLoading(on) {
    loadingDiv.hidden = !on;
    stepAuth.hidden = on;
  }

  phoneContinueBtn.addEventListener('click', async () => {
    const phone = phoneInput.value.trim();
    if (!phone) {
      showError(phoneError, 'Enter your phone number');
      return;
    }
    const digits = phone.replace(/\D/g, '');
    if (digits.length < 10 || digits.length > 15) {
      showError(phoneError, 'Enter a valid phone number');
      return;
    }
    currentPhone = phone;
    stepPhone.hidden = true;
    stepAuth.hidden = false;
    authPhoneDisplay.textContent = phone;
    hideError(authError);
  });

  passkeyBtn.addEventListener('click', async () => {
    setLoading(true);
    try {
      await startPasskeyAuth(currentPhone);
      // TODO: Full passkey auth flow requires driver_id in challenge response.
      // For v1.3, passkey button directs user to PIN as fallback.
      showError(authError, 'Passkey auth requires additional backend support. Please use PIN.');
    } catch (err) {
      showError(authError, err.message || 'Passkey sign-in failed');
    } finally {
      setLoading(false);
    }
  });

  pinBtn.addEventListener('click', async () => {
    const pin = pinInput.value.trim();
    if (!pin) {
      showError(authError, 'Enter your PIN');
      return;
    }
    if (pin.length < 4) {
      showError(authError, 'PIN must be at least 4 digits');
      return;
    }
    setLoading(true);
    hideError(authError);
    try {
      await loginWithPin(currentPhone, pin);
      navigate('/driver/trips');
    } catch (err) {
      if (err.status === 423) {
        const locked = err.data?.locked_until ? new Date(err.data.locked_until).toLocaleTimeString() : 'later';
        showError(authError, `Account locked. Try again after ${locked}.`);
      } else {
        showError(authError, 'Invalid phone or PIN');
      }
    } finally {
      setLoading(false);
    }
  });

  backBtn.addEventListener('click', () => {
    stepAuth.hidden = true;
    stepPhone.hidden = false;
    hideError(authError);
    pinInput.value = '';
  });

  // Allow Enter key on inputs
  phoneInput.addEventListener('keydown', e => {
    if (e.key === 'Enter') phoneContinueBtn.click();
  });
  pinInput.addEventListener('keydown', e => {
    if (e.key === 'Enter') pinBtn.click();
  });
}
