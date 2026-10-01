import { tauriInvoke, invoke } from './runtime.js';
import { recordFailure, recordPollingFailure, pollingRecovered } from './feedback.js';

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
    pollingRecovered('home/repair', 'read repair status');
    if (revision === repairState.revision && !(status.phase === 'idle' && repairState.status?.phase === 'error')) {
      repairState.status = status;
    }
  } catch (error) {
    recordPollingFailure('home/repair', 'read repair status', error, 'Couldn\'t read game repair status.');
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
    recordFailure('home/repair', 'start repair', error, repairState.status.error);
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
    recordFailure('home/repair', `${action} repair`, error, `Couldn't ${action} game repair.`);
  }
  return repairState.status;
}

export { repairState, refreshGameRepairStatus, startGameRepair, controlGameRepair };
