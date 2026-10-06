import { home, settingsState, tauriInvoke, invoke, serverOptions } from './runtime.js';
import { feedbackToken, feedbackCurrent, clearFeedback, showFeedback, reportFailure, recordFailure } from './feedback.js';

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

const GAME_SETTINGS_WARNING = 'Game settings require a valid retail config.sys.';
const BORDERLESS_MONITOR_WARNING = 'Borderless monitor selection is unavailable. Copy logs from the Help page for support.';
const OBJECT_DISTANCE_WARNING = 'Extended draw distance is unavailable. Copy logs from the Help page for support.';
const CAMERA_ZOOM_WARNING = 'Extended camera zoom is unavailable. Copy logs from the Help page for support.';

function activeSettingsCard() {
  return document.querySelector('[data-settings-pane][data-active] .settings-card')
    || document.querySelector('[data-screen="settings"] .settings-card');
}

function settingLabel(target, fallback = 'game settings') {
  const element = typeof target === 'string' ? document.querySelector(target) : target;
  const label = element?.closest?.('.settings-field')?.querySelector('.settings-label, label')?.textContent?.trim();
  return label || fallback;
}

function settingAction(target, fallback = 'game settings') {
  return `save ${settingLabel(target, fallback)}`;
}

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
  if (row) row.hidden = !view.supported;
  const currentValue = view.selected || '';
  const options = [new Option('Default Monitor', '')];
  if (currentValue && !view.monitors.some(monitor => monitor.id === currentValue)) {
    options.push(new Option('Unavailable display (uses primary)', currentValue));
  }
  for (const monitor of view.monitors) {
    const details = `${monitor.width} x ${monitor.height}${monitor.primary ? ', Primary' : ''}`;
    options.push(new Option(`${monitor.name} (${details})`, monitor.id));
  }
  if (select) {
    select.replaceChildren(...options);
    select.value = currentValue;
  }
  const borderless = settingsState.gameSettings?.available
    && settingsState.gameSettings.settings?.display_mode === 'borderless';
  if (select) select.disabled = !view.supported || !borderless;
}

function renderObjectDistanceSelection(selection) {
  objectDistanceRevision += 1;
  settingsState.objectDistanceSelection = selection;
  renderGraphicsSelection(document.querySelector('[data-object-distance-selection]'), selection);
}

function queueObjectDistanceSelection(selection) {
  const token = feedbackToken('#object-distance-range');
  const action = 'save Extended Draw Distance';
  clearFeedback(token);
  graphicsSaveQueue = graphicsSaveQueue.then(async () => {
    const previous = settingsState.objectDistanceSelection;
    if (previous === undefined) return;
    try {
      renderObjectDistanceSelection(await invoke('set_object_distance_selection', { percent: selection }));
    } catch (error) {
      renderObjectDistanceSelection(previous);
      reportFailure(token, action, error, { context:'set_object_distance_selection' });
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
  const token = feedbackToken('#camera-zoom-range');
  const action = 'save Extended Camera Zoom';
  clearFeedback(token);
  graphicsSaveQueue = graphicsSaveQueue.then(async () => {
    const previous = settingsState.cameraZoomSelection;
    if (previous === undefined) return;
    try {
      renderCameraZoomSelection(await invoke('set_camera_zoom_selection', { limit: selection }));
    } catch (error) {
      renderCameraZoomSelection(previous);
      reportFailure(token, action, error, { context:'set_camera_zoom_selection' });
    }
  });
  return graphicsSaveQueue;
}

function queueBorderlessMonitorUpdate(monitorId) {
  const token = feedbackToken('#borderless-monitor');
  const action = 'save Borderless Monitor';
  clearFeedback(token);
  borderlessMonitorSaveQueue = borderlessMonitorSaveQueue.then(async () => {
    const previous = settingsState.borderlessMonitors;
    if (!previous?.supported) return;
    try {
      renderBorderlessMonitorSettings(await invoke('set_borderless_monitor', { monitorId }));
    } catch (error) {
      renderBorderlessMonitorSettings(previous);
      reportFailure(token, action, error, { context:'set_borderless_monitor' });
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

function queueGameSettingsUpdate(update, target) {
  const token = feedbackToken(target || activeSettingsCard());
  const action = settingAction(target);
  clearFeedback(token);
  gameSettingsSaveQueue = gameSettingsSaveQueue.then(async () => {
    if (!settingsState.gameSettings?.available) return;
    const previous = settingsState.gameSettings;
    const next = structuredClone(previous.settings);
    update(next);
    try {
      renderGameSettings(await invoke('set_game_settings', { settings:next }));
    } catch (error) {
      renderGameSettings(previous);
      reportFailure(token, action, error, { context:'set_game_settings' });
    }
  });
  return gameSettingsSaveQueue;
}
function queueLauncherBehaviorUpdate(closeOnGameStart, target) {
  return queueLauncherSettingUpdate(
    () => invoke('set_close_on_game_start', { closeOnGameStart }),
    target,
    'save Close Launcher on Game Start',
    'set_close_on_game_start',
  );
}
function queueNativeResolutionOverrideUpdate(nativeResolutionOverride, target) {
  return queueLauncherSettingUpdate(
    () => invoke('set_native_resolution_override', { nativeResolutionOverride }),
    target,
    'save Native Resolution Override',
    'set_native_resolution_override',
  );
}
function queueLauncherSettingUpdate(save, target, action, context) {
  const token = feedbackToken(target || activeSettingsCard());
  clearFeedback(token);
  launcherBehaviorSaveQueue = launcherBehaviorSaveQueue.then(async () => {
    const previous = settingsState.launcherBehavior;
    try {
      renderLauncherBehavior(await save());
      if (settingsState.gameSettings?.available === false && feedbackCurrent(token)) {
        showFeedback(token, GAME_SETTINGS_WARNING, 'warning');
      }
    } catch (error) {
      if (previous) renderLauncherBehavior(previous);
      reportFailure(token, action, error, { context });
    }
  });
  return launcherBehaviorSaveQueue;
}
async function hydrateSettings(lifetimeToken) {
  if (!tauriInvoke) return;
  const token = feedbackToken(activeSettingsCard());
  const monitorToken = feedbackToken('#borderless-monitor');
  const objectDistanceToken = feedbackToken('#object-distance-range');
  const cameraZoomToken = feedbackToken('#camera-zoom-range');
  const canShow = candidate => feedbackCurrent(candidate) && (!lifetimeToken || feedbackCurrent(lifetimeToken));
  if (canShow(token)) clearFeedback(token);
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
        .catch(error => {
          void recordFailure(monitorToken.scope, 'load borderless monitor settings', error, BORDERLESS_MONITOR_WARNING, 'get_borderless_monitors');
          return { view:{ supported:false, monitors:[], selected:null }, error };
        }),
      invoke('get_object_distance_selection')
        .then(selection => ({ selection, error:null }))
        .catch(error => {
          void recordFailure(objectDistanceToken.scope, 'load Extended Draw Distance', error, OBJECT_DISTANCE_WARNING, 'get_object_distance_selection');
          return { selection:undefined, error };
        }),
      invoke('get_camera_zoom_selection')
        .then(selection => ({ selection, error:null }))
        .catch(error => {
          void recordFailure(cameraZoomToken.scope, 'load Extended Camera Zoom', error, CAMERA_ZOOM_WARNING, 'get_camera_zoom_selection');
          return { selection:undefined, error };
        }),
    ]);
    if (revision !== settingsHydrationRevision) return;
    const gamePath = document.querySelector('#settings-game-path');
    gamePath.textContent = install.detected || 'No install selected';
    gamePath.title = install.detected || '';
    const operationActive = Boolean(home.installSnapshot?.is_running);
    document.querySelector('[data-settings-action="browse-game"]').disabled = operationActive;
    const monitorResultIsCurrent = monitorRevision === borderlessMonitorRevision;
    const gameResultIsCurrent = gameRevision === gameSettingsRevision;
    if (monitorResultIsCurrent) renderBorderlessMonitorSettings(monitorResult.view);
    if (gameResultIsCurrent) renderGameSettings(gameSettings);
    if (launcherRevision === launcherBehaviorRevision) renderLauncherBehavior(launcherBehavior);
    if (gameResultIsCurrent && !gameSettings.available && canShow(token)) {
      showFeedback(token, GAME_SETTINGS_WARNING, 'warning');
    }
    if (monitorResultIsCurrent && monitorResult.error && canShow(monitorToken)) {
      showFeedback(monitorToken, BORDERLESS_MONITOR_WARNING, 'warning');
    }
    if (distanceRevision === objectDistanceRevision) {
      renderObjectDistanceSelection(objectDistanceResult.selection);
      if (objectDistanceResult.error && canShow(objectDistanceToken)) {
        showFeedback(objectDistanceToken, OBJECT_DISTANCE_WARNING, 'warning');
      }
    }
    if (zoomRevision === cameraZoomRevision) {
      renderCameraZoomSelection(cameraZoomResult.selection);
      if (cameraZoomResult.error && canShow(cameraZoomToken)) {
        showFeedback(cameraZoomToken, CAMERA_ZOOM_WARNING, 'warning');
      }
    }
  } catch (error) {
    if (revision !== settingsHydrationRevision || !canShow(token)) {
      void recordFailure(token.scope, 'load settings', error, `Couldn't load settings. Copy logs from the Help page for support.`, 'hydrate_settings');
      return;
    }
    reportFailure(token, 'load settings', error, { context:'hydrate_settings' });
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
  if (home.token) showFeedback(feedbackToken('#profile-server-select'), 'Log out before changing server profiles.');
}

export { renderChoice, setNestedValue, renderGameSettings, renderBorderlessMonitorSettings, queueBorderlessMonitorUpdate, graphicsRangeSelection, renderGraphicsRangePreview, queueObjectDistanceSelection, queueCameraZoomSelection, renderLauncherBehavior, renderGameSettingProgress, queueGameSettingsUpdate, queueLauncherBehaviorUpdate, queueNativeResolutionOverrideUpdate, hydrateSettings, renderServerSettings };
