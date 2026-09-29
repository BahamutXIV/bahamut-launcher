import { home, settingsState, tauriInvoke, invoke, serverOptions } from './runtime.js';

let gameSettingsSaveQueue = Promise.resolve();
let launcherBehaviorSaveQueue = Promise.resolve();
let borderlessMonitorSaveQueue = Promise.resolve();
let graphicsSaveQueue = Promise.resolve();
let settingsHydrationRevision = 0;
let gameSettingsRevision = 0;
let launcherBehaviorRevision = 0;
let borderlessMonitorRevision = 0;
let objectDistanceRevision = 0;
let cameraZoomRevision = 0;

function renderChoice(selector, value, dataKey) {
  document.querySelectorAll(selector).forEach(button => button.setAttribute('aria-pressed', String(button.dataset[dataKey] === String(value))));
}
function setNestedValue(target, path, value) {
  const parts = path.split('.');
  const key = parts.pop();
  const parent = parts.reduce((current, part) => current[part], target);
  parent[key] = value;
}
function gameSettingValue(settings, path) {
  return path.split('.').reduce((current, part) => current[part], settings);
}
function populateGameSettingOptions(view) {
  const resolution = document.querySelector('#game-resolution');
  const expected = view.supported_resolutions.map(([width, height]) => `${width}x${height}`);
  if ([...resolution.options].map(option => option.value).join(',') !== expected.join(',')) {
    resolution.replaceChildren(...expected.map(value => new Option(value, value)));
  }
}
function renderGameSettingProgress(path) {
  const control = document.querySelector(`input[type="range"][data-game-setting="${path}"]`);
  if (!control) return;
  const minimum = Number(control.min);
  const maximum = Number(control.max);
  const progress = ((Number(control.value) - minimum) / (maximum - minimum)) * 100;
  control.style.setProperty('--range-progress', `${progress}%`);
}
function renderGameSettings(view) {
  gameSettingsRevision += 1;
  settingsState.gameSettings = view;
  populateGameSettingOptions(view);
  const settings = view.settings;
  const status = document.querySelector('#game-settings-status');
  status.textContent = view.available ? '' : 'Game settings require a valid retail config.sys.';
  document.querySelectorAll('[data-game-setting]').forEach(control => {
    const path = control.dataset.gameSetting;
    const value = path === 'resolution' ? `${settings.width}x${settings.height}` : gameSettingValue(settings, path);
    const values = control.dataset.gameSettingValues?.split(',');
    control.value = values ? String(values.indexOf(String(value))) : String(value);
    control.disabled = !view.available;
    renderGameSettingProgress(path);
  });
  document.querySelectorAll('[data-game-setting-choice]').forEach(button => {
    const value = gameSettingValue(settings, button.dataset.gameSettingChoice);
    button.setAttribute('aria-pressed', String(button.dataset.gameSettingValue === String(value)));
    button.disabled = !view.available;
  });
  renderBorderlessMonitorSettings(settingsState.borderlessMonitors);
}

function renderBorderlessMonitorSettings(view) {
  if (!view) return;
  borderlessMonitorRevision += 1;
  settingsState.borderlessMonitors = view;
  const row = document.querySelector('#borderless-monitor-row');
  const select = document.querySelector('#borderless-monitor');
  const status = document.querySelector('#borderless-monitor-status');
  row.hidden = !view.supported;
  const currentValue = view.selected || '';
  const options = [new Option('Default Monitor', '')];
  if (currentValue && !view.monitors.some(monitor => monitor.id === currentValue)) {
    options.push(new Option('Unavailable display (uses primary)', currentValue));
  }
  for (const monitor of view.monitors) {
    const details = `${monitor.width} x ${monitor.height}${monitor.primary ? ', Primary' : ''}`;
    options.push(new Option(`${monitor.name} (${details})`, monitor.id));
  }
  select.replaceChildren(...options);
  select.value = currentValue;
  const borderless = settingsState.gameSettings?.available
    && settingsState.gameSettings.settings?.display_mode === 'borderless';
  select.disabled = !view.supported || !borderless;
  status.textContent = '';
}

function renderObjectDistanceSelection(selection) {
  objectDistanceRevision += 1;
  settingsState.objectDistanceSelection = selection;
  renderGraphicsSelection(document.querySelector('[data-object-distance-selection]'), selection);
}

function queueObjectDistanceSelection(selection) {
  graphicsSaveQueue = graphicsSaveQueue.then(async () => {
    const previous = settingsState.objectDistanceSelection;
    if (previous === undefined) return;
    const status = document.querySelector('#object-distance-status');
    status.textContent = '';
    try {
      renderObjectDistanceSelection(await invoke('set_object_distance_selection', { percent: selection }));
    } catch (error) {
      renderObjectDistanceSelection(previous);
      status.textContent = error.message || String(error);
    }
  });
  return graphicsSaveQueue;
}

function renderCameraZoomSelection(selection) {
  cameraZoomRevision += 1;
  settingsState.cameraZoomSelection = selection;
  renderGraphicsSelection(document.querySelector('[data-camera-zoom-selection]'), selection);
}

function graphicsRangeSelection(range) {
  const value = range.dataset.graphicsValues.split(',')[Number(range.value)];
  return value === 'off' ? null : Number(value);
}

function renderGraphicsRangePreview(range) {
  const selection = graphicsRangeSelection(range);
  const label = selection === null ? 'Off' : range.hasAttribute('data-object-distance-selection') ? `${selection / 100}x` : String(selection);
  range.setAttribute('aria-valuetext', label);
  const progress = Number(range.value) / Number(range.max) * 100;
  range.style.setProperty('--range-progress', `${progress}%`);
}

function renderGraphicsSelection(range, selection) {
  const values = range.dataset.graphicsValues.split(',');
  range.value = String(values.indexOf(selection === null || selection === undefined ? 'off' : String(selection)));
  range.disabled = selection === undefined;
  renderGraphicsRangePreview(range);
}

function queueCameraZoomSelection(selection) {
  graphicsSaveQueue = graphicsSaveQueue.then(async () => {
    const previous = settingsState.cameraZoomSelection;
    if (previous === undefined) return;
    const status = document.querySelector('#camera-zoom-status');
    status.textContent = '';
    try {
      renderCameraZoomSelection(await invoke('set_camera_zoom_selection', { limit: selection }));
    } catch (error) {
      renderCameraZoomSelection(previous);
      status.textContent = error.message || String(error);
    }
  });
  return graphicsSaveQueue;
}

function queueBorderlessMonitorUpdate(monitorId) {
  borderlessMonitorSaveQueue = borderlessMonitorSaveQueue.then(async () => {
    const previous = settingsState.borderlessMonitors;
    if (!previous?.supported) return;
    const status = document.querySelector('#borderless-monitor-status');
    status.textContent = '';
    try {
      renderBorderlessMonitorSettings(await invoke('set_borderless_monitor', { monitorId }));
    } catch (error) {
      renderBorderlessMonitorSettings(previous);
      status.textContent = error.message || String(error);
    }
  });
  return borderlessMonitorSaveQueue;
}
function renderLauncherBehavior(view) {
  launcherBehaviorRevision += 1;
  settingsState.launcherBehavior = view;
  document.querySelectorAll('[data-launcher-setting-choice="close_on_game_start"]').forEach(button => {
    button.setAttribute('aria-pressed', String(button.dataset.launcherSettingValue === String(view.close_on_game_start)));
  });
  document.querySelectorAll('[data-launcher-setting-choice="native_resolution_override"]').forEach(button => {
    button.setAttribute('aria-pressed', String(button.dataset.launcherSettingValue === String(view.native_resolution_override)));
  });
}

function queueGameSettingsUpdate(update) {
  gameSettingsSaveQueue = gameSettingsSaveQueue.then(async () => {
    if (!settingsState.gameSettings?.available) return;
    const previous = settingsState.gameSettings;
    const next = structuredClone(previous.settings);
    update(next);
    const status = document.querySelector('#game-settings-status');
    status.textContent = '';
    try {
      renderGameSettings(await invoke('set_game_settings', { settings:next }));
    } catch (error) {
      renderGameSettings(previous);
      status.textContent = error.message || String(error);
    }
  });
  return gameSettingsSaveQueue;
}
function queueLauncherBehaviorUpdate(closeOnGameStart) {
  return queueLauncherSettingUpdate(() => invoke('set_close_on_game_start', { closeOnGameStart }));
}
function queueNativeResolutionOverrideUpdate(nativeResolutionOverride) {
  return queueLauncherSettingUpdate(() => invoke('set_native_resolution_override', { nativeResolutionOverride }));
}
function queueLauncherSettingUpdate(save) {
  launcherBehaviorSaveQueue = launcherBehaviorSaveQueue.then(async () => {
    const previous = settingsState.launcherBehavior;
    const status = document.querySelector('#game-settings-status');
    const gameSettingsWarning = settingsState.gameSettings?.available === false ? status.textContent : '';
    status.textContent = '';
    try {
      renderLauncherBehavior(await save());
      status.textContent = gameSettingsWarning;
    } catch (error) {
      if (previous) renderLauncherBehavior(previous);
      status.textContent = error.message || String(error);
    }
  });
  return launcherBehaviorSaveQueue;
}
async function hydrateSettings() {
  if (!tauriInvoke) return;
  const revision = ++settingsHydrationRevision;
  const gameRevision = gameSettingsRevision;
  const launcherRevision = launcherBehaviorRevision;
  const monitorRevision = borderlessMonitorRevision;
  const distanceRevision = objectDistanceRevision;
  const zoomRevision = cameraZoomRevision;
  try {
    const [install, gameSettings, launcherBehavior, monitorResult, objectDistanceResult, cameraZoomResult] = await Promise.all([
      invoke('detect_game_install_command'),
      invoke('get_game_settings'),
      invoke('get_launcher_behavior'),
      invoke('get_borderless_monitors')
        .then(view => ({ view, error:null }))
        .catch(error => ({ view:{ supported:false, monitors:[], selected:null }, error })),
      invoke('get_object_distance_selection')
        .then(selection => ({ selection, error:null }))
        .catch(error => ({ selection:undefined, error })),
      invoke('get_camera_zoom_selection')
        .then(selection => ({ selection, error:null }))
        .catch(error => ({ selection:undefined, error })),
    ]);
    if (revision !== settingsHydrationRevision) return;
    const gamePath = document.querySelector('#settings-game-path');
    gamePath.textContent = install.detected || 'No install selected';
    gamePath.title = install.detected || '';
    const operationActive = Boolean(home.installSnapshot?.is_running);
    document.querySelector('[data-settings-action="browse-game"]').disabled = operationActive;
    const monitorResultIsCurrent = monitorRevision === borderlessMonitorRevision;
    if (monitorResultIsCurrent) renderBorderlessMonitorSettings(monitorResult.view);
    if (gameRevision === gameSettingsRevision) renderGameSettings(gameSettings);
    if (launcherRevision === launcherBehaviorRevision) renderLauncherBehavior(launcherBehavior);
    if (monitorResultIsCurrent && monitorResult.error) document.querySelector('#game-settings-status').textContent = `Borderless monitor selection is unavailable: ${monitorResult.error.message || monitorResult.error}`;
    if (distanceRevision === objectDistanceRevision) {
      renderObjectDistanceSelection(objectDistanceResult.selection);
      document.querySelector('#object-distance-status').textContent = objectDistanceResult.error
        ? objectDistanceResult.error.message || String(objectDistanceResult.error)
        : '';
    }
    if (zoomRevision === cameraZoomRevision) {
      renderCameraZoomSelection(cameraZoomResult.selection);
      document.querySelector('#camera-zoom-status').textContent = cameraZoomResult.error
        ? cameraZoomResult.error.message || String(cameraZoomResult.error)
        : '';
    }
  } catch (error) {
    if (revision !== settingsHydrationRevision) return;
    document.querySelector('#settings-misc-status').textContent = error.message || String(error);
  }
}
function renderServerSettings(serverSettings) {
  settingsState.serverSettings = serverSettings;
  const select = document.querySelector('#profile-server-select');
  select.innerHTML = serverOptions(serverSettings.selected_server);
  select.disabled = Boolean(home.token);
  const profile = serverSettings.servers.find(server => server.display_name === serverSettings.selected_server);
  if (!profile) return;
  document.querySelector('#profile-server-auth').textContent = `${profile.use_https ? 'https' : 'http'}://${profile.host}:${profile.auth_port}`;
  document.querySelector('#profile-server-lobby').textContent = `${profile.host}:${profile.lobby_port}`;
  const form = document.querySelector('#profile-form');
  form.dataset.originalDisplayName = profile.display_name;
  form.querySelectorAll('input,button').forEach(control => { control.disabled = Boolean(home.token); });
  document.querySelector('#profile-name').value = profile.display_name;
  document.querySelector('#profile-host').value = profile.host;
  document.querySelector('#profile-auth-port').value = profile.auth_port;
  document.querySelector('#profile-lobby-port').value = profile.lobby_port;
  document.querySelector('#profile-https').checked = profile.use_https;
  const status = document.querySelector('#profile-server-select-status');
  status.textContent = home.token ? 'Log out before changing server profiles.' : '';
  if (home.token) status.dataset.tone = 'error';
  else delete status.dataset.tone;
}

export { renderChoice, setNestedValue, renderGameSettings, renderBorderlessMonitorSettings, queueBorderlessMonitorUpdate, graphicsRangeSelection, renderGraphicsRangePreview, queueObjectDistanceSelection, queueCameraZoomSelection, renderLauncherBehavior, renderGameSettingProgress, queueGameSettingsUpdate, queueLauncherBehaviorUpdate, queueNativeResolutionOverrideUpdate, hydrateSettings, renderServerSettings };
