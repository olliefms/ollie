import { setContent, goBack } from '../utils/dom.js';
import { renderForm } from '../components/form.js';

/**
 * Render an entity create/edit form as a page: back link + the renderForm
 * inline panel. `opts` is passed straight through to renderForm
 * ({ title, fields, values, submitLabel, onSubmit }).
 */
export function renderFormPage(opts) {
  setContent('<button class="back-link" id="form-back">← Back</button><div id="form-host"></div>');
  document.getElementById('form-back').addEventListener('click', goBack);
  renderForm(document.getElementById('form-host'), opts);
}
