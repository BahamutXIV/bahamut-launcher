import { body, launcherShell, tauriInvoke, home, settingsState, extensionState, invoke, selectedServer, normalizeLauncherLog } from './runtime.js';
import { renderChoice, setNestedValue, queueGameSettingsUpdate, queueBorderlessMonitorUpdate, graphicsRangeSelection, renderGraphicsRangePreview, queueObjectDistanceSelection, queueCameraZoomSelection, queueLauncherBehaviorUpdate, queueNativeResolutionOverrideUpdate, hydrateSettings, renderServerSettings, renderGameSettingProgress } from './settings.js';
import { hydrateExtensions, renderExtensionLibrary } from './extensions.js';
import { repairState, refreshGameRepairStatus, startGameRepair, controlGameRepair } from './game-repair.js';
import { hydrateLauncherUpdates, handleLauncherUpdateAction, refreshLauncherUpdateAvailability, startBackgroundLauncherUpdateCheck } from './launcher-updates.js';
import { renderNews, renderHome, renderLifecycleStrip, refreshGameStatus, refreshInstallSnapshot, refreshHomeStatus, restoreSession, chooseInstallFolder, startInstall, launchGame, handleSettingsAction } from './home.js';

let activePrimaryRoute = 'home';
const PROFILE_SUCCESS_MS = 2500;
const BACKUP_STATUS_MS = 8000;
const profileFeedbackTimers = new Map();
let backupStatusTimer = null;
let backupBusy = false;
let settingsConfirmationResolver = null;
let settingsDialogOpener = null;

function setProfileFeedback(element, message, tone = '') {
  const pending = profileFeedbackTimers.get(element);
  if (pending) clearTimeout(pending);
  profileFeedbackTimers.delete(element);
  element.textContent = message;
  if (tone) element.dataset.tone = tone;
  else delete element.dataset.tone;
  if (message && tone === 'success') {
    profileFeedbackTimers.set(element, setTimeout(() => {
      element.textContent = '';
      delete element.dataset.tone;
      profileFeedbackTimers.delete(element);
    }, PROFILE_SUCCESS_MS));
  }
}

function setBackupStatus(message, tone = '') {
  const status = document.querySelector('#settings-backup-status');
  if (backupStatusTimer) clearTimeout(backupStatusTimer);
  backupStatusTimer = null;
  status.textContent = message;
  if (tone) status.dataset.tone = tone;
  else delete status.dataset.tone;
  if (message && tone === 'success') {
    backupStatusTimer = setTimeout(() => {
      status.textContent = '';
      delete status.dataset.tone;
      backupStatusTimer = null;
    }, BACKUP_STATUS_MS);
  }
}

function closeSettingsConfirmation(confirmed) {
  const dialog = document.querySelector('#settings-confirmation-dialog');
  if (dialog.open) dialog.close();
  const resolve = settingsConfirmationResolver;
  settingsConfirmationResolver = null;
  resolve?.(confirmed);
  if (!confirmed) {
    settingsDialogOpener?.focus({ preventScroll:true });
    settingsDialogOpener = null;
  }
}

function showSettingsConfirmation({ title, copy, confirmLabel = 'Confirm', cancelLabel = 'Cancel', message = false }) {
  const dialog = document.querySelector('#settings-confirmation-dialog');
  const cancel = document.querySelector('#settings-confirmation-cancel');
  const confirm = document.querySelector('#settings-confirmation-confirm');
  document.querySelector('#settings-confirmation-title').textContent = title;
  document.querySelector('#settings-confirmation-copy').textContent = copy;
  cancel.textContent = cancelLabel;
  cancel.hidden = message;
  confirm.textContent = confirmLabel;
  settingsDialogOpener = document.activeElement;
  dialog.showModal();
  (message ? confirm : cancel).focus();
  return new Promise(resolve => { settingsConfirmationResolver = resolve; });
}

function confirmRestore(target) {
  const label = target === 'user-settings' ? 'User Settings and Macros' : 'Extensions';
  return showSettingsConfirmation({
    title:`Restore ${label}?`,
    copy:target === 'extensions'
      ? 'Replace the current Extensions settings and installed Lua/DAT packages with the latest backup? This cannot be undone.'
      : 'Replace the current User Settings and Macros files with the latest backup? This cannot be undone.',
  });
}

async function runRepairInstall() {
  if (repairState.starting || repairState.status?.is_running) return;
  const confirmed = await showSettingsConfirmation({
    title:'Repair Install?',
    copy:'This may require downloading the full 7.2 GB game archive. Proceed?',
  });
  if (!confirmed) return;
  settingsDialogOpener = null;
  activateRoute('home');
  const operation = startGameRepair();
  renderHome();
  await operation;
  renderHome();
}

async function runBackupAction(button) {
  if (backupBusy) return;
  backupBusy = true;
  const action = button.dataset.backupAction;
  const target = button.dataset.backupTarget;
  try {
    if (action === 'restore' && !(await confirmRestore(target))) return;
    document.querySelectorAll('[data-backup-action]').forEach(control => { control.disabled = true; });
    const label = target === 'user-settings' ? 'User Settings and Macros' : 'Extensions';
    setBackupStatus(action === 'create' ? `Creating ${label} backup...` : `Restoring ${label}...`);
    const message = await invoke(action === 'create' ? 'create_backup' : 'restore_backup', { target });
    if (action === 'restore' && target === 'user-settings') await hydrateSettings();
    if (action === 'restore' && target === 'extensions') await hydrateExtensions();
    setBackupStatus(message, 'success');
  } catch (error) {
    setBackupStatus(error.message || String(error), 'error');
  } finally {
    backupBusy = false;
    document.querySelectorAll('[data-backup-action]').forEach(control => { control.disabled = false; });
    const focusTarget = settingsDialogOpener || button;
    settingsDialogOpener = null;
    focusTarget?.focus({ preventScroll:true });
  }
}

const registerFields = {
  username: document.querySelector('#register-username'),
  password: document.querySelector('#register-password'),
  confirm: document.querySelector('#register-confirm'),
};
let registerBusy = false;
const registerSubmitErrors = {
  'username-taken': 'That username is already taken.',
  'rate-limited': 'Too many attempts. Wait a few minutes.',
  network: 'Server unavailable.',
  'create-failed': 'Account creation failed. Create a ticket on Discord.',
};

function registerValidation() {
  const usernameDraft = registerFields.username.value;
  const username = usernameDraft.trim();
  const password = registerFields.password.value;
  const confirm = registerFields.confirm.value;
  return {
    username: usernameDraft.length > 0 && (username.length < 3 || username.length > 32) ? '3-32 characters.' : '',
    password: password.length > 0 && (password.length < 8 || password.length > 128) ? '8-128 characters.' : '',
    confirm: confirm.length > 0 && password !== confirm ? 'Passwords do not match.' : '',
    complete: username.length >= 3 && username.length <= 32 && password.length >= 8 && password.length <= 128 && password === confirm,
  };
}

function renderRegisterValidation(clearSubmitError = false) {
  const validation = registerValidation();
  for (const name of ['username', 'password', 'confirm']) {
    const input = registerFields[name];
    const message = validation[name];
    document.querySelector(`#register-${name}-error`).textContent = message;
    input.setAttribute('aria-invalid', String(Boolean(message)));
  }
  document.querySelector('.register-submit').disabled = registerBusy || !validation.complete;
  if (clearSubmitError) document.querySelector('#register-alert').textContent = '';
  return validation;
}

function setRegisterBusy(busy) {
  registerBusy = busy;
  const validation = registerValidation();
  document.querySelector('.register-submit').disabled = busy || !validation.complete;
}

function renderRegisterSubmitError(error) {
  const fieldName = error && error.kind === 'username-invalid' ? 'username' : error && error.kind === 'password-invalid' ? 'password' : '';
  if (fieldName && error.message) {
    document.querySelector(`#register-${fieldName}-error`).textContent = error.message;
    registerFields[fieldName].setAttribute('aria-invalid', 'true');
    return;
  }
  document.querySelector('#register-alert').textContent = registerSubmitErrors[error && error.kind] || 'Something went wrong. Create a ticket on Discord.';
}

function activateRoute(route, focusTab = false) {
  resetGamepadModes();
  activePrimaryRoute = route;
  document.querySelectorAll('[data-route]').forEach(tab => {
    const selected = tab.dataset.route === route;
    tab.setAttribute('aria-selected', String(selected));
    tab.tabIndex = selected ? 0 : -1;
    if (selected && focusTab) tab.focus();
  });
  document.querySelectorAll('[data-screen]').forEach(screen => {
    screen.toggleAttribute('data-active', screen.dataset.screen === route);
  });
  document.querySelector('[data-utility="gamepad"]').setAttribute('aria-pressed', 'false');
  document.querySelector('[data-utility="help"]').setAttribute('aria-pressed', 'false');
  updateGamepadPrompts(navigator.getGamepads ? [...navigator.getGamepads()] : []);
  if (route === 'settings') hydrateSettings();
  if (route === 'extensions') hydrateExtensions();
}

function activateGamepadScreen(focusRegion = false) {
  resetGamepadModes();
  document.querySelectorAll('[data-route]').forEach(tab => {
    tab.setAttribute('aria-selected', 'false');
    tab.tabIndex = tab.dataset.route === activePrimaryRoute ? 0 : -1;
  });
  document.querySelectorAll('[data-screen]').forEach(screen => {
    screen.toggleAttribute('data-active', screen.dataset.screen === 'gamepad');
  });
  document.querySelector('[data-utility="gamepad"]').setAttribute('aria-pressed', 'true');
  document.querySelector('[data-utility="help"]').setAttribute('aria-pressed', 'false');
  updateGamepadPrompts(navigator.getGamepads ? [...navigator.getGamepads()] : []);
  hydrateGamepadPage();
  if (focusRegion) focusAdjacentRegion(1);
}

function activateHelpScreen(focusRegion = false) {
  resetGamepadModes();
  document.querySelectorAll('[data-route]').forEach(tab => {
    tab.setAttribute('aria-selected', 'false');
    tab.tabIndex = tab.dataset.route === activePrimaryRoute ? 0 : -1;
  });
  document.querySelectorAll('[data-screen]').forEach(screen => {
    screen.toggleAttribute('data-active', screen.dataset.screen === 'help');
  });
  document.querySelector('[data-utility="gamepad"]').setAttribute('aria-pressed', 'false');
  document.querySelector('[data-utility="help"]').setAttribute('aria-pressed', 'true');
  updateGamepadPrompts(navigator.getGamepads ? [...navigator.getGamepads()] : []);
  hydrateHelpPage();
  if (focusRegion) focusAdjacentRegion(1);
}

function activateSettingsTab(name, focus = false) {
  document.querySelectorAll('[data-settings-tab]').forEach(tab => {
    const selected = tab.dataset.settingsTab === name;
    tab.setAttribute('aria-selected', String(selected));
    tab.tabIndex = selected ? 0 : -1;
    if (selected && focus) tab.focus();
  });
  document.querySelectorAll('[data-settings-pane]').forEach(pane => pane.toggleAttribute('data-active', pane.dataset.settingsPane === name));
  hydrateSettings();
  if (name === 'misc') {
    hydrateLauncherUpdates();
  }
}
function syncThemeToggle() {
  const night = body.dataset.theme === 'dark';
  document.querySelector('[data-utility="theme"]').setAttribute('aria-label', night ? 'Use day theme' : 'Use night theme');
  document.querySelector('.brand img').src = `assets/brand-logo-${night ? 'night' : 'day'}.png`;
}

async function hydrateGamepadPage() {
  updateControllerLabel(navigator.getGamepads ? [...navigator.getGamepads()] : []);
  const button = document.querySelector('#gamepad-config-tool-button');
  if (!tauriInvoke) {
    button.hidden = true;
    return;
  }
  try {
    button.hidden = !(await invoke('config_tool_supported'));
  } catch {
    button.hidden = true;
  }
}
async function hydrateHelpPage() {
  const log = document.querySelector('#help-log');
  document.querySelector('#help-status').textContent = '';
  log.textContent = 'Loading launcher logs...';
  try {
    const snapshot = await invoke('get_launcher_log');
    log.textContent = normalizeLauncherLog(snapshot.content);
  } catch (error) {
    log.textContent = (error && error.message) || 'Unable to load launcher logs.';
  }
}

async function copyHelpLog() {
  const status = document.querySelector('#help-status');
  status.textContent = '';
  try {
    if (!navigator.clipboard || typeof navigator.clipboard.writeText !== 'function') throw new Error('Clipboard access is unavailable.');
    await navigator.clipboard.writeText(document.querySelector('#help-log').textContent);
    status.textContent = 'Logs copied to clipboard.';
  } catch (error) {
    status.textContent = 'Unable to copy logs.';
    console.error('Unable to copy launcher logs.', error);
  }
}

function activateProfilesScreen(focusRegion = false) {
  resetGamepadModes();
  document.querySelectorAll('[data-route]').forEach(tab => {
    tab.setAttribute('aria-selected', 'false');
    tab.tabIndex = tab.dataset.route === activePrimaryRoute ? 0 : -1;
  });
  document.querySelectorAll('[data-screen]').forEach(screen => {
    screen.toggleAttribute('data-active', screen.dataset.screen === 'profiles');
  });
  document.querySelector('[data-utility="gamepad"]').setAttribute('aria-pressed', 'false');
  document.querySelector('[data-utility="help"]').setAttribute('aria-pressed', 'false');
  updateGamepadPrompts(navigator.getGamepads ? [...navigator.getGamepads()] : []);
  hydrateProfiles();
  if (focusRegion) focusAdjacentRegion(1);
}

async function openHelpLogs() {
  const status = document.querySelector('#help-status');
  status.textContent = '';
  try {
    await invoke('open_launcher_log');
  } catch (error) {
    status.textContent = 'Unable to open logs.';
    console.error('Unable to open launcher logs.', error);
  }
}

async function hydrateProfiles() {
  try {
    renderServerSettings(await invoke('get_server_settings'));
  } catch (error) {
    setProfileFeedback(document.querySelector('#profile-server-select-status'), error.message || String(error), 'error');
  }
}
document.addEventListener('click', event => {
  const route = event.target.closest('[data-route]');
  if (route) activateRoute(route.dataset.route);
  const link = event.target.closest('[data-link]');
  if (link) {
    invoke('open_external', { target:link.dataset.link }).catch(error => { console.error('Unable to open external link.', error); });
  }
  const settingsTab = event.target.closest('[data-settings-tab]');
  if (settingsTab) activateSettingsTab(settingsTab.dataset.settingsTab);
  const settingsAction = event.target.closest('[data-settings-action]');
  if (settingsAction) handleSettingsAction(settingsAction.dataset.settingsAction);
  const gameFilesAction = event.target.closest('[data-game-files-action]');
  if (gameFilesAction?.dataset.gameFilesAction === 'repair') void runRepairInstall();
  const launcherUpdateAction = event.target.closest('[data-launcher-update-action]');
  if (launcherUpdateAction) handleLauncherUpdateAction(event);
  const backupAction = event.target.closest('[data-backup-action]');
  if (backupAction) runBackupAction(backupAction);
  const helpAction = event.target.closest('[data-help-action]');
  if (helpAction?.dataset.helpAction === 'copy') copyHelpLog();
  if (helpAction?.dataset.helpAction === 'open') openHelpLogs();
  const gameSettingChoice = event.target.closest('[data-game-setting-choice]');
  if (gameSettingChoice && settingsState.gameSettings?.available) {
    const path = gameSettingChoice.dataset.gameSettingChoice;
    const value = ['true', 'false'].includes(gameSettingChoice.dataset.gameSettingValue)
      ? gameSettingChoice.dataset.gameSettingValue === 'true'
      : gameSettingChoice.dataset.gameSettingValue;
    queueGameSettingsUpdate(next => setNestedValue(next, path, value));
  }
  const launcherSettingChoice = event.target.closest('[data-launcher-setting-choice]');
  if (launcherSettingChoice) {
    const value = launcherSettingChoice.dataset.launcherSettingValue === 'true';
    if (launcherSettingChoice.dataset.launcherSettingChoice === 'close_on_game_start') {
      queueLauncherBehaviorUpdate(value);
    } else if (launcherSettingChoice.dataset.launcherSettingChoice === 'native_resolution_override') {
      queueNativeResolutionOverrideUpdate(value);
    }
  }
  const gamepadChoice = event.target.closest('[data-gamepad-choice]');
  if (gamepadChoice) {
    localStorage.setItem('bahamut-gamepad-enabled', gamepadChoice.dataset.gamepadChoice);
    renderChoice('[data-gamepad-choice]', gamepadChoice.dataset.gamepadChoice, 'gamepadChoice');
    if (gamepadChoice.dataset.gamepadChoice === 'false') {
      controllerStates.clear();
      resetGamepadModes();
    }
    updateControllerLabel(navigator.getGamepads ? [...navigator.getGamepads()] : []);
  }
  const action = event.target.closest('[data-action]');
  if (!action) return;
  if (action.dataset.action === 'open-register') activateRoute('register');
  if (action.dataset.action === 'close-register') activateRoute('home');
  if (action.dataset.action === 'open-profiles') activateProfilesScreen();
  if (action.dataset.action === 'close-profiles') activateRoute('home');
  if (action.dataset.action === 'open-gamepad') activateGamepadScreen();
  if (action.dataset.action === 'install-folder') chooseInstallFolder();
  if (action.dataset.action === 'logout') {
    const token = home.token;
    const server = home.server;
    const authEndpoint = home.authEndpoint;
    home.token = '';
    home.authEndpoint = '';
    home.expiresAt = 0;
    localStorage.removeItem('bahamut-session');
    invoke('logout', { token, server, authEndpoint }).catch(() => {});
    refreshHomeStatus();
  }
});

document.querySelector('#profile-server-select').addEventListener('change', async event => {
  const status = document.querySelector('#profile-server-select-status');
  setProfileFeedback(status, '');
  try {
    const serverSettings = await invoke('set_selected_server', { displayName:event.target.value });
    renderServerSettings(serverSettings);
    if (!home.token) home.server = serverSettings.selected_server;
  } catch (error) {
    if (settingsState.serverSettings) renderServerSettings(settingsState.serverSettings);
    setProfileFeedback(status, error.message || String(error), 'error');
  }
});

document.querySelectorAll('[data-game-setting]').forEach(control => {
  control.addEventListener('input', event => {
    if (!event.currentTarget.matches('input[type="range"]')) return;
    const path = event.currentTarget.dataset.gameSetting;
    renderGameSettingProgress(path);
  });
  control.addEventListener('change', event => {
    if (!settingsState.gameSettings?.available) return;
    const path = event.currentTarget.dataset.gameSetting;
    const values = event.currentTarget.dataset.gameSettingValues?.split(',');
    const value = values ? values[Number(event.currentTarget.value)] : event.currentTarget.value;
    if (path === 'resolution') {
      const [width, height] = value.split('x').map(Number);
      queueGameSettingsUpdate(next => {
        next.width = width;
        next.height = height;
      });
    } else {
      const numeric = ['graphics.general_quality', 'graphics.background_quality'].includes(path);
      queueGameSettingsUpdate(next => setNestedValue(next, path, numeric ? Number(value) : value));
    }
  });
});
document.querySelectorAll('[data-object-distance-selection],[data-camera-zoom-selection]').forEach(range => {
  range.addEventListener('input', () => renderGraphicsRangePreview(range));
  range.addEventListener('change', () => {
    if (range.disabled) return;
    const selection = graphicsRangeSelection(range);
    if (range.hasAttribute('data-object-distance-selection')) queueObjectDistanceSelection(selection);
    else queueCameraZoomSelection(selection);
  });
});
document.querySelector('#borderless-monitor').addEventListener('change', event => {
  if (event.currentTarget.disabled || !settingsState.borderlessMonitors?.supported) return;
  queueBorderlessMonitorUpdate(event.currentTarget.value || null);
});

document.querySelector('#profile-form').addEventListener('submit', async event => {
  event.preventDefault();
  const status = document.querySelector('#profile-save-status');
  setProfileFeedback(status, '');
  const form = event.currentTarget;
  const profile = {
    display_name:document.querySelector('#profile-name').value.trim(),
    host:document.querySelector('#profile-host').value.trim(),
    auth_port:Number(document.querySelector('#profile-auth-port').value),
    lobby_port:Number(document.querySelector('#profile-lobby-port').value),
    use_https:document.querySelector('#profile-https').checked,
  };
  try {
    const previous = form.dataset.originalDisplayName;
    const serverSettings = await invoke('save_server_profile', { originalDisplayName:previous, profile });
    renderServerSettings(serverSettings);
    if (!home.token && home.server === previous) home.server = serverSettings.selected_server;
    setProfileFeedback(status, 'Server profile saved.', 'success');
  } catch (error) {
    setProfileFeedback(status, error.message || String(error), 'error');
  }
});

document.querySelectorAll('[data-route]').forEach(tab => {
  tab.addEventListener('keydown', event => {
    if (!['ArrowLeft', 'ArrowRight'].includes(event.key)) return;
    event.preventDefault();
    const tabs = [...document.querySelectorAll('[data-route]')];
    const delta = event.key === 'ArrowRight' ? 1 : -1;
    const next = tabs[(tabs.indexOf(tab) + delta + tabs.length) % tabs.length];
    activateRoute(next.dataset.route, true);
  });
});

document.querySelectorAll('[data-settings-tab]').forEach(tab => {
  tab.addEventListener('keydown', event => {
    if (!['ArrowLeft', 'ArrowRight'].includes(event.key)) return;
    event.preventDefault();
    const tabs = [...document.querySelectorAll('[data-settings-tab]')];
    const delta = event.key === 'ArrowRight' ? 1 : -1;
    const next = tabs[(tabs.indexOf(tab) + delta + tabs.length) % tabs.length];
    activateSettingsTab(next.dataset.settingsTab, true);
  });
});

document.querySelector('#home-primary').addEventListener('click', () => {
  if (!home.status) return;
  if (['no-valid-install', 'outdated-install'].includes(home.status.state)) {
    if (home.installDestination || home.status.default_game_dir) startInstall();
    else chooseInstallFolder();
  }
  if (home.status.state === 'ready') launchGame();
});

document.querySelector('#lifecycle-cancel').addEventListener('click', async () => {
  if (repairState.status?.is_running) {
    await controlGameRepair('cancel');
    renderLifecycleStrip();
  } else {
    await invoke('cancel_install').catch(() => {});
  }
});

document.querySelector('#lifecycle-pause').addEventListener('click', async event => {
  const resume = event.currentTarget.dataset.paused === 'true';
  try {
    if (repairState.status?.is_running) {
      await controlGameRepair(resume ? 'resume' : 'pause');
      renderLifecycleStrip();
    } else {
      await invoke(resume ? 'resume_install' : 'pause_install');
      await refreshInstallSnapshot();
    }
  } catch (error) {
    const alert = document.querySelector('#install-home-alert');
    if (alert) alert.textContent = error.message || String(error);
  }
});

document.querySelector('#register-form').addEventListener('submit', async event => {
  event.preventDefault();
  if (registerBusy) return;
  const alert = document.querySelector('#register-alert');
  const username = document.querySelector('#register-username').value.trim();
  const password = document.querySelector('#register-password').value;
  if (!renderRegisterValidation().complete) return;
  alert.textContent = '';
  setRegisterBusy(true);
  try {
    await invoke('register', { username, password, server:selectedServer() });
    home.username = username;
    activateRoute('home');
    renderHome();
  } catch (error) {
    renderRegisterSubmitError(error);
  } finally {
    setRegisterBusy(false);
  }
});

Object.values(registerFields).forEach(input => input.addEventListener('input', () => renderRegisterValidation(true)));
renderRegisterValidation();

document.querySelector('[data-utility="theme"]').addEventListener('click', () => {
  const next = body.dataset.theme === 'dark' ? 'light' : 'dark';
  body.dataset.theme = next;
  localStorage.setItem('bahamut-theme', next);
  syncThemeToggle();
});
document.querySelector('[data-utility="gamepad"]').addEventListener('click', () => {
  activateGamepadScreen();
});
document.querySelector('[data-utility="help"]').addEventListener('click', () => activateHelpScreen());
const extensionSearch = document.querySelector('#extension-search');
extensionSearch.addEventListener('focus', () => body.setAttribute('data-extension-search-focused', ''));
extensionSearch.addEventListener('blur', () => body.removeAttribute('data-extension-search-focused'));
extensionSearch.addEventListener('input', event => {
  extensionState.query = event.target.value;
  renderExtensionLibrary();
});
document.querySelectorAll('[data-extension-folder]').forEach(button => {
  button.addEventListener('click', () => invoke('open_extension_folder', { target:button.dataset.extensionFolder }).catch(error => { console.error('Unable to open extension folder.', error); }));
});
document.querySelector('#settings-confirmation-cancel').addEventListener('click', () => closeSettingsConfirmation(false));
document.querySelector('#settings-confirmation-confirm').addEventListener('click', () => closeSettingsConfirmation(true));
document.querySelector('#settings-confirmation-dialog').addEventListener('cancel', event => {
  event.preventDefault();
  closeSettingsConfirmation(false);
});
const closingDialog = document.querySelector('#launcher-closing-dialog');
closingDialog.addEventListener('keydown', event => {
  if (event.key === 'Escape') event.preventDefault();
});
const tauriListen = window.__TAURI__?.event?.listen;
if (tauriListen) {
  tauriListen('launcher-closing', () => {
    if (!closingDialog.open) closingDialog.showModal();
  }).catch(error => console.error('Unable to listen for launcher closing.', error));
}
document.querySelectorAll('[data-window]').forEach(button => {
  button.addEventListener('click', () => invoke('control_window', { action:button.dataset.window }).catch(error => { console.error('Unable to control the launcher window.', error); }));
});
document.querySelector('[data-window-drag-region]').addEventListener('pointerdown', event => {
  if (event.button !== 0 || event.target.closest('[data-window-no-drag], button, a, input, select, textarea, [role="button"]')) return;
  invoke('control_window', { action:'start-dragging' }).catch(error => { console.error('Unable to drag the launcher window.', error); });
});
const savedTheme = localStorage.getItem('bahamut-theme');
if (savedTheme === 'light' || savedTheme === 'dark') body.dataset.theme = savedTheme;
syncThemeToggle();
renderChoice('[data-gamepad-choice]', localStorage.getItem('bahamut-gamepad-enabled') !== 'false', 'gamepadChoice');
const GAMEPAD_ACTIVE_POLL_MS = 90;
const GAMEPAD_IDLE_POLL_MS = 250;
const GAMEPAD_INITIAL_REPEAT_MS = 320;
const GAMEPAD_REPEAT_MS = 120;
const GAMEPAD_AXIS_PRESS = .55;
const GAMEPAD_AXIS_RELEASE = .35;
const controllerStates = new Map();
let activeControllerIndex = null;
let gamepadMode = null;
let previousGamepadFocus = null;
let gamepadPromptsHiddenAfterPointer = false;
let lastMouseX = null;
let lastMouseY = null;
let suppressKeyboardInputMode = false;

function gamepadEnabled() {
  return localStorage.getItem('bahamut-gamepad-enabled') !== 'false';
}

function elementIsVisible(element) {
  return element.getClientRects().length > 0 && getComputedStyle(element).visibility !== 'hidden';
}

function visibleFocusTargets(root, includeSpecial = false) {
  return [...root.querySelectorAll('button:not(:disabled), input:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])')]
    .filter(element => elementIsVisible(element)
      && !element.hasAttribute('data-gamepad-back')
      && !element.matches('.choice-option')
      && (includeSpecial || !element.closest('[data-gamepad-social]')));
}

function gamepadRegions() {
  const dialog = document.querySelector('dialog[open][data-gamepad-region]');
  if (dialog) return [dialog];
  const screen = document.querySelector('[data-screen][data-active]');
  if (!screen) return [];
  return [...screen.querySelectorAll('[data-gamepad-region]')]
    .filter(elementIsVisible)
    .filter(region => {
      const controls = [
        ...(region.matches('button, input, select, textarea') ? [region] : []),
        ...region.querySelectorAll('button, input, select, textarea'),
      ].filter(elementIsVisible);
      return !controls.length || controls.some(control => !control.disabled && control.getAttribute('aria-disabled') !== 'true');
    })
    .map(region => {
      if (!region.hasAttribute('tabindex')) region.tabIndex = -1;
      return region;
    })
    .sort((left, right) => Number(left.dataset.gamepadRegionOrder || 0) - Number(right.dataset.gamepadRegionOrder || 0));
}

function focusAdjacentRegion(delta) {
  const regions = gamepadRegions();
  if (!regions.length) return;
  const current = regions.findIndex(region => region === document.activeElement || region.contains(document.activeElement));
  const base = current < 0 ? (delta > 0 ? -1 : 0) : current;
  const next = regions[(base + delta + regions.length) % regions.length];
  next.focus({ preventScroll:true });
}

function moveHomeAccountFocus(direction) {
  const current = document.activeElement;
  const create = document.querySelector('[data-action="open-register"]');
  const profiles = document.querySelector('[data-action="open-profiles"]');
  const submit = document.querySelector('#login-submit');
  const loginControls = ['#login-username', '#login-password', '#login-remember', '#login-submit']
    .map(selector => document.querySelector(selector));
  if (loginControls.includes(current) && ['left', 'right'].includes(direction)) return true;
  if (current === submit && direction === 'down') {
    create?.focus();
    return true;
  }
  if (current !== create && current !== profiles) return false;
  if (direction === 'up') submit?.focus();
  else if (direction === 'right' && current === create) profiles?.focus();
  else if (direction === 'left' && current === profiles) create?.focus();
  return true;
}

function focusExtensionRow(button) {
  if (!button) return;
  button.click();
  button.focus({ preventScroll:true });
}

function moveExtensionLibraryFocus(region, direction) {
  if (!region.matches('.extension-library')) return false;
  if (['left', 'right'].includes(direction)) return true;
  if (!['up', 'down'].includes(direction)) return false;
  const search = region.querySelector('.extension-search');
  const rows = [...region.querySelectorAll('.extension-row-select')].filter(elementIsVisible);
  const current = document.activeElement;
  if (current === search) {
    return true;
  }
  const currentButton = current === region
    ? region.querySelector('.extension-row[data-current="true"] .extension-row-select')
    : current.closest?.('.extension-row')?.querySelector('.extension-row-select');
  const index = rows.indexOf(currentButton);
  if (current === region && index < 0) {
    if (!rows.length) return true;
    focusExtensionRow(direction === 'up' ? rows[rows.length - 1] : rows[0]);
    return true;
  }
  if (index < 0) return false;
  focusExtensionRow(rows[Math.max(0, Math.min(rows.length - 1, index + (direction === 'down' ? 1 : -1)))]);
  return true;
}

function toggleFocusedExtension(target) {
  if (!target?.closest?.('[data-screen="extensions"][data-active]')) return false;
  const row = target.closest('.extension-row');
  if (!row) return false;
  const button = row.querySelector('.extension-row-select');
  const checkbox = row.querySelector('input[type="checkbox"]:not(:disabled)');
  if (checkbox) checkbox.click();
  else button?.click();
  button?.focus({ preventScroll:true });
  return true;
}

function moveLinearFocus(targets, direction) {
  if (!targets.length) return;
  const current = targets.indexOf(document.activeElement);
  const delta = ['left', 'up'].includes(direction) ? -1 : 1;
  const base = current < 0 ? (delta > 0 ? -1 : targets.length) : current;
  const next = Math.max(0, Math.min(targets.length - 1, base + delta));
  targets[next].focus();
}

function moveProfilesFocus(region, direction) {
  if (!region?.closest('[data-screen="profiles"]')) return false;
  if (['left', 'right'].includes(direction)) return true;
  if (!['up', 'down'].includes(direction)) return false;
  moveLinearFocus(visibleFocusTargets(region), direction);
  return true;
}

function utilityButtons() {
  return [...document.querySelectorAll('.utility-cluster button:not(:disabled)')].filter(elementIsVisible);
}

function socialButtons() {
  if (!document.querySelector('[data-screen="home"][data-active]')) return [];
  return [...document.querySelectorAll('[data-gamepad-social]')].filter(elementIsVisible);
}

function settingsRows(region) {
  return [...region.querySelectorAll('[data-gamepad-settings-row]')]
    .filter(row => elementIsVisible(row)
      && row.getAttribute('aria-disabled') !== 'true'
      && row.querySelector('button:not(:disabled), select:not(:disabled), input:not(:disabled)'));
}

function focusedSettingsRow(region) {
  return document.activeElement.matches?.('[data-gamepad-settings-row]')
    ? document.activeElement
    : document.activeElement.closest?.('[data-gamepad-settings-row]') || null;
}

function focusSettingsRow(row) {
  row.focus({ preventScroll:true });
}

function miscSettingsRowButtons(row) {
  if (!row?.closest('[data-settings-pane="misc"]')) return [];
  return [...row.querySelectorAll('button:not(:disabled):not(.choice-option)')].filter(elementIsVisible);
}

function stepSettingsRange(row, direction) {
  const range = row?.querySelector('input[type="range"]:not(:disabled)');
  if (!range) return false;
  const minimum = Number(range.min);
  const maximum = Number(range.max);
  const current = Number(range.value);
  const delta = direction === 'left' ? -Number(range.step || 1) : Number(range.step || 1);
  const next = Math.max(minimum, Math.min(maximum, current + delta));
  if (next === current) return true;
  range.value = String(next);
  range.dispatchEvent(new Event('input', { bubbles:true }));
  range.dispatchEvent(new Event('change', { bubbles:true }));
  row.focus();
  return true;
}

function focusByDirection(direction) {
  if (gamepadMode === 'utility') {
    if (['left', 'right'].includes(direction)) moveLinearFocus(utilityButtons(), direction);
    return;
  }
  if (gamepadMode === 'social') {
    if (['left', 'right'].includes(direction)) moveLinearFocus(socialButtons(), direction);
    return;
  }
  const region = document.activeElement.closest && document.activeElement.closest('[data-gamepad-region]');
  if (!region || !elementIsVisible(region)) {
    focusAdjacentRegion(['left', 'up'].includes(direction) ? -1 : 1);
    return;
  }
  if (region.matches('.help-log-window')) {
    if (['up', 'down'].includes(direction)) {
      const step = Math.max(72, Math.round(region.clientHeight * .55));
      const limit = Math.max(0, region.scrollHeight - region.clientHeight);
      region.scrollTop = Math.max(0, Math.min(limit, region.scrollTop + (direction === 'down' ? step : -step)));
    }
    return;
  }
  if (moveHomeAccountFocus(direction)) return;
  if (moveExtensionLibraryFocus(region, direction)) return;
  if (moveProfilesFocus(region, direction)) return;
  const activeRow = focusedSettingsRow(region);
  if (activeRow && ['left', 'right'].includes(direction)) {
    if (stepSettingsRange(activeRow, direction)) return;
    const miscTargets = miscSettingsRowButtons(activeRow);
    if (miscTargets.length) {
      const current = miscTargets.indexOf(document.activeElement);
      const next = current < 0
        ? (direction === 'left' ? 0 : miscTargets.length - 1)
        : Math.max(0, Math.min(miscTargets.length - 1, current + (direction === 'right' ? 1 : -1)));
      miscTargets[next]?.focus();
      return;
    }
    if (activeRow.querySelector('.choice-row')) return;
  }
  const rows = settingsRows(region);
  const row = activeRow;
  if (['up', 'down'].includes(direction) && rows.length) {
    const current = row ? rows.indexOf(row) : -1;
    const next = current < 0
      ? (direction === 'down' ? 0 : rows.length - 1)
      : Math.max(0, Math.min(rows.length - 1, current + (direction === 'down' ? 1 : -1)));
    focusSettingsRow(rows[next]);
    return;
  }
  const targets = visibleFocusTargets(region);
  if (!targets.length) return;
  const current = targets.includes(document.activeElement) ? document.activeElement : null;
  if (!current) {
    targets[['left', 'up'].includes(direction) ? targets.length - 1 : 0].focus();
    return;
  }
  const source = current.getBoundingClientRect();
  const sx = source.left + source.width / 2;
  const sy = source.top + source.height / 2;
  const candidates = targets.filter(target => target !== current).map(target => {
    const rect = target.getBoundingClientRect();
    const dx = rect.left + rect.width / 2 - sx;
    const dy = rect.top + rect.height / 2 - sy;
    const valid = direction === 'left' ? dx < -8 : direction === 'right' ? dx > 8 : direction === 'up' ? dy < -8 : dy > 8;
    const primary = ['left', 'right'].includes(direction) ? Math.abs(dx) : Math.abs(dy);
    const cross = ['left', 'right'].includes(direction) ? Math.abs(dy) : Math.abs(dx);
    return { target, valid, primary, cross };
  }).filter(candidate => candidate.valid).sort((left, right) => left.primary - right.primary || left.cross - right.cross);
  if (candidates[0]) candidates[0].target.focus();
}

function activateFocused() {
  const target = document.activeElement;
  if (!target || target === body) {
    focusAdjacentRegion(1);
    return;
  }
  if (toggleFocusedExtension(target)) return;
  if (target.matches('[data-gamepad-region]')) {
    if (target.matches('button:not(:disabled), a[href]')) {
      target.click();
      return;
    }
    const row = settingsRows(target)[0];
    if (row) row.focus();
    else visibleFocusTargets(target)[0]?.focus();
    return;
  }
  if (target.matches('[data-gamepad-settings-row]')) {
    const controls = miscSettingsRowButtons(target);
    if (controls.length > 1) {
      const preferred = controls.find(control => control.getAttribute('aria-pressed') === 'true') || controls[0];
      preferred?.focus();
      return;
    }
    if (!controls.length) {
      const fallbackControls = visibleFocusTargets(target);
      if (fallbackControls.length === 1 && fallbackControls[0].matches('button, input[type="checkbox"], input[type="radio"]')) fallbackControls[0].click();
      else fallbackControls[0]?.focus();
      return;
    }
    if (controls.length === 1 && controls[0].matches('button, input[type="checkbox"], input[type="radio"]')) {
      controls[0].click();
      focusSettingsRow(target);
    }
    else controls[0]?.focus();
    return;
  }
  const settingsRow = target.closest?.('[data-gamepad-settings-row]');
  if (settingsRow && target.matches('button')) {
    target.click();
    focusSettingsRow(settingsRow);
    return;
  }
  if (target instanceof HTMLSelectElement && target.options.length) {
    target.selectedIndex = (target.selectedIndex + 1) % target.options.length;
    target.dispatchEvent(new Event('change', { bubbles:true }));
    return;
  }
  target.click();
}

function runSecondaryAction() {
  const target = document.activeElement;
  if (!target) return;
  if (toggleFocusedExtension(target)) return;
  const row = target.closest?.('[data-gamepad-settings-row]') || null;
  if (!row && target.matches('input:not([type="checkbox"]), textarea')) return;
  const select = row?.querySelector('select:not(:disabled)') || (target instanceof HTMLSelectElement ? target : null);
  if (select?.options.length) {
    select.selectedIndex = (select.selectedIndex + 1) % select.options.length;
    select.dispatchEvent(new Event('change', { bubbles:true }));
    row?.focus();
    return;
  }
  if (row && stepSettingsRange(row, 'right')) return;
  if (target.matches('input[type="checkbox"]')) {
    target.click();
    return;
  }
  const choice = row?.querySelector('.choice-row') || target.closest('.choice-row');
  if (!choice) return;
  const options = [...choice.querySelectorAll('button:not(:disabled)')];
  if (!options.length) return;
  const selected = options.findIndex(option => option.getAttribute('aria-pressed') === 'true');
  const next = options[(selected + 1 + options.length) % options.length];
  next.click();
  if (row) row.focus();
  else next.focus();
}

function resetGamepadModes(restoreFocus = false) {
  const target = previousGamepadFocus;
  gamepadMode = null;
  previousGamepadFocus = null;
  if (restoreFocus || document.activeElement !== extensionSearch) body.removeAttribute('data-extension-search-focused');
  if (restoreFocus && target && target.isConnected && elementIsVisible(target)) target.focus();
}

function toggleUtilityMode() {
  if (gamepadMode === 'utility') {
    resetGamepadModes(true);
    return;
  }
  previousGamepadFocus = document.activeElement === body ? null : document.activeElement;
  gamepadMode = 'utility';
  utilityButtons()[0]?.focus();
}

function toggleSocialMode() {
  if (document.querySelector('[data-screen="extensions"][data-active]')) {
    toggleExtensionSearchMode();
    return;
  }
  const targets = socialButtons();
  if (!targets.length) return;
  if (gamepadMode === 'social') {
    resetGamepadModes(true);
    return;
  }
  previousGamepadFocus = document.activeElement === body ? null : document.activeElement;
  gamepadMode = 'social';
  targets[0].focus();
}

function toggleExtensionSearchMode() {
  const screen = document.querySelector('[data-screen="extensions"][data-active]');
  const search = screen?.querySelector('.extension-search');
  if (!search || !elementIsVisible(search)) return;
  if (gamepadMode === 'search') {
    resetGamepadModes(true);
    return;
  }
  const region = search.closest('.extension-library');
  const rows = [...region.querySelectorAll('.extension-row-select')].filter(elementIsVisible);
  const focusedRow = document.activeElement.closest?.('.extension-row')?.querySelector('.extension-row-select');
  previousGamepadFocus = focusedRow && elementIsVisible(focusedRow)
    ? focusedRow
    : rows.find(button => button.getAttribute('aria-current') === 'true') || rows[0] || region;
  gamepadMode = 'search';
  body.setAttribute('data-extension-search-focused', '');
  search.focus({ preventScroll:true });
}

function cancelGamepadAction() {
  if (document.querySelector('#settings-confirmation-dialog')?.open) {
    closeSettingsConfirmation(false);
    return;
  }
  if (gamepadMode) {
    resetGamepadModes(true);
    return;
  }
  const screen = document.querySelector('[data-screen][data-active]');
  const backControl = [...document.querySelectorAll('[data-gamepad-back]')]
    .find(element => elementIsVisible(element));
  if (backControl) {
    backControl.click();
    focusAdjacentRegion(1);
    return;
  }
  if (screen?.dataset.gamepadBackRoute) {
    activateRoute(screen.dataset.gamepadBackRoute);
    focusAdjacentRegion(1);
    return;
  }
  suppressKeyboardInputMode = true;
  try {
    document.dispatchEvent(new KeyboardEvent('keydown', { key:'Escape', bubbles:true, cancelable:true }));
    document.dispatchEvent(new KeyboardEvent('keyup', { key:'Escape', bubbles:true, cancelable:true }));
  } finally {
    suppressKeyboardInputMode = false;
  }
}

function switchRouteBy(delta) {
  const tabs = [...document.querySelectorAll('[data-route]')];
  const current = Math.max(0, tabs.findIndex(tab => tab.dataset.route === activePrimaryRoute));
  const next = tabs[(current + delta + tabs.length) % tabs.length];
  activateRoute(next.dataset.route);
  focusAdjacentRegion(1);
}

function cycleSettingsTab() {
  if (!document.querySelector('[data-screen="settings"][data-active]')) return;
  const tabs = [...document.querySelectorAll('[data-settings-tab]:not(:disabled)')];
  const current = Math.max(0, tabs.findIndex(tab => tab.getAttribute('aria-selected') === 'true'));
  activateSettingsTab(tabs[(current + 1) % tabs.length].dataset.settingsTab);
  focusAdjacentRegion(1);
}

function latchedAxisDirection(value, previous) {
  if (previous < 0 && value < -GAMEPAD_AXIS_RELEASE) return -1;
  if (previous > 0 && value > GAMEPAD_AXIS_RELEASE) return 1;
  if (value <= -GAMEPAD_AXIS_PRESS) return -1;
  if (value >= GAMEPAD_AXIS_PRESS) return 1;
  return 0;
}

function buttonPressed(button) {
  return Boolean(button && (button.pressed || button.value > .5));
}

function readControllerActions(gamepad, previous, now = Date.now()) {
  const priorHorizontal = previous ? previous.horizontal : 0;
  const priorVertical = previous ? previous.vertical : 0;
  const horizontal = latchedAxisDirection(gamepad.axes[0] || 0, priorHorizontal);
  const vertical = latchedAxisDirection(gamepad.axes[1] || 0, priorVertical);
  const dpad = {
    up:buttonPressed(gamepad.buttons[12]), down:buttonPressed(gamepad.buttons[13]),
    left:buttonPressed(gamepad.buttons[14]), right:buttonPressed(gamepad.buttons[15]),
  };
  const hasDpadDirection = Object.values(dpad).some(Boolean);
  const stick = {
    up:vertical < 0, down:vertical > 0,
    left:horizontal < 0, right:horizontal > 0,
  };
  const controls = {
    confirm:buttonPressed(gamepad.buttons[0]),
    cancel:buttonPressed(gamepad.buttons[1]),
    secondary:buttonPressed(gamepad.buttons[2]),
    'cycle-settings':buttonPressed(gamepad.buttons[3]),
    'previous-region':buttonPressed(gamepad.buttons[4]),
    'next-region':buttonPressed(gamepad.buttons[5]),
    'previous-route':buttonPressed(gamepad.buttons[6]),
    'next-route':buttonPressed(gamepad.buttons[7]),
    'social-mode':buttonPressed(gamepad.buttons[8]),
    'utility-mode':buttonPressed(gamepad.buttons[9]),
  };
  const repeats = previous ? { ...previous.repeats } : {};
  const actions = [];
  const readControl = (control, action, pressed, repeatable, runAllowed = true) => {
    const prior = repeats[control] || { pressed:false, nextRepeatAt:0 };
    if (!pressed) {
      repeats[control] = { pressed:false, nextRepeatAt:0 };
    } else if (!prior.pressed) {
      repeats[control] = { pressed:true, nextRepeatAt:now + GAMEPAD_INITIAL_REPEAT_MS };
      if (runAllowed) actions.push(action);
    } else if (repeatable && now >= prior.nextRepeatAt) {
      repeats[control] = { pressed:true, nextRepeatAt:now + GAMEPAD_REPEAT_MS };
      if (runAllowed) actions.push(action);
    }
  };
  for (const direction of ['up', 'down', 'left', 'right']) {
    readControl(`dpad:${direction}`, direction, dpad[direction], true);
  }
  for (const direction of ['up', 'down', 'left', 'right']) {
    readControl(`stick:${direction}`, direction, stick[direction], true, !hasDpadDirection);
  }
  for (const [action, pressed] of Object.entries(controls)) {
    readControl(action, action, pressed, false);
  }
  controllerStates.set(gamepad.index, { horizontal, vertical, repeats });
  return actions;
}

function runControllerAction(action) {
  if (closingDialog.open) return;
  body.dataset.inputMode = 'gamepad';
  gamepadPromptsHiddenAfterPointer = false;
  updateGamepadPrompts(navigator.getGamepads ? [...navigator.getGamepads()] : []);
  const directional = ['up', 'down', 'left', 'right'].includes(action);
  if (document.querySelector('dialog[open][data-gamepad-region]')) {
    if (directional) focusByDirection(action);
    else if (action === 'confirm') activateFocused();
    else if (action === 'cancel') cancelGamepadAction();
    return;
  }
  if (gamepadMode === 'utility') {
    if (directional) focusByDirection(action);
    else if (action === 'confirm') activateFocused();
    else if (action === 'cancel' || action === 'utility-mode') cancelGamepadAction();
    return;
  }
  if (gamepadMode === 'social') {
    if (directional) focusByDirection(action);
    else if (action === 'confirm') activateFocused();
    else if (action === 'cancel' || action === 'social-mode') cancelGamepadAction();
    else if (action === 'utility-mode') {
      gamepadMode = null;
      toggleUtilityMode();
    }
    return;
  }
  if (gamepadMode === 'search') {
    if (directional) return;
    if (action === 'confirm') activateFocused();
    else if (action === 'cancel' || action === 'social-mode') toggleExtensionSearchMode();
    else if (action === 'utility-mode') {
      resetGamepadModes(true);
      toggleUtilityMode();
    }
    return;
  }
  if (directional) focusByDirection(action);
  else if (action === 'confirm') activateFocused();
  else if (action === 'secondary') runSecondaryAction();
  else if (action === 'cancel') cancelGamepadAction();
  else if (action === 'previous-route') switchRouteBy(-1);
  else if (action === 'next-route') switchRouteBy(1);
  else if (action === 'previous-region') focusAdjacentRegion(-1);
  else if (action === 'next-region') focusAdjacentRegion(1);
  else if (action === 'cycle-settings') cycleSettingsTab();
  else if (action === 'utility-mode') toggleUtilityMode();
  else if (action === 'social-mode') toggleSocialMode();
}

function controllerFamily(gamepad) {
  const id = String(gamepad?.id || '').toLowerCase();
  if (id.includes('playstation') || id.includes('dualshock') || id.includes('dualsense') || id.includes('sony')) return 'PlayStation';
  if (id.includes('xbox') || id.includes('xinput')) return 'Xbox / XInput';
  return 'Gamepad';
}

function updateGamepadPrompts(gamepads) {
  const bar = document.querySelector('#gamepad-prompts');
  const active = selectActiveGamepad(gamepads, activeControllerIndex);
  const visible = Boolean(active && gamepadEnabled() && !gamepadPromptsHiddenAfterPointer);
  bar.hidden = !visible;
  launcherShell.toggleAttribute('data-gamepad-bar-visible', visible);
  const screen = document.querySelector('[data-screen][data-active]')?.dataset.screen || '';
  launcherShell.dataset.gamepadScreen = screen;
  const contextualPrompt = document.querySelector('[data-gamepad-prompt-scope="contextual"]');
  contextualPrompt.hidden = !['home', 'extensions'].includes(screen);
  contextualPrompt.querySelector('.gamepad-prompt-bar__label').textContent = screen === 'extensions' ? 'Search' : 'Social';
  if (!active) return;

  const playstation = controllerFamily(active) === 'PlayStation';
  const faceAssets = playstation
    ? { confirm:'cross.png', secondary:'square.png', settings:'triangle.png', back:'circle.png' }
    : { confirm:'a.png', secondary:'x.png', settings:'y.png', back:'b.png' };
  Object.entries(faceAssets).forEach(([name, file]) => {
    document.querySelector(`[data-gamepad-face="${name}"]`).src = `assets/gamepad/${playstation ? 'playstation' : 'xbox'}/${file}`;
  });
  const badges = playstation
    ? { 'previous-region':'L1', 'next-region':'R1', 'previous-route':'L2', 'next-route':'R2', utility:'Options', social:'Share' }
    : { 'previous-region':'LB', 'next-region':'RB', 'previous-route':'LT', 'next-route':'RT', utility:String.fromCodePoint(0x2630), social:String.fromCodePoint(0x29c9) };
  Object.entries(badges).forEach(([name, label]) => {
    document.querySelector(`[data-gamepad-badge="${name}"]`).textContent = label;
  });
}

function selectActiveGamepad(gamepads, currentIndex) {
  return gamepads.find(gamepad => gamepad && gamepad.index === currentIndex) || gamepads.find(Boolean) || null;
}

function updateControllerLabel(gamepads) {
  const label = document.querySelector('#active-controller');
  if (!label) return;
  const connected = gamepads.filter(Boolean);
  const active = selectActiveGamepad(connected, activeControllerIndex);
  updateGamepadPrompts(connected);
  if (!active) label.textContent = 'No gamepad detected.';
  else label.textContent = `Gamepad detected: ${controllerFamily(active)}`;
}

function pollGamepads() {
  const gamepads = navigator.getGamepads ? [...navigator.getGamepads()] : [];
  const connectedIndexes = new Set(gamepads.filter(Boolean).map(gamepad => gamepad.index));
  for (const index of controllerStates.keys()) {
    if (!connectedIndexes.has(index)) controllerStates.delete(index);
  }
  const activeGamepad = selectActiveGamepad(gamepads, activeControllerIndex);
  activeControllerIndex = activeGamepad ? activeGamepad.index : null;
  if (!activeGamepad) resetGamepadModes();
  if (activeGamepad && gamepadEnabled()) {
    const actions = readControllerActions(activeGamepad, controllerStates.get(activeGamepad.index));
    actions.forEach(runControllerAction);
  }
  updateControllerLabel(gamepads);
  setTimeout(pollGamepads, activeGamepad && gamepadEnabled() ? GAMEPAD_ACTIVE_POLL_MS : GAMEPAD_IDLE_POLL_MS);
}

function usePointerInput() {
  body.dataset.inputMode = 'pointer';
  gamepadPromptsHiddenAfterPointer = true;
  resetGamepadModes();
  updateGamepadPrompts(navigator.getGamepads ? [...navigator.getGamepads()] : []);
}

function trackMouseMovement(event) {
  const first = lastMouseX == null;
  const moved = lastMouseX !== event.screenX || lastMouseY !== event.screenY;
  lastMouseX = event.screenX;
  lastMouseY = event.screenY;
  if (moved && !first) usePointerInput();
}

window.addEventListener('mousemove', trackMouseMovement, { passive:true });
window.addEventListener('mousedown', usePointerInput, { passive:true });
window.addEventListener('wheel', usePointerInput, { passive:true });
window.addEventListener('touchstart', usePointerInput, { passive:true });
window.addEventListener('keydown', () => {
  if (suppressKeyboardInputMode) return;
  body.dataset.inputMode = 'keyboard';
  if (gamepadMode !== 'search' || document.activeElement !== extensionSearch) resetGamepadModes();
}, { passive:true });
window.addEventListener('gamepadconnected', event => {
  if (activeControllerIndex == null) activeControllerIndex = event.gamepad.index;
  updateControllerLabel(navigator.getGamepads ? [...navigator.getGamepads()] : []);
});
window.addEventListener('gamepaddisconnected', event => {
  controllerStates.delete(event.gamepad.index);
  const gamepads = navigator.getGamepads ? [...navigator.getGamepads()] : [];
  if (activeControllerIndex === event.gamepad.index) activeControllerIndex = selectActiveGamepad(gamepads, null)?.index ?? null;
  resetGamepadModes();
  updateControllerLabel(gamepads);
});

async function boot() {
  if (!tauriInvoke) {
    console.error('Launcher backend is unavailable.');
    return;
  }
  startBackgroundLauncherUpdateCheck();
  try {
    await invoke('fit_window_to_work_area', { availableWidth:window.screen.availWidth, availableHeight:window.screen.availHeight });
  } catch (error) {
    console.error('Unable to fit the launcher window.', error);
  }
  try {
    const [serverSettings, news, version] = await Promise.all([invoke('get_server_settings'), invoke('list_news'), invoke('launcher_version')]);
    settingsState.serverSettings = serverSettings;
    home.server = serverSettings.selected_server;
    home.news = news;
    document.querySelector('#launcher-version').textContent = `v${version}`;
    renderNews();
    await restoreSession();
    await refreshHomeStatus();
  } catch (error) {
    console.error('Unable to initialize the launcher.', error);
  }
}

setInterval(() => {
  if (home.status) {
    refreshInstallSnapshot();
    void refreshGameRepairStatus().then(renderLifecycleStrip);
  }
  if (home.status && home.status.state === 'ready' && home.gameRunning) refreshGameStatus();
  if (document.querySelector('[data-settings-pane="misc"]')?.hasAttribute('data-active')) {
    refreshLauncherUpdateAvailability();
  }
}, 1000);
pollGamepads();
boot();

export { activateSettingsTab, focusAdjacentRegion, readControllerActions, runControllerAction, selectActiveGamepad, trackMouseMovement, updateControllerLabel, updateGamepadPrompts, controllerStates, lastMouseX, lastMouseY };
