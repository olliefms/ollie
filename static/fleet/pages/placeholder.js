import { escHtml } from '../utils/format.js';

/** Render a "coming soon" placeholder for entity surfaces not yet built. */
export function renderPlaceholder(container, name) {
  container.innerHTML = `<div class="state-empty">
    <p>${escHtml(name)} management is coming in a later release.</p>
  </div>`;
}
