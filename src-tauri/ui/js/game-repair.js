import { tauriInvoke, invoke } from './runtime.js';

const repairState = {
  status:null,
  starting:false,
  revision:0,
};

async function refreshGameRepairStatus() {
  if (!tauriInvoke || repairState.starting) return repairState.status;
  const revision = ++repairState.revision;
  try {
    const status = await invoke('game_repair_status');
    if (revision === repairState.revision && !(status.phase === 'idle' && repairState.status?.phase === 'error')) {
      repairState.status = status;
    }
  } catch (error) {
    console.error('Unable to read game repair status.', error);
  }
  return repairState.status;
}

async function startGameRepair() {
  if (!tauriInvoke || repairState.starting || repairState.status?.is_running) return repairState.status;
  repairState.starting = true;
  repairState.status = { phase:'starting', is_running:true, is_terminal:false };
  try {
    await invoke('start_game_repair');
  } catch (error) {
    repairState.status = {
      phase:'error',
      is_running:false,
      is_terminal:true,
      error:error?.message || String(error),
    };
    console.error('Unable to start game repair.', error);
  } finally {
    repairState.starting = false;
  }
  if (repairState.status?.phase !== 'error') await refreshGameRepairStatus();
  return repairState.status;
}

async function controlGameRepair(action) {
  if (!['pause', 'resume', 'cancel'].includes(action)) return repairState.status;
  try {
    await invoke(`${action}_game_repair`);
    await refreshGameRepairStatus();
  } catch (error) {
    console.error(`Unable to ${action} game repair.`, error);
  }
  return repairState.status;
}

export { repairState, refreshGameRepairStatus, startGameRepair, controlGameRepair };
