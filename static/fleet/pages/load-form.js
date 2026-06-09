import { setContent } from '../utils/dom.js';

export function renderLoadForm() {
  setContent('<div class="state-loading"><div class="spinner"></div></div>');
}
