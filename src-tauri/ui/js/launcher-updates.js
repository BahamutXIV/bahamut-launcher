import { tauriInvoke, invoke } from './runtime.js';

let updateStatus = null;
let updateBusy = false;
let restartAvailable = false;
let startupCheckStarted = false;

function updateOfferAvailable() {
  return ['update_available', 'update_downloaded'].includes(updateStatus?.state);
}

function showUpdateMessage(message = '', tone = '') {
  const notice = document.querySelector('#launcher-update-status');
  if (!notice) return;
  notice.textContent = message;
  if (tone) notice.dataset.tone = tone;
  else delete notice.dataset.tone;
}

function renderLauncherUpdateStatus(status, message = '', tone = '') {
  updateStatus = status;
  const button = document.querySelector('[data-launcher-update-action]');
  if (!button) return;
  const available = updateOfferAvailable();
  button.dataset.launcherUpdateAction = available ? 'apply' : 'check';
  button.textContent = available ? 'Update Launcher' : 'Check for Updates';
  button.disabled = updateBusy || !tauriInvoke || (available && !restartAvailable);
  showUpdateMessage(message, tone);
}

async function refreshLauncherUpdateAvailability() {
  if (!updateOfferAvailable() || updateBusy) return;
  try {
    restartAvailable = await invoke('launcher_update_restart_available');
  } catch {
    restartAvailable = false;
  }
  const button = document.querySelector('[data-launcher-update-action]');
  if (button) button.disabled = !restartAvailable;
  if (!restartAvailable && document.querySelector('#launcher-update-status')?.dataset.tone !== 'error') {
    showUpdateMessage('Finish game or other launcher work before updating.');
  }
}

async function hydrateLauncherUpdates() {
  if (!tauriInvoke || updateBusy) return;
  try {
    const status = await invoke('get_launcher_update_status');
    renderLauncherUpdateStatus(status);
    await refreshLauncherUpdateAvailability();
    if (updateOfferAvailable()) {
      showUpdateMessage(`Version ${status.offeredVersion || 'new'} available.`);
    }
  } catch {
    renderLauncherUpdateStatus({ state:'blocked' });
  }
}

function startBackgroundLauncherUpdateCheck() {
  if (!tauriInvoke || startupCheckStarted) return;
  startupCheckStarted = true;
  updateBusy = true;
  const button = document.querySelector('[data-launcher-update-action]');
  if (button) button.disabled = true;
  void invoke('check_launcher_update').then(async status => {
    updateBusy = false;
    restartAvailable = false;
    renderLauncherUpdateStatus(status);
    await refreshLauncherUpdateAvailability();
    if (updateOfferAvailable() && restartAvailable) {
      showUpdateMessage(`Version ${status.offeredVersion || 'new'} available.`);
    }
  }).catch(error => {
    console.error('Launcher update check failed.', error);
    updateBusy = false;
    renderLauncherUpdateStatus({ state:'blocked' });
  });
}

async function handleLauncherUpdateAction(event) {
  const button = event?.target?.closest?.('[data-launcher-update-action]');
  if (!button || updateBusy || button.disabled) return null;
  const action = button.dataset.launcherUpdateAction;
  const restoreFocus = document.activeElement === button;
  updateBusy = true;
  renderLauncherUpdateStatus(updateStatus, action === 'check'
    ? 'Checking for launcher updates...'
    : 'Downloading and applying launcher update...');
  try {
    if (action === 'check') {
      const status = await invoke('check_launcher_update');
      updateBusy = false;
      restartAvailable = false;
      renderLauncherUpdateStatus(status, status.state === 'current'
        ? 'Launcher is up to date.'
        : status.state === 'update_available'
          ? `Version ${status.offeredVersion || 'new'} available.`
          : status.state === 'blocked' ? 'Update checks unavailable.' : '');
      await refreshLauncherUpdateAvailability();
      if (restoreFocus && !button.disabled) button.focus({ preventScroll:true });
      return status;
    }
    const result = await invoke('apply_launcher_update');
    if (result?.state === 'handoff') {
      showUpdateMessage('Restarting to finish the update.');
      button.disabled = true;
      try {
        await invoke('control_window', { action:'close' });
      } catch (error) {
        showUpdateMessage('Close the launcher to finish the update.', 'error');
        console.error('Could not close launcher for update.', error);
      }
      return result;
    }
    updateBusy = false;
    renderLauncherUpdateStatus(updateStatus, result?.message || 'Launcher update did not start.', 'error');
    await refreshLauncherUpdateAvailability();
    return result;
  } catch (error) {
    updateBusy = false;
    renderLauncherUpdateStatus(updateStatus, action === 'check'
      ? 'Update checks unavailable.'
      : (error?.message || 'Launcher update failed.'), 'error');
    await refreshLauncherUpdateAvailability();
    console.error('Launcher update failed.', error);
    if (restoreFocus && !button.disabled) button.focus({ preventScroll:true });
    return null;
  }
}

export {
  hydrateLauncherUpdates,
  handleLauncherUpdateAction,
  refreshLauncherUpdateAvailability,
  startBackgroundLauncherUpdateCheck,
};
