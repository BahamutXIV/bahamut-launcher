const body = document.body;
const launcherShell = document.querySelector('.launcher-shell');
const tauriInvoke = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke;
const home = {
  status: null,
  news: [],
  username: '',
  token: '',
  authEndpoint: '',
  expiresAt: 0,
  server: '',
  gameRunning: false,
  gameLaunchPending: false,
  gameStatusRevision: 0,
  installTerminal: null,
  installSnapshot: null,
  installSnapshotRevision: 0,
  installDoneHandled: false,
  installStartPending: false,
  installDestination: localStorage.getItem('bahamut-install-destination') || '',
  retryTimer: null,
};
const settingsState = { serverSettings:null, gameSettings:null, launcherBehavior:null, objectDistanceSelection:undefined, cameraZoomSelection:undefined };
const extensionState = { inventory:null, selectedKey:null, query:'' };
const AUTH_ERROR_KINDS = Object.freeze({
  invalidCredentials: 'invalid-credentials',
  rateLimited: 'rate-limited',
  network: 'network',
  noInstall: 'no-install',
  outdatedClient: 'outdated-client',
});

function invoke(command, args) {
  if (!tauriInvoke) return Promise.reject(new Error('Launcher backend is unavailable.'));
  return tauriInvoke(command, args);
}

function escapeHtml(value) {
  return String(value == null ? '' : value)
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

function selectedServer() {
  return home.server || (settingsState.serverSettings && settingsState.serverSettings.selected_server) || null;
}

function serverOptions(selected) {
  const servers = settingsState.serverSettings ? settingsState.serverSettings.servers : [];
  return servers.map(server => {
    const name = server.display_name;
    return `<option value="${escapeHtml(name)}"${name === selected ? ' selected' : ''}>${escapeHtml(name)}</option>`;
  }).join('');
}
function normalizeLauncherLog(content) {
  return String(content == null ? '' : content)
    .replace(/\r\n?/g, '\n')
    .replace(/[ \t]+$/gm, '');
}
function formatBytes(bytes) {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  const index = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
  return `${(bytes / (1024 ** index)).toFixed(index < 2 ? 0 : 1)} ${units[index]}`;
}

export { body, launcherShell, tauriInvoke, home, settingsState, extensionState, AUTH_ERROR_KINDS, invoke, escapeHtml, selectedServer, serverOptions, normalizeLauncherLog, formatBytes };
