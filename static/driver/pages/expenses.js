import { isAuthenticated, clearAuth, getToken } from '../utils/auth.js';
import { apiFetch } from '../utils/api.js';
import { navigate } from '../app.js';
import { renderAppBar } from '../components/app-bar.js';
import { renderBottomNav } from '../components/bottom-nav.js';
import { trashIcon } from '../components/icons.js';
import { categoryLabel, statusLabel, formatMoney, expenseDateStr } from '../utils/expense-meta.js';

export function renderExpenseCard(expense, onDelete) {
  const card = document.createElement('div');
  card.className = 'expense-card';

  const header = document.createElement('div');
  header.className = 'expense-card__header';

  const category = document.createElement('span');
  category.className = 'expense-card__category';
  category.textContent = categoryLabel(expense.category);
  header.appendChild(category);

  const chip = document.createElement('span');
  chip.className = `badge badge--${expense.status}`;
  chip.textContent = statusLabel(expense.status);
  header.appendChild(chip);

  card.appendChild(header);

  const date = document.createElement('div');
  date.className = 'expense-card__row';
  date.textContent = expenseDateStr(expense);
  card.appendChild(date);

  const amount = formatMoney(expense.amount);
  const approved = formatMoney(expense.approved_amount);
  if (amount || approved) {
    const amountRow = document.createElement('div');
    amountRow.className = 'expense-card__row';
    amountRow.textContent = amount || '—';
    if (approved) {
      amountRow.appendChild(document.createTextNode(' '));
      const approvedSpan = document.createElement('span');
      approvedSpan.className = 'expense-card__approved';
      approvedSpan.textContent = `(Approved: ${approved})`;
      amountRow.appendChild(approvedSpan);
    }
    card.appendChild(amountRow);
  }

  if (expense.review_note) {
    const noteRow = document.createElement('div');
    noteRow.className = 'expense-card__row expense-card__note';
    noteRow.textContent = expense.review_note;
    card.appendChild(noteRow);
  }

  const reimbursement = formatMoney(expense.reimbursement);
  const deduction = formatMoney(expense.deduction);
  if (reimbursement) {
    const row = document.createElement('div');
    row.className = 'expense-card__row expense-card__money expense-card__money--reimbursement';
    row.textContent = `Reimbursement: ${reimbursement}`;
    card.appendChild(row);
  } else if (deduction) {
    const row = document.createElement('div');
    row.className = 'expense-card__row expense-card__money expense-card__money--deduction';
    row.textContent = `Deducted: ${deduction}`;
    card.appendChild(row);
  }

  if (expense.status === 'submitted') {
    const del = document.createElement('button');
    del.type = 'button';
    del.className = 'expense-card__delete';
    del.appendChild(trashIcon());
    del.setAttribute('aria-label', 'Delete expense');
    del.addEventListener('click', () => onDelete(expense.id));
    card.appendChild(del);
  }

  return card;
}

function deleteErrorMessage(status) {
  if (status === 403) return 'This expense has been reviewed and can no longer be deleted.';
  if (status === 409) return 'This expense has been settled and is locked.';
  return 'Failed to delete expense.';
}

export async function renderExpenses(container) {
  if (!isAuthenticated()) {
    window.location.replace('/driver');
    return;
  }

  const renderToken = Symbol('expenses-render');
  container.__renderToken = renderToken;
  container.replaceChildren();

  const page = document.createElement('div');
  page.className = 'page-with-nav';

  const backBtn = document.createElement('button');
  backBtn.type = 'button';
  backBtn.className = 'btn-ghost-back';
  backBtn.textContent = '← Back';
  backBtn.addEventListener('click', () => {
    if (history.length > 1) history.back();
    else navigate('/driver/account');
  });

  page.appendChild(renderAppBar({ title: 'Expenses', right: backBtn }));

  const body = document.createElement('div');
  body.className = 'expenses-body';
  page.appendChild(body);
  page.appendChild(renderBottomNav('account'));
  container.appendChild(page);

  async function handleDelete(id) {
    if (!confirm('Delete this expense?')) return;
    try {
      const r = await fetch(`/driver/api/v1/expenses/${id}`, {
        method: 'DELETE',
        headers: { 'Authorization': `Bearer ${getToken()}` },
      });
      if (!r.ok) throw new Error(deleteErrorMessage(r.status));
      await load();
    } catch (err) {
      alert(err.message || 'Failed to delete expense');
    }
  }

  async function load() {
    body.replaceChildren();
    const spinner = document.createElement('div');
    spinner.className = 'spinner';
    body.appendChild(spinner);
    try {
      const data = await apiFetch('/expenses');
      if (container.__renderToken !== renderToken) return;
      body.replaceChildren();
      const items = data.items || [];
      if (items.length === 0) {
        const empty = document.createElement('div');
        empty.className = 'expenses-empty';
        empty.textContent = 'No expenses submitted yet.';
        body.appendChild(empty);
        return;
      }
      items.forEach(expense => body.appendChild(renderExpenseCard(expense, handleDelete)));
    } catch (err) {
      if (container.__renderToken !== renderToken) return;
      if (err.status === 401) {
        clearAuth();
        window.location.replace('/driver');
        return;
      }
      body.replaceChildren();
      const errorEl = document.createElement('div');
      errorEl.className = 'expenses-error';
      errorEl.textContent = err.message || 'Failed to load expenses';
      body.appendChild(errorEl);
    }
  }

  await load();
}
