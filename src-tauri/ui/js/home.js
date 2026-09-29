import { home, settingsState, AUTH_ERROR_KINDS, invoke, escapeHtml, selectedServer, formatBytes } from './runtime.js';
import { hydrateSettings } from './settings.js';
import { repairState } from './game-repair.js';

function renderNews() {
  const list = document.querySelector('#news-list');
  if (!home.news.length) {
    list.innerHTML = '<article class="news-entry" role="listitem" tabindex="0"><div class="news-media" aria-hidden="true">B</div><div class="news-entry-content"><div class="news-date">Local feed</div><div class="news-title">All caught up</div><p class="news-copy">No new posts yet.</p></div></article>';
    return;
  }
  list.innerHTML = home.news.map(item => `
        <article class="news-entry" role="listitem" tabindex="0">
          <div class="news-media" aria-hidden="true"><img src="assets/news-placeholder.jpg" alt=""></div>
          <div class="news-entry-content">
            <div class="news-date">${escapeHtml(item.date)}</div>
            <div class="news-title">${escapeHtml(item.title)}</div>
            <p class="news-copy">${escapeHtml(item.body)}</p>
          </div>
        </article>`).join('');
}

const loginFormTemplate = document.querySelector('#login-form').cloneNode(true);

function loginContent(loginEnabled = true) {
  const form = loginFormTemplate.cloneNode(true);
  form.removeAttribute('inert');
  form.querySelector('#login-username').value = home.username;
  form.querySelector('#login-submit').disabled = !loginEnabled;
  return form;
}

function renderHome() {
  if (!home.status) return;
  const state = home.status.state;
  const operationActive = Boolean(home.installSnapshot?.is_running || repairState.status?.is_running);
  if (state !== 'logged-out') clearRetryCountdown();
  document.querySelector('#home-layout').dataset.lifecycle = state;
  document.querySelector('#home-layout').dataset.operationActive = String(operationActive);
  document.querySelector('#home-eyebrow').textContent = home.status.eyebrow;
  document.querySelector('#home-title').textContent = home.status.title;
  const content = document.querySelector('#home-session-content');
  const primary = document.querySelector('#home-primary');
  primary.textContent = state === 'no-valid-install' || state === 'outdated-install' ? 'Install' : home.status.primary_action;
  primary.disabled = operationActive || home.installStartPending || state === 'logged-out' || (state === 'ready' && home.gameRunning);

  if (['no-valid-install', 'outdated-install', 'logged-out'].includes(state)) {
    content.replaceChildren(loginContent(state === 'logged-out'));
  }
  if (state === 'logged-out') {
    const form = document.querySelector('#login-form');
    form.addEventListener('submit', submitLogin);
  } else if (state === 'ready') {
    content.innerHTML = `
          <div class="account-ready">
            <div><div class="account-ready-kicker">Welcome back!</div><div class="account-ready-name">${escapeHtml(home.username)}</div></div>
          </div>
          <div class="inline-alert" id="launch-alert" aria-live="polite"></div>
          <div class="account-ready-footer"><button class="login-action account-logout" type="button" data-action="logout">Logout</button></div>`;
  }

  renderLifecycleStrip();
}

function renderLifecycleStrip() {
  if (!home.status) return;
  const state = home.status.state;
  const install = home.installSnapshot || home.installTerminal;
  const installTerminal = install && install.is_terminal && install.phase !== 'done';
  const installRunning = install && install.is_running;
  const repair = repairState.status;
  const repairRunning = Boolean(repair?.is_running);
  const repairTerminal = Boolean(repair?.is_terminal && !repairRunning);
  const installIdle = (state === 'no-valid-install' || state === 'outdated-install') && !installTerminal && !installRunning;
  const installRecovery = Boolean(installTerminal);
  const strip = document.querySelector('#lifecycle-strip');
  const location = document.querySelector('#lifecycle-location');
  const status = document.querySelector('#lifecycle-status');
  const locationTitle = document.querySelector('#lifecycle-location-title');
  const locationHelp = document.querySelector('#lifecycle-location-help');
  const locationPath = document.querySelector('#lifecycle-location-path');
  const pathAction = document.querySelector('#lifecycle-path-action');
  const title = document.querySelector('#lifecycle-title');
  const copy = document.querySelector('#lifecycle-copy');
  const metric = document.querySelector('#lifecycle-metric');
  const progress = document.querySelector('#lifecycle-progress');
  const fill = document.querySelector('#lifecycle-progress-fill');
  const progressText = document.querySelector('#lifecycle-progress-text');
  const installActions = document.querySelector('#lifecycle-actions');
  const pauseAction = document.querySelector('#lifecycle-pause');
  const cancelAction = document.querySelector('#lifecycle-cancel');
  const primary = document.querySelector('#home-primary');
  const operationActive = Boolean(installRunning || repairRunning);
  let percent = 0;
  document.querySelector('#home-layout').dataset.operationActive = String(operationActive);
  document.querySelector('#home-layout').dataset.repairResult = String(repairTerminal);
  document.querySelectorAll('[data-settings-action="browse-game"]').forEach(button => { button.disabled = operationActive; });
  progress.hidden = true;
  installActions.hidden = true;
  pauseAction.hidden = true;
  cancelAction.hidden = true;
  metric.textContent = '';
  const showLocation = !repairRunning && !repairTerminal && (installIdle || installRecovery);
  location.hidden = !showLocation;
  status.hidden = showLocation;
  strip.dataset.mode = showLocation ? 'location' : 'status';
  strip.dataset.recovery = String(Boolean(installRecovery));

  if (repairRunning) {
    strip.dataset.mode = 'progress';
    title.textContent = 'Repair Install';
    if (repair.is_paused) copy.textContent = 'Repair paused.';
    else if (repair.pause_requested) copy.textContent = 'Pausing after the current step.';
    else if (repair.cancel_requested) copy.textContent = 'Stopping after the current step.';
    else {
      copy.textContent = ({
        starting:'Preparing game repair.',
        recovering:'Recovering interrupted repair.',
        verifying:'Checking managed game files.',
        downloading:'Downloading verified game files.',
        staging:'Preparing replacement files.',
        publishing:'Replacing damaged game files.',
        'final-verification':'Checking repaired files.',
      })[repair.phase] || 'Repairing managed game files.';
    }
    const completedBytes = repair.bytes_completed || 0;
    const totalBytes = repair.bytes_total || 0;
    const completedFiles = repair.files_completed || 0;
    const totalFiles = repair.files_total || 0;
    if (totalBytes && repair.phase === 'downloading') {
      percent = completedBytes / totalBytes * 100;
      metric.textContent = `${formatBytes(completedBytes)} / ${formatBytes(totalBytes)}`;
    } else if (totalFiles) {
      percent = completedFiles / totalFiles * 100;
      metric.textContent = `File ${Math.min(completedFiles + 1, totalFiles)} of ${totalFiles}`;
    }
    progress.hidden = !totalBytes && !totalFiles;
    installActions.hidden = false;
    pauseAction.hidden = false;
    cancelAction.hidden = repair.can_cancel === false;
    pauseAction.disabled = repair.can_cancel === false;
    pauseAction.textContent = repair.pause_requested || repair.is_paused ? 'Resume' : 'Pause';
    pauseAction.dataset.paused = String(Boolean(repair.pause_requested || repair.is_paused));
    primary.disabled = true;
  } else if (repairTerminal) {
    pauseAction.disabled = false;
    const failed = !['done', 'complete'].includes(repair.phase);
    strip.dataset.mode = failed ? 'error' : 'status';
    title.textContent = failed
      ? repair.phase === 'cancelled' ? 'Repair Cancelled' : 'Repair Failed'
      : 'Repair Complete';
    copy.textContent = repair.phase === 'cancelled'
      ? 'The repair was cancelled.'
      : failed ? repair.error || 'The repair stopped before completion.' : 'Managed game files are ready.';
  } else if (installTerminal) {
    strip.dataset.mode = 'location';
    const cancelled = install.phase === 'cancelled';
    locationTitle.textContent = cancelled ? 'Install Cancelled' : 'Install Failed';
    locationHelp.textContent = home.installStartPending
        ? 'Checking required space and starting the installation...'
        : home.installError || (cancelled
          ? 'The installation was cancelled. Choose a different folder or retry this destination.'
          : install.error || 'The installation stopped with an error. Choose a different folder or retry this destination.');
    const installHint = installTarget() || 'Choose a folder for the new game install';
    locationPath.textContent = installHint;
    locationPath.title = installHint;
    pathAction.textContent = 'PATH';
    pathAction.dataset.action = 'install-folder';
    primary.textContent = 'Retry Install';
    primary.disabled = home.installStartPending;
  } else if (installRunning) {
    strip.dataset.mode = 'progress';
    const completed = (install.previous_completed_bytes || 0) + (install.bytes_downloaded || 0);
    const total = install.total_download_bytes || 0;
    const itemTotal = install.total_files || 0;
    const itemNumber = Math.min((install.file_idx || 0) + 1, Math.max(1, itemTotal));
    title.textContent = 'Game Installation';
    if (install.is_paused) copy.textContent = 'Installation paused.';
    else if (install.pause_requested) copy.textContent = 'Pausing after the current step.';
    else if (install.phase === 'starting') copy.textContent = 'Preparing the game installation.';
    else if (install.phase === 'downloading') copy.textContent = 'Downloading game files.';
    else if (install.phase === 'validating-files') copy.textContent = 'Checking the installed client before finishing.';
    else if (install.phase === 'installing') copy.textContent = 'Installing game files.';
    else copy.textContent = 'Installing the game.';
    if (install.phase === 'downloading') {
      percent = total ? completed / total * 100 : 0;
      metric.textContent = total ? `${formatBytes(completed)} / ${formatBytes(total)}` : '';
    } else if (['installing', 'validating-files'].includes(install.phase) && itemTotal) {
      percent = itemTotal ? (install.file_idx || 0) / itemTotal * 100 : 0;
      metric.textContent = `File ${itemNumber} of ${itemTotal}`;
    } else {
      metric.textContent = '';
    }
    progress.hidden = false;
    installActions.hidden = false;
    pauseAction.hidden = false;
    cancelAction.hidden = false;
    pauseAction.textContent = install.pause_requested || install.is_paused ? 'Resume' : 'Pause';
    pauseAction.dataset.paused = String(Boolean(install.pause_requested || install.is_paused));
    primary.textContent = 'Installing...';
    primary.disabled = true;
  } else if (installIdle) {
    const installHint = installTarget() || 'Choose a folder for the new game install';
    locationTitle.textContent = 'Install Game';
    const idleHelp = state === 'outdated-install'
      ? 'The selected game folder is not the final 1.23b client. Install the game into a new folder.'
      : 'Install the client. Change the destination with PATH.';
    locationHelp.textContent = home.installStartPending ? 'Checking required space and starting the installation...' : (home.installError || idleHelp);
    locationPath.textContent = installHint;
    locationPath.title = installHint;
    pathAction.textContent = 'PATH';
    pathAction.dataset.action = 'install-folder';
    primary.textContent = 'Install';
    primary.disabled = home.installStartPending;
  } else if (state === 'logged-out') {
    title.textContent = 'Account session required';
    copy.textContent = 'Sign in against the selected server profile to enable Play.';
  } else if (state === 'ready') {
    title.textContent = 'Ready to play';
    copy.textContent = `${home.server || 'Selected server'} - client version 1.23b`;
  }
  fill.style.width = `${Math.max(0, Math.min(100, percent))}%`;
  progressText.textContent = `${Math.round(percent)}%`;
}

function rememberSession(remember) {
  if (!remember) {
    localStorage.removeItem('bahamut-session');
    return;
  }
  localStorage.setItem('bahamut-session', JSON.stringify({ username:home.username, token:home.token, authEndpoint:home.authEndpoint, expiresAt:home.expiresAt, server:home.server }));
}

function forgetSession() {
  localStorage.removeItem('bahamut-session');
  home.username = '';
  home.token = '';
  home.authEndpoint = '';
  home.expiresAt = 0;
  home.server = settingsState.serverSettings?.selected_server || '';
}

function clearRetryCountdown() {
  if (home.retryTimer) clearInterval(home.retryTimer);
  home.retryTimer = null;
}

async function submitLogin(event) {
  event.preventDefault();
  const alert = document.querySelector('#login-alert');
  const primary = document.querySelector('#login-submit');
  alert.textContent = '';
  primary.disabled = true;
  primary.textContent = 'Logging In...';
  try {
    const username = document.querySelector('#login-username').value.trim();
    const password = document.querySelector('#login-password').value;
    const server = selectedServer();
    const response = await invoke('login', { username, password, server });
    if (selectedServer() !== server) {
      throw { kind:'session-endpoint-changed', message:'The selected server changed during login. Log in again.' };
    }
    home.username = username;
    home.token = response.token;
    home.authEndpoint = response.authEndpoint;
    home.expiresAt = response.expiresAt;
    home.server = server;
    clearRetryCountdown();
    rememberSession(document.querySelector('#login-remember').checked);
    await refreshHomeStatus();
  } catch (error) {
    showLoginError(error, alert);
    if (!error || error.kind !== AUTH_ERROR_KINDS.rateLimited) primary.disabled = false;
  } finally {
    primary.textContent = 'Login';
  }
}
function showLoginError(error, alert) {
  const kind = error && error.kind;
  if (kind === AUTH_ERROR_KINDS.invalidCredentials) {
    alert.textContent = 'Incorrect username or password.';
  } else if (kind === AUTH_ERROR_KINDS.rateLimited) {
    startRetryCountdown(alert, Number(error.retryAfter || 0));
  } else if (kind === AUTH_ERROR_KINDS.network) {
    alert.textContent = 'The selected server could not be reached.';
  } else if (kind === AUTH_ERROR_KINDS.noInstall) {
    alert.textContent = 'Select a valid game install before logging in.';
  } else if (kind === AUTH_ERROR_KINDS.outdatedClient) {
    alert.textContent = 'Install the final 1.23b client before logging in.';
  } else {
    alert.textContent = (error && error.message) || 'The server could not complete login.';
  }
}

function startRetryCountdown(alert, seconds) {
  clearRetryCountdown();
  const primary = document.querySelector('#login-submit');
  if (primary) primary.disabled = true;
  let remaining = Math.max(1, seconds || 1);
  const render = () => { alert.textContent = `Too many attempts. Try again in ${remaining} second${remaining === 1 ? '' : 's'}.`; };
  render();
  home.retryTimer = setInterval(() => {
    remaining -= 1;
    if (remaining <= 0) {
      clearRetryCountdown();
      if (primary) primary.disabled = false;
      alert.textContent = 'You can try logging in again.';
    } else render();
  }, 1000);
}
async function chooseInstallFolder() {
  if (home.installSnapshot?.is_running || home.installStartPending) return;
  try {
    const destination = await invoke('pick_directory');
    if (!destination) return;
    home.installDestination = destination;
    home.installError = '';
    localStorage.setItem('bahamut-install-destination', destination);
    renderLifecycleStrip();
  } catch (error) {
    home.installError = error.message || String(error);
    renderLifecycleStrip();
  }
}

async function startInstall() {
  const destination = installTarget();
  if (!destination || home.installStartPending || home.installSnapshot?.is_running) return;
  home.installDestination = destination;
  localStorage.setItem('bahamut-install-destination', destination);
  home.installStartPending = true;
  home.installError = '';
  renderLifecycleStrip();
  try {
    await invoke('install_quote', { destination });
    await invoke('install_game', { destination });
    await refreshInstallSnapshot();
  } catch (error) {
    home.installError = error.message || String(error);
    console.error('Unable to start the game installation.', error);
    renderLifecycleStrip();
  } finally {
    home.installStartPending = false;
    renderHome();
  }
}

function installTarget() {
  return home.installDestination || home.status?.default_game_dir || '';
}

async function launchGame() {
  if (home.gameRunning || home.installSnapshot?.is_running) return;
  const alert = document.querySelector('#launch-alert');
  if (alert) alert.textContent = '';
  const primary = document.querySelector('#home-primary');
  home.gameStatusRevision += 1;
  home.gameLaunchPending = true;
  home.gameRunning = true;
  primary.disabled = true;
  try {
    await invoke('launch_game', { token:home.token || null, server:home.server || null, authEndpoint:home.authEndpoint || null });
  } catch (error) {
    if (error?.kind === 'session-endpoint-changed') {
      forgetSession();
      home.gameRunning = false;
      home.gameLaunchPending = false;
      await refreshHomeStatus();
      document.querySelector('#login-alert').textContent = 'The server address changed. Log in again.';
      return;
    }
    const titles = { 'no-install':'Game install not found', patch:'Patch verification failed', lobby:'Lobby connection failed', 'outdated-client':'Client is not 1.23b', extensions:'Client extensions were not loaded', prerequisite:'Windows runtime is required', 'game-running':'Game is already running' };
    // extensions always carries a backend message; the empty copy falls through to it.
    const copies = { 'no-install':'Set the game install path in Settings to continue.', patch:'The configured client files failed patch verification.', lobby:'The lobby could not be reached. Check the selected server and try again.', 'outdated-client':'Install the final 1.23b client into a new folder before launching.', extensions:'', prerequisite:'', 'game-running':'Close the current client before launching another.' };
    const kind = (error && error.kind) || 'lobby';
    const message = `${titles[kind] || 'Launch failed'}: ${copies[kind] || (error && error.message) || 'The extension bootstrap did not complete and the game was not started.'}`;
    if (alert) alert.textContent = message;
    home.gameRunning = kind === 'game-running';
  } finally {
    home.gameLaunchPending = false;
    primary.disabled = home.gameRunning;
  }
}

async function refreshGameStatus() {
  if (!home.status || home.status.state !== 'ready' || !home.gameRunning) return;
  const revision = ++home.gameStatusRevision;
  try {
    const gameRunning = Boolean(await invoke('game_status'));
    if (revision !== home.gameStatusRevision || (!gameRunning && home.gameLaunchPending)) return;
    home.gameRunning = gameRunning;
    document.querySelector('#home-primary').disabled = home.gameRunning || Boolean(home.installSnapshot?.is_running);
  } catch (error) {
    console.error('Unable to refresh the game process status.', error);
  }
}

async function refreshInstallSnapshot() {
  if (!home.status) return;
  const revision = ++home.installSnapshotRevision;
  const install = await invoke('install_status').catch(() => null);
  if (!install || revision !== home.installSnapshotRevision) return;
  home.installSnapshot = install;
  home.installTerminal = install.is_terminal && install.phase !== 'done' ? install : null;
  if (install.phase !== 'done') home.installDoneHandled = false;
  if (install.phase === 'done' && install.is_terminal && !home.installDoneHandled) {
    home.installDoneHandled = true;
    await refreshHomeStatus(install);
    return;
  }
  renderLifecycleStrip();
}

async function refreshHomeStatus(installSnapshot = null) {
  const sessionToken = home.token;
  const gameStatusRevision = ++home.gameStatusRevision;
  const installRevision = ++home.installSnapshotRevision;
  const [status, gameRunning, install] = await Promise.all([
    invoke('get_home_status', { authenticated:!!home.token }),
    invoke('game_status'),
    installSnapshot ? Promise.resolve(installSnapshot) : invoke('install_status'),
  ]);
  if (sessionToken !== home.token) return;
  home.status = status;
  if (gameStatusRevision === home.gameStatusRevision && (gameRunning || !home.gameLaunchPending)) {
    home.gameRunning = Boolean(gameRunning);
  }
  if (installRevision === home.installSnapshotRevision) {
    home.installSnapshot = install;
    home.installTerminal = install.is_terminal && install.phase !== 'done' ? install : null;
  }
  if (install.phase === 'done' && status.state !== 'no-valid-install' && status.state !== 'outdated-install') {
    home.installDestination = '';
    home.installError = '';
    localStorage.removeItem('bahamut-install-destination');
  }
  renderHome();
}

function resetInstallStatus() {
  home.installTerminal = null;
  home.installSnapshot = null;
  home.installDoneHandled = false;
  renderLifecycleStrip();
}

async function restoreSession() {
  let saved;
  try { saved = JSON.parse(localStorage.getItem('bahamut-session') || 'null'); } catch { saved = null; }
  if (!saved || !saved.token || typeof saved.authEndpoint !== 'string' || !saved.authEndpoint || Number(saved.expiresAt) <= Date.now()) {
    forgetSession();
    return;
  }
  const verdict = await invoke('validate_session', { token:saved.token, server:saved.server || null, authEndpoint:saved.authEndpoint });
  if (verdict === 'invalid' || verdict === 'missing-profile' || verdict === 'endpoint-mismatch') {
    forgetSession();
    return;
  }
  home.username = saved.username || '';
  home.token = saved.token;
  home.authEndpoint = saved.authEndpoint;
  home.expiresAt = Number(saved.expiresAt) || 0;
  home.server = saved.server || '';
}
async function handleSettingsAction(action) {
  const status = document.querySelector('#settings-misc-status');
  const gamepadStatus = document.querySelector('#gamepad-action-status');
  if (status) status.textContent = '';
  if (action === 'open-config') gamepadStatus.textContent = '';
  if (home.installSnapshot?.is_running && action === 'browse-game') {
    if (status) status.textContent = 'Wait for the current install to finish.';
    return;
  }
  try {
    if (action === 'browse-game') {
      const path = await invoke('pick_install_dir');
      if (path) {
        if (home.installSnapshot?.is_terminal) {
          await invoke('reset_install');
          resetInstallStatus();
        }
        home.installDestination = '';
        home.installError = '';
        localStorage.removeItem('bahamut-install-destination');
      }
      await refreshHomeStatus();
    } else if (action === 'open-config') {
      await invoke('launch_config_tool');
      return;
    }
    await hydrateSettings();
  } catch (error) {
    const target = action === 'open-config' ? gamepadStatus : status;
    if (target) target.textContent = error.message || String(error);
  }
}

export { renderNews, renderHome, renderLifecycleStrip, refreshGameStatus, refreshInstallSnapshot, refreshHomeStatus, resetInstallStatus, restoreSession, chooseInstallFolder, startInstall, launchGame, handleSettingsAction };
