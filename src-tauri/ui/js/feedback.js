import { invoke } from './runtime.js';

const generations = new Map();
const pendingLogs = new Set();
const pollingFailures = new Map();
const visibleFeedback = new Map();
const SUPPORT_COPY = 'Copy logs from the Help page for support.';
const cardSelector = '.session-card,.settings-card,.profile-card,.register-card,.gamepad-card:not(.gamepad-card--stub)';

function initializeFeedback() {
  document.querySelectorAll(cardSelector).forEach(card => {
    if (card.querySelector('[data-feedback]')) return;
    const slot = document.createElement('div');
    slot.className = 'page-feedback';
    slot.dataset.feedback = '';
    if (card.id === 'session-card') slot.id = 'account-feedback';
    slot.setAttribute('role', 'status');
    slot.setAttribute('aria-live', 'polite');
    slot.setAttribute('aria-atomic', 'true');
    if (card.id === 'session-card') card.querySelector('.check-line').after(slot);
    else if (card.classList.contains('gamepad-card')) card.querySelector('.gamepad-card-body')?.append(slot);
    else card.append(slot);
  });
}

function feedbackToken(target) {
  const element = typeof target === 'string' ? document.querySelector(target) : target;
  const screen = element?.closest('[data-screen]') || document.querySelector('[data-screen][data-active]');
  const pane = element?.closest('[data-settings-pane]') || (screen?.dataset.screen === 'settings' ? screen.querySelector('[data-settings-pane][data-active]') : null);
  const surface = pane || screen;
  const card = element?.closest(cardSelector) || element?.querySelector(cardSelector) || surface?.querySelector(cardSelector);
  return { surface, card, page:screen?.dataset.screen || 'launcher', scope:pane ? `settings/${pane.dataset.settingsPane}` : screen?.dataset.screen || 'launcher', generation:generations.get(surface) || 0 };
}

function feedbackCurrent(token) {
  return token && token.generation === (generations.get(token.surface) || 0)
    && token.surface?.hasAttribute('data-active')
    && token.surface.closest('[data-screen]')?.hasAttribute('data-active');
}

function feedbackSlot(token, tone) {
  if (!token?.surface) return null;
  if (token.page === 'help') return token.surface.querySelector(tone === 'success' ? '#help-status' : '#help-feedback');
  if (token.page === 'extensions') return token.surface.querySelector('#extension-feedback');
  const stacked = window.matchMedia('(max-width:900px)').matches;
  const card = stacked && token.card?.isConnected ? token.card : token.surface.querySelector(cardSelector);
  return card?.querySelector('[data-feedback]');
}

function clearFeedback(token, expectedMessage) {
  if (!feedbackCurrent(token)) return;
  const current = visibleFeedback.get(token.surface);
  if (expectedMessage !== undefined && (current?.token !== token || current?.message !== expectedMessage)) return;
  visibleFeedback.delete(token.surface);
  token.surface.querySelectorAll('[data-feedback]').forEach(slot => {
    slot.textContent = '';
    delete slot.dataset.tone;
  });
}

function showFeedback(token, message, tone = '') {
  if (!feedbackCurrent(token)) return;
  clearFeedback(token);
  const slot = feedbackSlot(token, tone);
  if (!slot) return;
  slot.textContent = message;
  slot.dataset.tone = tone;
  if (message) visibleFeedback.set(token.surface, { token, message, tone });
}

function leaveFeedback() {
  const screen = document.querySelector('[data-screen][data-active]');
  if (!screen) return;
  [screen, ...screen.querySelectorAll('[data-settings-pane]')].forEach(surface => {
    generations.set(surface, (generations.get(surface) || 0) + 1);
    visibleFeedback.delete(surface);
  });
  screen.querySelectorAll('[data-feedback],.register-field-error').forEach(slot => { slot.textContent = ''; });
  screen.querySelectorAll('[aria-invalid]').forEach(input => input.setAttribute('aria-invalid', 'false'));
}

function diagnosticText(error) {
  if (error instanceof Error) return `${error.stack || error.message}${error.cause ? `\nCaused by: ${diagnosticText(error.cause)}` : ''}`;
  if (typeof error === 'string') return error;
  try { return JSON.stringify(error); } catch { return String(error); }
}

function recordFailure(page, action, error, message, context = '') {
  const write = invoke('record_ui_failure', { page, action, message, diagnostic:diagnosticText(error) || 'Unknown failure', context })
    .catch(loggingError => { console.error('Could not persist launcher failure.', loggingError); });
  pendingLogs.add(write);
  write.finally(() => pendingLogs.delete(write));
  return write;
}

function reportFailure(token, action, error, { message = `Couldn't ${action}. ${SUPPORT_COPY}`, context = '' } = {}) {
  const write = recordFailure(token?.scope || 'launcher', action, error, message, context);
  showFeedback(token, message, 'error');
  return write;
}

function recordPollingFailure(page, action, error, message, context = '') {
  const key = `${page}/${action}`;
  const diagnostic = diagnosticText(error);
  if (pollingFailures.get(key) === diagnostic) return;
  pollingFailures.set(key, diagnostic);
  return recordFailure(page, action, error, message, context);
}

function pollingRecovered(page, action) { pollingFailures.delete(`${page}/${action}`); }
async function flushFailureLogs() { while (pendingLogs.size) await Promise.all([...pendingLogs]); }

window.addEventListener('resize', () => {
  [...visibleFeedback.values()].forEach(({ token, message, tone }) => showFeedback(token, message, tone));
});

export { SUPPORT_COPY, initializeFeedback, feedbackToken, feedbackCurrent, clearFeedback, showFeedback, leaveFeedback, recordFailure, reportFailure, recordPollingFailure, pollingRecovered, flushFailureLogs };
