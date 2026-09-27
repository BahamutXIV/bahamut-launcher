import { test } from 'node:test';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { existsSync, lstatSync, mkdtempSync, readdirSync, readFileSync, rmSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';

const UI_PATH = new URL('./index.html', import.meta.url);
const TAURI_CONFIG_PATH = new URL('../tauri.conf.json', import.meta.url);
const DAY_BRAND_LOGO_PATH = new URL('./assets/brand-logo-day.png', import.meta.url);
const NIGHT_BRAND_LOGO_PATH = new URL('./assets/brand-logo-night.png', import.meta.url);
const DAY_BACKGROUND_PATH = new URL('./assets/background-day.png', import.meta.url);
const NIGHT_BACKGROUND_PATH = new URL('./assets/background-night.jpg', import.meta.url);
const DAY_THEME_ICON_PATH = new URL('./assets/theme-day.png', import.meta.url);
const NIGHT_THEME_ICON_PATH = new URL('./assets/theme-night.png', import.meta.url);
const NEWS_PLACEHOLDER_PATH = new URL('./assets/news-placeholder.jpg', import.meta.url);
const FONT_CSS_PATH = new URL('./assets/fonts/fonts.css', import.meta.url);
const INTER_FONT_PATH = new URL('./assets/fonts/inter-opsz-wght.ttf', import.meta.url);
const CINZEL_FONT_PATH = new URL('./assets/fonts/cinzel-wght.ttf', import.meta.url);
const JETBRAINS_FONT_PATH = new URL('./assets/fonts/jetbrainsmono-v24-tDbv2o-flEEny0FZhsfKu5WU4zr3E_BX0PnT8RD8yKwBNntkaToggR7BYRbKPxDcwg.woff2', import.meta.url);
const UI_FILES = new Map([
  ['/css/theme.css', [new URL('./css/theme.css', import.meta.url), 'text/css; charset=utf-8']],
  ['/css/shell.css', [new URL('./css/shell.css', import.meta.url), 'text/css; charset=utf-8']],
  ['/css/screens.css', [new URL('./css/screens.css', import.meta.url), 'text/css; charset=utf-8']],
  ['/css/components.css', [new URL('./css/components.css', import.meta.url), 'text/css; charset=utf-8']],
  ['/js/runtime.js', [new URL('./js/runtime.js', import.meta.url), 'text/javascript; charset=utf-8']],
  ['/js/settings.js', [new URL('./js/settings.js', import.meta.url), 'text/javascript; charset=utf-8']],
  ['/js/extensions.js', [new URL('./js/extensions.js', import.meta.url), 'text/javascript; charset=utf-8']],
  ['/js/game-repair.js', [new URL('./js/game-repair.js', import.meta.url), 'text/javascript; charset=utf-8']],
  ['/js/launcher-updates.js', [new URL('./js/launcher-updates.js', import.meta.url), 'text/javascript; charset=utf-8']],
  ['/js/home.js', [new URL('./js/home.js', import.meta.url), 'text/javascript; charset=utf-8']],
  ['/js/main.js', [new URL('./js/main.js', import.meta.url), 'text/javascript; charset=utf-8']],
  ['/assets/theme-day.png', [DAY_THEME_ICON_PATH, 'image/png']],
  ['/assets/theme-night.png', [NIGHT_THEME_ICON_PATH, 'image/png']],
  ['/assets/news-placeholder.jpg', [NEWS_PLACEHOLDER_PATH, 'image/jpeg']],
]);
const GAMEPAD_ASSETS = new Map([
  ['xbox/a.png', [new URL('./assets/gamepad/xbox/a.png', import.meta.url), '1ad0ba1faabf928c75ed7897be52e2200f93cee310ecdfb64f3b62483f65595e']],
  ['xbox/b.png', [new URL('./assets/gamepad/xbox/b.png', import.meta.url), 'e6175e24d248390e0922326803e9757e4201894597d7f75729000a9ca2a249f7']],
  ['xbox/x.png', [new URL('./assets/gamepad/xbox/x.png', import.meta.url), '0b2aa73a3ec7c15e698c02b0be5298d43dd9ea09ba40cc486c007e3d92685e54']],
  ['xbox/y.png', [new URL('./assets/gamepad/xbox/y.png', import.meta.url), '875dcda45683d1b5d34fe229648cb162f32f802ff3571b9ec119da05960fe094']],
  ['playstation/circle.png', [new URL('./assets/gamepad/playstation/circle.png', import.meta.url), '84d90d0ee555a9ab8a50780f1968ee9b1352359227ef6602ceef2c6120d85259']],
  ['playstation/cross.png', [new URL('./assets/gamepad/playstation/cross.png', import.meta.url), 'fd38583c20d852dd676b12413c47ed4ab5e43174f7b0be277733a6d69d8c88db']],
  ['playstation/square.png', [new URL('./assets/gamepad/playstation/square.png', import.meta.url), 'cd7d1fa0e2bfbd033e950d212b6694e057d715a4c4d49b48514e095877a6bad1']],
  ['playstation/triangle.png', [new URL('./assets/gamepad/playstation/triangle.png', import.meta.url), '23568ef932d404ea8c29fe03fcbabdf94e5a2f38ca033bf52239033c2637a9ff']],
]);
const ICON_PNG_PATH = new URL('../icons/icon.png', import.meta.url);
const ICON_ICO_PATH = new URL('../icons/icon.ico', import.meta.url);
const ICON_SOURCE_PATH = new URL('../icons/icon-source.png', import.meta.url);
const BROWSER_CANDIDATES = [
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
  'C:/Program Files/Microsoft/Edge/Application/msedge.exe',
];
const DEVTOOLS_CONNECT_TIMEOUT_MS = 30_000;
const DEVTOOLS_POLL_INTERVAL_MS = 100;

function browserPath() {
  const path = BROWSER_CANDIDATES.find(candidate => existsSync(candidate));
  if (!path) throw new Error('A Chromium browser (Edge or Chrome) is required for this test.');
  return path;
}

function delay(milliseconds) {
  return new Promise(resolve => setTimeout(resolve, milliseconds));
}

async function stopChild(child) {
  if (child.exitCode !== null) return;
  child.kill();
  await new Promise(resolve => child.once('exit', resolve));
}

function removeTempRoot(root) {
  const parent = join(tmpdir());
  if (join(root, '..') !== parent || !root.split(/[\\/]/).pop().startsWith('bahamut-launcher-browser-')) {
    throw new Error(`Refusing to remove unexpected browser profile path: ${root}`);
  }
  const unlinkJunctions = directory => {
    for (const entry of readdirSync(directory)) {
      const path = join(directory, entry);
      if (lstatSync(path).isSymbolicLink()) unlinkSync(path);
      else if (lstatSync(path).isDirectory()) unlinkJunctions(path);
    }
  };
  unlinkJunctions(root);
  rmSync(root, { recursive: true, maxRetries: 50, retryDelay: 100 });
}

async function freePort() {
  const probe = createServer();
  await new Promise(resolve => probe.listen(0, '127.0.0.1', resolve));
  const port = probe.address().port;
  await new Promise(resolve => probe.close(resolve));
  return port;
}

function serveUiFixture(request, response) {
  if (request.url?.startsWith('/index.html')) {
    response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
    response.end(readFileSync(UI_PATH));
    return;
  }
  const uiFile = UI_FILES.get(request.url);
  if (uiFile) {
    response.writeHead(200, { 'content-type': uiFile[1] });
    response.end(readFileSync(uiFile[0]));
    return;
  }
  const gamepadAsset = request.url?.startsWith('/assets/gamepad/')
    ? GAMEPAD_ASSETS.get(request.url.slice('/assets/gamepad/'.length))?.[0]
    : null;
  const asset = request.url === '/assets/background-day.png'
    ? [DAY_BACKGROUND_PATH, 'image/png']
    : request.url === '/assets/background-night.jpg'
      ? [NIGHT_BACKGROUND_PATH, 'image/jpeg']
      : request.url === '/assets/fonts/fonts.css'
        ? [FONT_CSS_PATH, 'text/css; charset=utf-8']
        : request.url === '/assets/fonts/inter-opsz-wght.ttf'
          ? [INTER_FONT_PATH, 'font/ttf']
        : request.url === '/assets/fonts/cinzel-wght.ttf'
          ? [CINZEL_FONT_PATH, 'font/ttf']
        : request.url === '/assets/fonts/jetbrainsmono-v24-tDbv2o-flEEny0FZhsfKu5WU4zr3E_BX0PnT8RD8yKwBNntkaToggR7BYRbKPxDcwg.woff2'
          ? [JETBRAINS_FONT_PATH, 'font/woff2']
      : gamepadAsset
        ? [gamepadAsset, 'image/png']
        : null;
  if (asset) {
    response.writeHead(200, { 'content-type': asset[1] });
    response.end(readFileSync(asset[0]));
    return;
  }
  response.writeHead(404);
  response.end();
}

class DevTools {
  constructor(socket) {
    this.socket = socket;
    this.nextId = 1;
    this.pending = new Map();
    socket.addEventListener('message', event => {
      const message = JSON.parse(event.data);
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(message.error.message));
      else pending.resolve(message.result || {});
    });
  }

  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }
}

async function connectDevTools(port) {
  const deadline = Date.now() + DEVTOOLS_CONNECT_TIMEOUT_MS;
  do {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/list`);
      const targets = await response.json();
      const target = targets.find(candidate => candidate.type === 'page');
      if (!target) throw new Error('No browser page target is available.');
      const socket = new WebSocket(target.webSocketDebuggerUrl);
      await new Promise((resolve, reject) => {
        socket.addEventListener('open', resolve, { once: true });
        socket.addEventListener('error', reject, { once: true });
      });
      return new DevTools(socket);
    } catch {
      await delay(DEVTOOLS_POLL_INTERVAL_MS);
    }
  } while (Date.now() < deadline);
  throw new Error('Timed out waiting for the browser DevTools endpoint.');
}

function evaluateScript() {
  return `(() => {
    const state = {
      patchRequired: false,
      noValidInstall: false,
      hostedPatches: true,
      mode: 'success',
      loginGate: false,
      loginStarted: false,
      finishLogin: null,
      lastLoginRecipient: null,
      lastLoginResponseEndpoint: null,
      patch: { phase: 'idle', download_idx:0, patch_idx:0, total_patches:4, bytes_downloaded:0, previous_completed_bytes:0, total_download_bytes:100, is_running: false, is_paused: false, pause_requested:false, is_terminal: false, error: null },
      patchStartError: null,
      patchStartDelayMs: 0,
      installQuote: { download_bytes:100, staging_bytes:200, destination_bytes:300, available_cache_bytes:1000, available_destination_bytes:2000 },
      installQuoteError: null,
      installStartError: null,
      nextDirectory: 'C:/patch-source',
      nextInstallDir: 'C:/game',
      pickerSelectsFinal: false,
      patchStatusDelayMs: 0,
      readyStatusDelayMs: 0,
      sessionValidation: 'invalid',
      gameRunning: false,
      gameStatusDelayMs: 0,
      launchDelayMs: 0,
      launchError: null,
      patchSettings: { storage_dir: 'C:/patches', storage_overridden: true },
      launcherBehavior: { close_on_game_start: true, native_resolution_override: false },
      borderlessMonitors: {
        supported:true,
        monitors:[
          { id:'display-primary-id', name:'Primary Display', width:1920, height:1080, primary:true },
          { id:'display-secondary-id', name:'Side Display', width:2560, height:1440, primary:false },
        ],
        selected:'removed-display-id',
      },
      serverSettings: {
        selected_server: 'Local',
        servers: [
          { display_name: 'Local', host: '127.0.0.1', auth_port: 8080, lobby_port: 54994, use_https: false },
          { display_name: 'Bahamut', host: 'bahamut.example', auth_port: 443, lobby_port: 54994, use_https: true },
        ],
      },
      failAddonWrite: false,
      launcherUpdateStatus: {
        state:'blocked', message:'Launcher updates need an explicit signed source.', installedVersion:null, offeredVersion:null,
      },
      launcherUpdateCheckResult: {
        state:'current', message:'Bahamut Launcher version 1.0.0 is installed.', installedVersion:'1.0.0', offeredVersion:null,
      },
      launcherUpdateRestartAvailable: true,
      launcherUpdateChecks: 0,
      launcherUpdateApplyResult: {
        state:'handoff', message:'Launcher update to version 1.1.0 is ready. The launcher will close and restart.',
      },
      failLauncherUpdateAction: null,
      gameRepairStatus: {
        phase:'idle', is_running:false, is_terminal:false, is_paused:false,
        pause_requested:false, cancel_requested:false, can_cancel:false,
        bytes_completed:0, bytes_total:null, files_completed:0, files_total:null,
        current_file:null, complete:false, error:null,
      },
      gameRepairStartError: null,
      failGameSettingsWrite: false,
      gameSettingsWriteDelayMs: 0,
      gameSettingsWritesInFlight: 0,
      maxGameSettingsWritesInFlight: 0,
      objectDistanceEnabled: false,
      objectDistancePercent: 200,
      cameraZoomEnabled: false,
      cameraZoomLimit: 15,
      failObjectDistanceWrite: false,
      failCameraZoomWrite: false,
      settingsHydrationPlan: null,
      gameSettings: {
        available: true,
        settings: {
          display_mode: 'windowed', width: 1280, height: 720,
          graphics: {
            multisampling: 'none', general_quality: 8, background_quality: 5,
            shadow_detail: 'standard', ambient_occlusion: false, depth_of_field: false,
            cutscene_effects: true, hardware_mouse: true,
            texture_quality: 'standard', texture_filtering: 'standard',
          },
          audio: { enabled: true, play_in_background: false },
        },
        supported_resolutions: [
          [1024,768],[1152,864],[1280,720],[1280,768],[1280,800],[1280,854],[1280,960],[1280,1024],[1360,768],
          [1366,768],[1368,768],[1400,1050],[1440,900],[1440,1050],[1440,1080],[1600,900],[1600,1024],[1600,1050],
          [1600,1200],[1680,1050],[1920,1080],[1920,1200],[1920,1440],[2048,1536],[2560,1440],[2560,1600],[2560,2048],
        ],
      },
      extensionInventory: {
        overlays: [{
          id:'bahamut-dats-overlay', name:'Bahamut DAT Overlay', author:'BahamutXIV', version:'1.0.0',
          description:'Bundled official overlay.', homepage:null, enabled:true,
        }, {
          id:'base-world', name:'Base World', author:'Aeshur', version:'1.0.0',
          description:'Base world replacement DATs.', homepage:'https://example.test/base-world',
          enabled:true,
        }, {
          id:'detail-world', name:'Detail World', author:'BahamutXIV', version:'2.0.0',
          description:'Higher-detail world replacement DATs.', homepage:null,
          enabled:true,
        }],
        overlay_conflicts: [{ relative_path:'data/2A/08.DAT', package_ids:['base-world', 'detail-world'] }],
        plugins: [{
          id:'screenshot', name:'Screenshot', author:'Aeshur', version:'1.0',
          description:'Captures the game to the portable screenshots folder.',
          homepage:null, commands:[
            { name:'screenshot', usage:'/screenshot', description:'Captures a frame when invoked by a configured binding.' },
          ], capabilities:[], compatibility:'Supported client build',
          status:'Enabled', trust:'First-party native', enabled:true,
        }, {
          id:'discord-rpc', name:'DiscordRPC', author:'Aeshur', version:'1.0',
          description:'Shows your character name, location, and level in Discord as rich presence.',
          homepage:null, commands:[], capabilities:[], compatibility:'Supported client build',
          status:'Enabled', trust:'First-party native', enabled:true,
        }],
        addons: [{
          id:'fps', name:'fps', author:'Aeshur', version:'1.0',
          description:"Displays the game's current frame rate.",
          homepage:'https://github.com/BahamutXIV/bahamut-launcher', commands:[
            { name:'/fps', usage:'/fps', description:'Toggle the FPS overlay.' },
            { name:'/fps lock', usage:'/fps lock', description:'Toggle overlay dragging.' },
          ],
          capabilities:['ui.draw'], compatibility:'Supported client build', status:'Enabled',
          trust:'Isolated Lua', enabled:true,
        }, {
          id:'wiki', name:'wiki', author:'Aeshur', version:'1.0',
          description:'Opens the Bahamut wiki and searches its MediaWiki pages in your default browser.',
          homepage:'https://bahamut.miraheze.org/wiki/Main_Page', commands:[
            { name:'/wiki', usage:'/wiki', description:'Open the Bahamut wiki.' },
          ], capabilities:['chat.print','url.open'], compatibility:'Supported client build', status:'Disabled',
          trust:'Isolated Lua', enabled:false,
        }],
      },
      supportLog: {
        content: 'first line  \\r\\npassword=[REDACTED]\\r\\n' + Array.from({ length: 180 }, (_, index) => 'launcher event ' + index).join('\\r\\n'),
        log_path: 'C:/BahamutXIV Launcher/logs/launcher/bahamut-launcher.log',
        truncated: false,
        updated_at: 1788026400000,
      },
      clipboardText: null,
      clipboardMode: 'success',
      profileMode: 'success',
      registerMode: 'success',
      backupMode: 'success',
      gamepads: [],
      eventHandlers: {},
      calls: [],
    };
    window.__launcherBrowserState = state;
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: async text => {
          if (state.clipboardMode === 'error') throw new Error('clipboard denied');
          state.clipboardText = text;
        },
      },
    });
    Object.defineProperty(navigator, 'getGamepads', {
      configurable: true,
      value: () => state.gamepads,
    });
    window.__TAURI__ = { event: { listen: async (name, handler) => { state.eventHandlers[name] = handler; } }, core: { invoke: async (command, args = {}) => {
      state.calls.push({ command, args });
      if (state.settingsHydrationPlan?.command === command) {
        const plan = state.settingsHydrationPlan;
        state.settingsHydrationPlan = null;
        if (plan.delayMs) await new Promise(resolve => setTimeout(resolve, plan.delayMs));
        if (plan.error) throw new Error(plan.error);
      }
      if (command === 'fit_window_to_work_area' && location.search === '?fit-fail') throw new Error('fixture resize failure');
      if (command === 'get_server_settings') return structuredClone(state.serverSettings);
      if (command === 'set_selected_server') {
        if (state.profileMode === 'select-error') throw new Error('Server profile selection could not be saved because the fixture returned a deliberately long validation message.');
        state.serverSettings.selected_server = args.displayName;
        return structuredClone(state.serverSettings);
      }
      if (command === 'save_server_profile') {
        const index = state.serverSettings.servers.findIndex(server => server.display_name === args.originalDisplayName);
        state.serverSettings.servers[index] = structuredClone(args.profile);
        if (state.serverSettings.selected_server === args.originalDisplayName) state.serverSettings.selected_server = args.profile.display_name;
        return structuredClone(state.serverSettings);
      }
      if (command === 'list_news') return [{
        date:'September 27',
        title:'Open Beta is Live',
        body:"Bahamut's open beta is now live!",
      }];
      if (command === 'launcher_version') return 'browser-test';
      if (command === 'get_home_status') {
        const authenticated = !!args.authenticated;
        if (authenticated && state.readyStatusDelayMs) await new Promise(resolve => setTimeout(resolve, state.readyStatusDelayMs));
        const stateName = state.noValidInstall ? 'no-valid-install' : state.patchRequired ? 'patch-required' : authenticated ? 'ready' : 'logged-out';
        const presentation = {
          ready: ['', 'Account Login', 'Play'],
          'patch-required': ['', 'Account Login', 'Update'],
          'no-valid-install': ['', 'Account Login', 'Install'],
          'logged-out': ['', 'Account Login', 'Play'],
        }[stateName];
        return { state: stateName, eyebrow: presentation[0], title: presentation[1], primary_action: presentation[2], game_dir: state.noValidInstall ? null : 'C:/game', default_game_dir:'C:/Games/FINAL FANTASY XIV', game_version: state.patchRequired ? 'old' : null, hosted_patches: state.hostedPatches };
      }
      if (command === 'validate_session') return state.sessionValidation;
      if (command === 'game_status') {
        const running = state.gameRunning;
        if (state.gameStatusDelayMs) await new Promise(resolve => setTimeout(resolve, state.gameStatusDelayMs));
        return running;
      }
      if (command === 'launch_game') {
        if (state.gameRunning) throw { kind:'game-running', message:'The game is already running.' };
        state.gameRunning = true;
        if (state.launchDelayMs) await new Promise(resolve => setTimeout(resolve, state.launchDelayMs));
        if (state.launchError) {
          state.gameRunning = false;
          throw structuredClone(state.launchError);
        }
        return null;
      }
      if (command === 'login') {
        if (state.mode === 'invalid') throw { kind: 'invalid-credentials' };
        if (state.mode === 'rate') throw { kind: 'rate-limited', message: 'too many attempts', retryAfter: 2 };
        state.mode = 'success';
        const profile = state.serverSettings.servers.find(candidate => candidate.display_name === args.server);
        const scheme = profile?.use_https ? 'https' : 'http';
        const endpoint = new URL(scheme + '://' + (profile?.host || '127.0.0.1') + ':' + (profile?.auth_port || 8080) + '/api/v1/').href;
        state.lastLoginRecipient = endpoint;
        if (state.loginGate) {
          state.loginStarted = true;
          await new Promise(resolve => { state.finishLogin = resolve; });
          state.loginGate = false;
        }
        const response = { token: '0123456789abcdef0123456789abcdef0123456789abcdef01234567', authEndpoint:endpoint, expiresAt: Date.now() + 60000 };
        state.lastLoginResponseEndpoint = response.authEndpoint;
        return response;
      }
      if (command === 'register') {
        if (state.registerMode === 'username-taken') throw { kind: 'username-taken' };
        if (state.registerMode === 'username-invalid') throw { kind: 'username-invalid', message: 'Username is rejected by this server.' };
        if (state.registerMode === 'network') throw { kind: 'network' };
        if (state.registerMode === 'rate-limited') throw { kind: 'rate-limited', message: 'server wording' };
        if (state.registerMode === 'create-failed') throw { kind: 'create-failed' };
        if (state.registerMode === 'server') throw { kind: 'server', message: 'internal detail' };
        return { ok: true };
      }
      if (command === 'patch_status') {
        const snapshot = structuredClone(state.patch);
        if (state.patchStatusDelayMs) await new Promise(resolve => setTimeout(resolve, state.patchStatusDelayMs));
        return snapshot;
      }
      if (command === 'reset_patch') { state.patch = { phase:'idle', download_idx:0, patch_idx:0, total_patches:4, bytes_downloaded:0, previous_completed_bytes:0, total_download_bytes:100, is_running:false, is_paused:false, pause_requested:false, is_terminal:false, error:null }; return null; }
      if (['start_patch_download', 'start_local_patch'].includes(command)) {
        if (state.patchStartDelayMs) await new Promise(resolve => setTimeout(resolve, state.patchStartDelayMs));
        if (state.patchStartError) throw new Error(state.patchStartError);
        state.patch = { ...state.patch, phase:'starting', is_install:false, is_running:true, is_paused:false, pause_requested:false, is_terminal:false, error:null };
        return null;
      }
      if (command === 'install_quote') {
        if (state.installQuoteError) throw new Error(state.installQuoteError);
        return structuredClone(state.installQuote);
      }
      if (command === 'install_game') {
        if (state.installStartError) throw new Error(state.installStartError);
        state.installDestination = args.destination;
        state.patch = { ...state.patch, phase:'starting', is_install:true, is_running:true, is_paused:false, pause_requested:false, is_terminal:false, error:null };
        return null;
      }
      if (command === 'cancel_patch' && state.patch.is_running) state.patch = { ...state.patch, phase:'cancelled', is_running:false, is_terminal:true, pause_requested:false };
      if (command === 'pause_patch' && state.patch.is_running) state.patch.pause_requested = true;
      if (command === 'resume_patch' && state.patch.is_running) { state.patch.pause_requested = false; state.patch.is_paused = false; }
      if (command === 'pick_directory') return state.nextDirectory;
      if (command === 'pick_install_dir') {
        if (state.nextInstallDir && state.pickerSelectsFinal) {
          state.patchRequired = false;
          state.noValidInstall = false;
        }
        return state.nextInstallDir;
      }
      if (command === 'detect_game_install_command') return { detected: state.installPath || 'C:/game', source: 'preferences' };
      if (command === 'get_patch_settings') return { ...state.patchSettings };
      if (command === 'config_tool_supported') return true;
      if (command === 'get_launcher_behavior') return { ...state.launcherBehavior };
      if (command === 'get_borderless_monitors') return structuredClone(state.borderlessMonitors);
      if (command === 'set_borderless_monitor') {
        state.borderlessMonitors.selected = args.monitorId;
        return structuredClone(state.borderlessMonitors);
      }
      if (command === 'set_close_on_game_start') {
        state.launcherBehavior.close_on_game_start = args.closeOnGameStart;
        return { ...state.launcherBehavior };
      }
      if (command === 'set_native_resolution_override') {
        state.launcherBehavior.native_resolution_override = args.nativeResolutionOverride;
        return { ...state.launcherBehavior };
      }
      if (command === 'get_game_settings') return structuredClone(state.gameSettings);
      if (command === 'get_object_distance_selection') return state.objectDistanceEnabled ? state.objectDistancePercent : null;
      if (command === 'get_camera_zoom_selection') return state.cameraZoomEnabled ? state.cameraZoomLimit : null;
      if (command === 'set_object_distance_selection') {
        if (state.failObjectDistanceWrite) {
          state.failObjectDistanceWrite = false;
          throw new Error('fixture draw distance write failure');
        }
        state.objectDistanceEnabled = args.percent !== null;
        if (state.objectDistanceEnabled) state.objectDistancePercent = args.percent;
        return args.percent;
      }
      if (command === 'set_camera_zoom_selection') {
        if (state.failCameraZoomWrite) {
          state.failCameraZoomWrite = false;
          throw new Error('fixture camera zoom write failure');
        }
        state.cameraZoomEnabled = args.limit !== null;
        if (state.cameraZoomEnabled) state.cameraZoomLimit = args.limit;
        return args.limit;
      }
      if (command === 'set_game_settings') {
        state.gameSettingsWritesInFlight += 1;
        state.maxGameSettingsWritesInFlight = Math.max(state.maxGameSettingsWritesInFlight, state.gameSettingsWritesInFlight);
        try {
          if (state.gameSettingsWriteDelayMs) await new Promise(resolve => setTimeout(resolve, state.gameSettingsWriteDelayMs));
          if (state.failGameSettingsWrite) {
            state.failGameSettingsWrite = false;
            throw new Error('fixture game settings write failure');
          }
          const resolution = [args.settings.width, args.settings.height];
          if (!state.gameSettings.supported_resolutions.some(candidate => candidate[0] === resolution[0] && candidate[1] === resolution[1])) throw new Error('unsupported fixture resolution');
          if (args.settings.graphics.general_quality < 1 || args.settings.graphics.general_quality > 10) throw new Error('invalid fixture general quality');
          if (args.settings.graphics.background_quality < 1 || args.settings.graphics.background_quality > 5) throw new Error('invalid fixture background quality');
          state.gameSettings.settings = structuredClone(args.settings);
          return structuredClone(state.gameSettings);
        } finally {
          state.gameSettingsWritesInFlight -= 1;
        }
      }
      if (command === 'get_extension_inventory') {
        return structuredClone(state.extensionInventory);
      }
      if (command === 'get_launcher_update_status') return structuredClone(state.launcherUpdateStatus);
      if (command === 'launcher_update_restart_available') return state.launcherUpdateRestartAvailable;
      if (command === 'check_launcher_update') {
        state.launcherUpdateChecks += 1;
        if (state.failLauncherUpdateAction === 'check') throw new Error('fixture launcher update check failure');
        state.launcherUpdateStatus = structuredClone(state.launcherUpdateCheckResult);
        return structuredClone(state.launcherUpdateStatus);
      }
      if (command === 'apply_launcher_update') {
        if (state.failLauncherUpdateAction === 'apply') throw new Error('fixture launcher update apply failure');
        return structuredClone(state.launcherUpdateApplyResult);
      }
      if (command === 'game_repair_status') return structuredClone(state.gameRepairStatus);
      if (command === 'start_game_repair') {
        if (state.gameRepairStartError) throw new Error(state.gameRepairStartError);
        state.gameRepairStatus = { ...state.gameRepairStatus, phase:'verifying', is_running:true, is_terminal:false, can_cancel:true };
        return structuredClone(state.gameRepairStatus);
      }
      if (command === 'pause_game_repair') {
        state.gameRepairStatus.pause_requested = true;
        return structuredClone(state.gameRepairStatus);
      }
      if (command === 'resume_game_repair') {
        state.gameRepairStatus.pause_requested = false;
        state.gameRepairStatus.is_paused = false;
        return structuredClone(state.gameRepairStatus);
      }
      if (command === 'cancel_game_repair') {
        state.gameRepairStatus = { ...state.gameRepairStatus, phase:'cancelled', is_running:false, is_terminal:true, can_cancel:false };
        return structuredClone(state.gameRepairStatus);
      }
      if (command === 'control_window') return null;
      if (command === 'set_addon_enabled') {
        if (state.failAddonWrite) {
          state.failAddonWrite = false;
          throw new Error('fixture addon write failure');
        }
        const addon = state.extensionInventory.addons.find(candidate => candidate.id === args.id);
        if (!addon) throw new Error('addon is not installed');
        addon.enabled = args.enabled;
        addon.status = args.enabled ? 'Enabled' : 'Disabled';
        return structuredClone(state.extensionInventory);
      }
      if (command === 'set_screenshot_enabled') {
        const screenshot = state.extensionInventory.plugins.find(candidate => candidate.id === 'screenshot');
        screenshot.enabled = args.enabled;
        screenshot.status = args.enabled ? 'Enabled' : 'Disabled';
        return structuredClone(state.extensionInventory);
      }
      if (command === 'set_discord_rpc_enabled') {
        const plugin = state.extensionInventory.plugins.find(candidate => candidate.id === 'discord-rpc');
        plugin.enabled = args.enabled;
        plugin.status = args.enabled ? 'Enabled' : 'Disabled';
        return structuredClone(state.extensionInventory);
      }
      if (command === 'set_dat_package_enabled') {
        if (args.id === 'bahamut-dats-overlay') throw new Error('Official overlay is always enabled');
        const packageItem = state.extensionInventory.overlays.find(candidate => candidate.id === args.id);
        if (!packageItem) throw new Error('DAT package is not installed');
        packageItem.enabled = args.enabled;
        const enabledPackages = state.extensionInventory.overlays.filter(candidate => candidate.enabled && candidate.id !== 'bahamut-dats-overlay');
        state.extensionInventory.overlay_conflicts = enabledPackages.length > 1 ? [{ relative_path:'data/2A/08.DAT', package_ids:enabledPackages.map(candidate => candidate.id) }] : [];
        return structuredClone(state.extensionInventory);
      }
      if (command === 'reorder_dat_package') {
        const packages = state.extensionInventory.overlays;
        const index = packages.findIndex(candidate => candidate.id === args.id);
        if (args.id === 'bahamut-dats-overlay' || index < 0 || args.position < 1 || args.position >= packages.length) throw new Error('DAT package cannot move to that position');
        const [packageItem] = packages.splice(index, 1);
        packages.splice(args.position, 0, packageItem);
        return structuredClone(state.extensionInventory);
      }
      if (command === 'get_launcher_log') return { ...state.supportLog };
      if (command === 'create_backup') {
        if (state.backupMode === 'error') throw new Error('fixture backup failure');
        return args.target === 'user-settings' ? 'User Settings and Macros backup created.' : 'Extensions backup created.';
      }
      if (command === 'restore_backup') {
        if (state.backupMode === 'error') throw new Error('fixture restore failure');
        return args.target === 'user-settings' ? 'User Settings and Macros restored.' : 'Extensions restored.';
      }
      return null;
    } } };
  })();`;
}

test('window_config_matches_fixed_shell_contract', () => {
  const config = JSON.parse(readFileSync(TAURI_CONFIG_PATH, 'utf8'));
  const window = config.app.windows.find(candidate => candidate.label === 'main');
  if (!window) throw new Error('main Tauri window is missing');
  if (window.create !== false) throw new Error('main window would bypass the portable WebView data directory');
  if (window.width !== 1280 || window.height !== 800) throw new Error('main window is not 1280x800');
  if (window.decorations !== false || window.resizable !== false || window.maximizable !== false || window.fullscreen !== false) {
    throw new Error('main window is not fixed custom chrome');
  }
  const screensCss = readFileSync(new URL('./css/screens.css', import.meta.url), 'utf8');
  if (!screensCss.includes('.account-page-back:hover { color:var(--control-text); }')) throw new Error('account Back hover must retain the readable control-text token');
});

test('brand_assets_match_the_theme_and_windows_icon_contract', () => {
  const pngInfo = path => {
    const png = readFileSync(path);
    if (png.subarray(0, 8).toString('hex') !== '89504e470d0a1a0a') throw new Error(`${path} has an invalid PNG signature`);
    if (png.readUInt32BE(8) !== 13 || png.subarray(12, 16).toString('ascii') !== 'IHDR') throw new Error(`${path} has an invalid IHDR chunk`);
    return {
      bytes: png,
      width: png.readUInt32BE(16),
      height: png.readUInt32BE(20),
      bitDepth: png[24],
      colorType: png[25],
    };
  };
  const dayBrand = pngInfo(DAY_BRAND_LOGO_PATH);
  const nightBrand = pngInfo(NIGHT_BRAND_LOGO_PATH);
  const icon = pngInfo(ICON_PNG_PATH);
  const iconSource = pngInfo(ICON_SOURCE_PATH);
  if (dayBrand.width !== 512 || dayBrand.height !== 512 || dayBrand.bitDepth !== 8 || dayBrand.colorType !== 6) throw new Error('day brand logo must be a 512px RGBA asset');
  if (nightBrand.width !== 512 || nightBrand.height !== 512 || nightBrand.bitDepth !== 8 || nightBrand.colorType !== 6) throw new Error('night brand logo must be a 512px RGBA asset');
  for (const [name, [path, expectedHash]] of GAMEPAD_ASSETS) {
    const prompt = pngInfo(path);
    const hash = createHash('sha256').update(prompt.bytes).digest('hex');
    if (prompt.width !== 480 || prompt.height !== 480 || prompt.bitDepth !== 8 || prompt.colorType !== 6 || hash !== expectedHash) throw new Error(`${name} is not the exact owner-authorized prompt asset`);
  }
  if (icon.width !== 480 || icon.height !== 480 || icon.bitDepth !== 8 || icon.colorType !== 6) throw new Error('application PNG icon must be a generated 480x480 RGBA body');
  if (iconSource.width !== 512 || iconSource.height !== 512 || iconSource.bitDepth !== 8 || iconSource.colorType !== 6) throw new Error('application icon source must be a 512px RGBA asset');

  const ico = readFileSync(ICON_ICO_PATH);
  if (ico.readUInt16LE(0) !== 0 || ico.readUInt16LE(2) !== 1) throw new Error('application ICO header is invalid');
  const count = ico.readUInt16LE(4);
  if (count !== 6) throw new Error(`application ICO image count drifted: ${count}`);
  const directoryEnd = 6 + count * 16;
  const sizes = Array.from({ length: count }, (_, index) => {
    const entry = 6 + index * 16;
    const width = ico[entry] || 256;
    const height = ico[entry + 1] || 256;
    const planes = ico.readUInt16LE(entry + 4);
    const bitCount = ico.readUInt16LE(entry + 6);
    const byteLength = ico.readUInt32LE(entry + 8);
    const offset = ico.readUInt32LE(entry + 12);
    if (width !== height || planes !== 0 || bitCount !== 32) throw new Error(`application ICO entry ${index} metadata is invalid`);
    if (byteLength <= 8 || offset < directoryEnd || offset + byteLength > ico.length) throw new Error(`application ICO entry ${index} bounds are invalid`);
    if (ico.subarray(offset, offset + 8).toString('hex') !== '89504e470d0a1a0a') throw new Error(`application ICO entry ${index} is not PNG encoded`);
    return width;
  });
  if (sizes[0] !== 256) throw new Error('Tauri uses the first ICO image for the window; it must be 256px to avoid upscaling');
  sizes.sort((left, right) => left - right);
  if (sizes.join(',') !== '16,24,32,48,64,256') throw new Error(`application ICO sizes drifted: ${sizes.join(',')}`);
  const interHash = createHash('sha256').update(readFileSync(INTER_FONT_PATH)).digest('hex');
  const cinzelHash = createHash('sha256').update(readFileSync(CINZEL_FONT_PATH)).digest('hex');
  if (interHash !== '0be2399ea925f1f83ff974764761da9860ec50742ed29a5d4c1ffd0c5c7ac3a8') throw new Error('Inter is not the exact Avalon launcher asset');
  if (cinzelHash !== 'f4d83d34d1f6c741193e4acf4b3dff9531e5a67b6aa65228d00a7db72a4e0f34') throw new Error('Cinzel is not the exact Avalon launcher asset');
});

async function evaluate(devtools, expression) {
  const result = await devtools.send('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
  }
  return result.result?.value;
}

async function exposeFrontendModules(devtools) {
  await evaluate(devtools, `(async () => {
    const [settings, homeModule, main, runtime, extensions, launcherUpdates, gameRepair] = await Promise.all([
      import('/js/settings.js'),
      import('/js/home.js'),
      import('/js/main.js'),
      import('/js/runtime.js'),
      import('/js/extensions.js'),
      import('/js/launcher-updates.js'),
      import('/js/game-repair.js'),
    ]);
    window.__launcherModules = { main, homeModule, extensions, launcherUpdates, gameRepair };
    window.__launcherHome = runtime.home;
    window.__launcherSettings = runtime.settingsState;
    Object.assign(window, {
      renderGameSettings:settings.renderGameSettings,
      renderBorderlessMonitorSettings:settings.renderBorderlessMonitorSettings,
      queueBorderlessMonitorUpdate:settings.queueBorderlessMonitorUpdate,
      queueGameSettingsUpdate:settings.queueGameSettingsUpdate,
      queueLauncherBehaviorUpdate:settings.queueLauncherBehaviorUpdate,
      hydrateSettings:settings.hydrateSettings,
      refreshHomeStatus:homeModule.refreshHomeStatus,
      refreshPatchSnapshot:homeModule.refreshPatchSnapshot,
      refreshGameStatus:homeModule.refreshGameStatus,
      resetPatchStatus:homeModule.resetPatchStatus,
      chooseInstallFolder:homeModule.chooseInstallFolder,
      startInstall:homeModule.startInstall,
      hydrateExtensions:extensions.hydrateExtensions,
      hydrateLauncherUpdates:launcherUpdates.hydrateLauncherUpdates,
      handleLauncherUpdateAction:launcherUpdates.handleLauncherUpdateAction,
      refreshLauncherUpdateAvailability:launcherUpdates.refreshLauncherUpdateAvailability,
      startBackgroundLauncherUpdateCheck:launcherUpdates.startBackgroundLauncherUpdateCheck,
      refreshGameRepairStatus:gameRepair.refreshGameRepairStatus,
      startGameRepair:gameRepair.startGameRepair,
      controlGameRepair:gameRepair.controlGameRepair,
      startUpdate:homeModule.startUpdate,
      restoreSession:homeModule.restoreSession,
      activateSettingsTab:main.activateSettingsTab,
      focusAdjacentRegion:main.focusAdjacentRegion,
      readControllerActions:main.readControllerActions,
      runControllerAction:main.runControllerAction,
      selectActiveGamepad:main.selectActiveGamepad,
      trackMouseMovement:main.trackMouseMovement,
      updateControllerLabel:main.updateControllerLabel,
      updateGamepadPrompts:main.updateGamepadPrompts,
      controllerStates:main.controllerStates,
    });
    return true;
  })()`);
}

test('home_auth_and_terminal_patch_states', async t => {
  const server = createServer(serveUiFixture);
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const httpPort = server.address().port;
  const debugPort = await freePort();
  const profileDir = mkdtempSync(join(tmpdir(), 'bahamut-launcher-browser-'));
  const browser = spawn(browserPath(), [
    '--headless=new', '--disable-gpu', '--no-sandbox',
    `--remote-debugging-port=${debugPort}`, `--user-data-dir=${profileDir}`,
    'about:blank',
  ], { stdio: 'ignore' });
  let devtools = null;

  t.after(async () => {
    if (devtools) {
      await devtools.send('Browser.close').catch(() => {});
      devtools.socket.close();
    }
    await stopChild(browser);
    await delay(1000);
    await new Promise(resolve => server.close(resolve));
    removeTempRoot(profileDir);
  });

  devtools = await connectDevTools(debugPort);
  await devtools.send('Page.addScriptToEvaluateOnNewDocument', { source: evaluateScript() });
  await devtools.send('Page.enable');
  await devtools.send('Runtime.enable');
  await devtools.send('Emulation.setDeviceMetricsOverride', {
    width: 1280,
    height: 800,
    screenWidth: 1280,
    screenHeight: 800,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await devtools.send('Page.navigate', { url: `http://127.0.0.1:${httpPort}/index.html` });
  await evaluate(devtools, `new Promise(resolve => {
    const check = () => document.querySelector('#home-layout')?.dataset.lifecycle ? resolve(true) : setTimeout(check, 10);
    check();
  })`);
  await exposeFrontendModules(devtools);
  await delay(100);

  const assertUi = expression => evaluate(devtools, `(async () => { ${expression} })()`);

  await assertUi(`
    const state = window.__launcherBrowserState;
    const topbar = document.querySelector('.topbar').getBoundingClientRect();
    const stage = document.querySelector('.stage').getBoundingClientRect();
    const homeLayout = document.querySelector('#home-layout').getBoundingClientRect();
    const homeLeft = document.querySelector('.home-left').getBoundingClientRect();
    const homeRight = document.querySelector('.home-right').getBoundingClientRect();
    const session = document.querySelector('#session-card').getBoundingClientRect();
    const primary = document.querySelector('#home-primary').getBoundingClientRect();
    const newsPanel = document.querySelector('.news-panel').getBoundingClientRect();
    const newsPanelStyle = getComputedStyle(document.querySelector('.news-panel'));
    const newsListStyle = getComputedStyle(document.querySelector('.news-list'));
    const hasOuterShadow = boxShadow => boxShadow.split(/,(?![^\\(]*\\))/).some(layer => {
      const normalized = layer.trim();
      return normalized !== 'none' && !normalized.includes('inset') && !normalized.includes('rgba(0, 0, 0, 0)');
    });
    const resolveTokenColor = token => {
      const probe = document.createElement('span');
      probe.style.color = 'var(' + token + ')';
      document.body.append(probe);
      const color = getComputedStyle(probe).color;
      probe.remove();
      return color;
    };
    const parseColor = value => {
      const channels = value.match(/[\\d.]+/g);
      if (!channels) throw new Error('could not parse computed color: ' + JSON.stringify(value));
      return channels.map(Number);
    };
    const compositeOverWhite = value => {
      const [red, green, blue, alpha = 1] = parseColor(value);
      return [red, green, blue].map(channel => channel * alpha + 255 * (1 - alpha));
    };
    const luminance = channels => channels
      .map(channel => channel / 255)
      .map(channel => channel <= .04045 ? channel / 12.92 : ((channel + .055) / 1.055) ** 2.4)
      .reduce((sum, channel, index) => sum + channel * [.2126, .7152, .0722][index], 0);
    const contrast = (foreground, background) => {
      const values = [luminance(foreground), luminance(background)].sort((left, right) => right - left);
      return (values[0] + .05) / (values[1] + .05);
    };
    const requiredThemeTokens = [
      '--font-body','--font-display','--font-mono','--font-console',
      '--panel','--panel-compact','--panel-sunk','--border','--border-accent',
      '--card-radius','--card-radius-inner','--card-shadow','--focus',
      '--control-subtle-bg','--control-secondary-bg','--accent-soft-bg','--accent-emphasis-bg',
      '--action-fill-start','--action-fill-end','--on-accent',
      '--disabled-bg','--disabled-border','--disabled-text','--disabled-opacity',
      '--help-log-bg','--help-log-text',
    ];
    const assertThemeTokensResolve = (style, themeName) => {
      const missing = requiredThemeTokens.filter(token => !style.getPropertyValue(token).trim());
      if (missing.length) throw new Error(themeName + ' semantic tokens are missing: ' + missing.join(','));
    };
    const near = (actual, expected) => Math.abs(actual - expected) < .1;
    if (!near(topbar.height, 77)) throw new Error('topbar is not 77px tall');
    if (!near(stage.x, 0) || !near(stage.y, 77) || !near(stage.width, 1280) || !near(stage.height, 723)) throw new Error('stage shell geometry drifted');
    if (!near(homeLayout.x, 30) || !near(homeLayout.y, 101) || !near(homeLayout.width, 1220) || !near(homeLayout.height, 669)) throw new Error('Home stage geometry drifted');
    if (!near(homeLeft.width, 430) || !near(homeRight.width, 766) || !near(homeRight.x - homeLeft.right, 24)) throw new Error('Home columns are not 430px + 24px + 766px');
    if (!near(session.height, 402) || !near(primary.height, 68)) throw new Error('Home account/play geometry drifted');
    if (newsPanel.height < 190 || newsPanel.height > 260 || newsPanelStyle.flexGrow !== '0' || newsListStyle.maxHeight !== '416px') throw new Error('Recent News is not compact with a bounded scrolling feed: ' + newsPanel.height);
    state.sessionCardHeight = session.height;
    if (getComputedStyle(document.body).minWidth !== '0px' || getComputedStyle(document.body).minHeight !== '0px') throw new Error('body still prevents smaller work-area sizing');
    await document.fonts.ready;
    await Promise.all([
      document.fonts.load('15px Inter', 'Bahamut'),
      document.fonts.load('700 15px Cinzel', 'Bahamut'),
      document.fonts.load('500 15px "JetBrains Mono"', 'Bahamut'),
    ]);
    const fontChecks = {
      body: document.fonts.check('15px Inter', 'Bahamut'),
      display: document.fonts.check('700 15px Cinzel', 'Bahamut'),
      mono: document.fonts.check('500 15px "JetBrains Mono"', 'Bahamut'),
    };
    if (Object.values(fontChecks).some(loaded => !loaded)) throw new Error('bundled semantic font families did not load: ' + JSON.stringify(fontChecks));
    state.newsPanelHeight = document.querySelector('.news-panel').getBoundingClientRect().height;
    const brand = document.querySelector('.brand-title');
    if (brand.textContent !== 'BAHAMUT' || brand.querySelectorAll(':scope > span').length !== 1 || document.querySelector('.brand-subtitle')) throw new Error('brand is not the BAHAMUT wordmark');
    const brandLogo = document.querySelector('.brand img');
    const darkStyle = getComputedStyle(document.body);
    assertThemeTokensResolve(darkStyle, 'night');
    if ([...document.querySelectorAll('.glass-panel')].some(panel => hasOuterShadow(getComputedStyle(panel).boxShadow))) throw new Error('night glass panels retained unintended outer elevation');
    const darkDisabledChoice = getComputedStyle(document.querySelector('.gamepad-card--stub .choice-option'));
    if (darkDisabledChoice.backgroundColor !== resolveTokenColor('--disabled-bg') || darkDisabledChoice.borderColor !== resolveTokenColor('--disabled-border') || darkDisabledChoice.color !== resolveTokenColor('--disabled-text')) throw new Error('night disabled controls bypassed semantic tokens');
    const hero = document.querySelector('.launcher-shell');
    const heroStyle = getComputedStyle(hero, '::before');
    const imageDimensions = await Promise.all(['assets/background-day.png', 'assets/background-night.jpg'].map(source => new Promise((resolve, reject) => {
      const image = new Image();
      image.onload = () => resolve([image.naturalWidth, image.naturalHeight]);
      image.onerror = () => reject(new Error('failed to decode ' + source));
      image.src = source;
    })));
    if (imageDimensions[0].join('x') !== '1920x1080' || imageDimensions[1].join('x') !== '800x450') throw new Error('theme background dimensions drifted: ' + JSON.stringify(imageDimensions));
    if (darkStyle.getPropertyValue('--accent').trim() !== '#c52a32' || darkStyle.getPropertyValue('--accent-text').trim() !== '#f6d89a' || darkStyle.getPropertyValue('--bg').trim() !== '#190a0d') throw new Error('night palette drifted');
    if (resolveTokenColor('--text') !== 'rgb(223, 195, 191)' || resolveTokenColor('--control-text') !== 'rgb(223, 195, 191)' || resolveTokenColor('--on-accent') !== 'rgb(25, 10, 13)') throw new Error('night text or button ink bypassed the softened palette');
    const nightActionInk = parseColor(resolveTokenColor('--on-accent')).slice(0, 3);
    if (['--action-fill-start','--action-fill-end'].some(token => contrast(nightActionInk, parseColor(resolveTokenColor(token)).slice(0, 3)) < 4.5)) throw new Error('night filled-action labels fail normal-text contrast');
    const nightCopyLogsStyle = getComputedStyle(document.querySelector('[data-help-action="copy"]'));
    if (!nightCopyLogsStyle.backgroundImage.includes('rgb(232, 109, 103)') || !nightCopyLogsStyle.backgroundImage.includes('rgb(217, 81, 80)') || nightCopyLogsStyle.color !== 'rgb(25, 10, 13)') throw new Error('Copy Logs bypassed the night filled-action tokens');
    if (!heroStyle.backgroundImage.includes('background-night.jpg')) throw new Error('night mode did not load the supplied night background');
    if (heroStyle.filter !== 'none' || heroStyle.transform !== 'none') throw new Error('theme background source pixels have a direct CSS filter or transform');
    if (getComputedStyle(hero, '::after').backgroundColor !== 'rgba(25, 10, 13, 0.48)') throw new Error('night mode did not apply its scene filter');
    if (!brandLogo.src.endsWith('/assets/brand-logo-night.png') || getComputedStyle(brandLogo).filter.includes('hue-rotate')) throw new Error('night mode did not load its accepted brand logo directly');
    const legacyPalette = {
      bodyAttribute: document.body.getAttribute('data-palette'),
      control: Boolean(document.querySelector('[data-palette-choice], .palette-row, .palette-button')),
    };
    if (legacyPalette.bodyAttribute !== null || legacyPalette.control) throw new Error('legacy accent customization is still active: ' + JSON.stringify(legacyPalette));
    const controls = [...document.querySelectorAll('.topbar-actions button')];
    const order = controls.map(button => button.dataset.utility ? 'utility:' + button.dataset.utility : button.dataset.link ? 'link:' + button.dataset.link : 'window:' + button.dataset.window).join(',');
    if (order !== 'utility:gamepad,utility:help,link:github,link:wiki,utility:theme,window:minimize,window:close') throw new Error('topbar control order drifted: ' + order);
    const icons = controls.map(button => button.querySelector('[data-icon]:not(.theme-icon-night)')?.dataset.icon).join(',');
    if (icons !== 'gamepad-2,circle-question-mark,github,book-open,theme-day,minus,x') throw new Error('topbar icon semantics drifted: ' + icons);
    const theme = document.querySelector('[data-utility="theme"]');
    theme.click();
    const lightStyle = getComputedStyle(document.body);
    assertThemeTokensResolve(lightStyle, 'day');
    if (document.body.dataset.theme !== 'light' || theme.getAttribute('aria-label') !== 'Use night theme' || getComputedStyle(theme.querySelector('.theme-icon-day')).display !== 'none' || getComputedStyle(theme.querySelector('.theme-icon-night')).display === 'none') throw new Error('day theme did not show the night-theme icon');
    if (lightStyle.getPropertyValue('--accent').trim() !== '#39c8f0' || lightStyle.getPropertyValue('--accent-text').trim() !== '#bceeff' || lightStyle.getPropertyValue('--bg').trim() !== '#087faf') throw new Error('day palette drifted');
    if (resolveTokenColor('--text') !== 'rgb(237, 248, 251)') throw new Error('day primary text returned to pure white');
    const dayActionInk = parseColor(resolveTokenColor('--on-accent')).slice(0, 3);
    if (['--action-fill-start','--action-fill-end'].some(token => contrast(dayActionInk, parseColor(resolveTokenColor(token)).slice(0, 3)) < 4.5)) throw new Error('day filled-action labels fail normal-text contrast');
    const dayCopyLogsStyle = getComputedStyle(document.querySelector('[data-help-action="copy"]'));
    if (!dayCopyLogsStyle.backgroundImage.includes('rgb(120, 221, 248)') || !dayCopyLogsStyle.backgroundImage.includes('rgb(57, 200, 240)') || dayCopyLogsStyle.color !== 'rgb(5, 48, 66)') throw new Error('Copy Logs bypassed the day filled-action tokens');
    const dayPanel = compositeOverWhite(lightStyle.getPropertyValue('--panel'));
    const lowContrastToken = ['--text','--muted','--muted-2','--accent-text'].find(token => contrast(parseColor(resolveTokenColor(token)).slice(0, 3), dayPanel) < 4.5);
    if (lowContrastToken) throw new Error('day panel text loses contrast over a bright scene: ' + lowContrastToken);
    const lightLoginInput = getComputedStyle(document.querySelector('#login-username'));
    const lightLoginPlaceholder = getComputedStyle(document.querySelector('#login-username'), '::placeholder');
    if (lightLoginInput.backgroundColor !== 'rgba(244, 252, 255, 0.94)' || lightLoginInput.color !== 'rgb(24, 54, 69)' || lightLoginPlaceholder.color !== 'rgb(96, 119, 132)') throw new Error('day Login controls are not using the readable semantic control palette');
    if ([...document.querySelectorAll('.glass-panel')].some(panel => hasOuterShadow(getComputedStyle(panel).boxShadow))) throw new Error('day glass panels retained unintended outer elevation');
    const lightDisabledChoice = getComputedStyle(document.querySelector('.gamepad-card--stub .choice-option'));
    if (lightDisabledChoice.backgroundColor !== resolveTokenColor('--disabled-bg') || lightDisabledChoice.borderColor !== resolveTokenColor('--disabled-border') || lightDisabledChoice.color !== resolveTokenColor('--disabled-text')) throw new Error('day disabled controls bypassed semantic tokens');
    if (getComputedStyle(document.querySelector('.glass-panel')).borderColor !== 'rgba(57, 200, 240, 0.42)') throw new Error('day panels retained the night crimson border');
    if (!getComputedStyle(hero, '::before').backgroundImage.includes('background-day.png')) throw new Error('day mode did not load the supplied day background');
    if (getComputedStyle(hero, '::after').backgroundColor !== 'rgba(8, 127, 175, 0.3)') throw new Error('day mode did not apply the blue scene filter');
    if (!brandLogo.src.endsWith('/assets/brand-logo-day.png') || getComputedStyle(brandLogo).filter.includes('hue-rotate')) throw new Error('day mode did not load its accepted brand logo directly');
    theme.click();
    if (!brandLogo.src.endsWith('/assets/brand-logo-night.png')) throw new Error('night theme return did not restore its accepted brand logo');
    const routeIcons = [...document.querySelectorAll('[data-route]')].map(button => button.querySelector('svg')?.dataset.icon).join(',');
    if (routeIcons !== 'house,plug,settings') throw new Error('route icon semantics drifted: ' + routeIcons);
    if (document.querySelector('[data-action="forgot-password"]') || document.body.textContent.includes('Forgot password')) throw new Error('Forgot Password is still exposed');
    if ([...document.querySelectorAll('.account-links .text-button')].map(button => button.textContent).join(',') !== 'Create account,Profiles') throw new Error('account links drifted');
    if (document.querySelector('#home-eyebrow').textContent || document.querySelector('#home-title').textContent !== 'Account Login' || getComputedStyle(document.querySelector('#home-title')).textAlign !== 'center') throw new Error('logged-out account title does not match Avalon');
    if (document.querySelector('#home-primary').textContent !== 'Play' || !document.querySelector('#home-primary').disabled || !document.querySelector('#login-submit')) throw new Error('Login did not move into the account card');
    if (document.querySelector('#login-submit').textContent !== 'Login' || !document.querySelector('.check-line').textContent.includes('Remember Login')) throw new Error('Login labels drifted');
    const loginForm = document.querySelector('#login-form');
    if (loginForm.autocomplete !== 'off' || document.querySelector('#login-username').autocomplete !== 'off' || document.querySelector('#login-password').autocomplete !== 'off') throw new Error('Account Login still advertises browser autocomplete');
    if (document.querySelector('#login-username').hasAttribute('name') || document.querySelector('#login-password').hasAttribute('name')) throw new Error('Account Login still exposes credential field names to autofill heuristics');
    if ([...document.querySelectorAll('form')].some(form => form.autocomplete !== 'off') || [...document.querySelectorAll('input:not([type="checkbox"]),select')].some(control => control.autocomplete !== 'off')) throw new Error('a launcher form control still advertises browser autocomplete');
    const accountCard = document.querySelector('#session-card');
    const accountStyle = getComputedStyle(accountCard);
    const accountTitleStyle = getComputedStyle(document.querySelector('#home-title'));
    const loginFormStyle = getComputedStyle(document.querySelector('#login-form'));
    const usernameInput = document.querySelector('#login-username').getBoundingClientRect();
    const rememberInput = document.querySelector('#login-remember').getBoundingClientRect();
    if (accountStyle.paddingTop !== '24px' || accountStyle.paddingLeft !== '24px' || accountStyle.overflowY !== 'visible') throw new Error('Account card shell does not match Avalon spacing');
    const loginHeight = document.querySelector('#login-submit').getBoundingClientRect().height;
    const loginBottomGap = document.querySelector('.account-links').getBoundingClientRect().top - document.querySelector('#login-submit').getBoundingClientRect().bottom;
    const loginStyle = getComputedStyle(document.querySelector('#login-submit'));
    const accountLinkStyle = getComputedStyle(document.querySelector('.account-links .text-button'));
    if (accountStyle.gap !== '16px' || accountTitleStyle.marginBottom !== '12px' || accountTitleStyle.lineHeight !== '33px' || loginFormStyle.gap !== '8px' || usernameInput.height < 40 || usernameInput.height > 41 || !near(rememberInput.width, 18) || loginHeight < 41 || loginHeight > 43 || loginBottomGap < 28 || loginBottomGap > 34 || loginStyle.fontSize !== '14px' || loginStyle.lineHeight !== '19.6px' || accountLinkStyle.fontSize !== '12px' || !accountLinkStyle.fontFamily.includes('Inter') || accountLinkStyle.textTransform !== 'uppercase') throw new Error('Account Login rhythm drifted: ' + JSON.stringify({ cardGap:accountStyle.gap, titleMargin:accountTitleStyle.marginBottom, titleLineHeight:accountTitleStyle.lineHeight, formGap:loginFormStyle.gap, input:usernameInput.height, remember:rememberInput.width, login:loginHeight, loginBottomGap, loginFont:loginStyle.fontSize, loginLineHeight:loginStyle.lineHeight, linkFont:accountLinkStyle.fontSize, linkFamily:accountLinkStyle.fontFamily, linkTransform:accountLinkStyle.textTransform }));
    if (getComputedStyle(document.querySelector('#lifecycle-strip')).display !== 'none') throw new Error('installed logged-out state still shows the lifecycle strip');
    if (accountCard.scrollHeight > accountCard.clientHeight) throw new Error('Account Login still has an internal scrollbar');
    if (document.body.textContent.includes('Bahamut network') || document.body.textContent.includes('Sign in to Bahamut')) throw new Error('invented Home labels are still visible');
    if (document.querySelector('.news-header h2').textContent !== 'Recent News') throw new Error('Recent News heading capitalization drifted');
    const newsEntry = document.querySelector('.news-entry');
    const newsMedia = document.querySelector('.news-media').getBoundingClientRect();
    if (newsEntry.querySelector('.news-date').textContent !== 'September 27' || newsEntry.querySelector('.news-title').textContent !== 'Open Beta is Live' || newsEntry.querySelector('.news-copy').textContent !== "Bahamut's open beta is now live!") throw new Error('Open Beta fixture copy drifted');
    if (!near(newsMedia.width, 88) || !newsEntry.querySelector('.news-media img')) throw new Error('Recent News did not render the Avalon media slot');
    const newsLinks = [...document.querySelectorAll('.news-actions [data-link]')].map(button => button.dataset.link + ':' + button.querySelector('svg')?.dataset.icon).join(',');
    if (newsLinks !== 'discord:discord,youtube:youtube') throw new Error('Recent News social links drifted: ' + newsLinks);
    const utilityStyle = getComputedStyle(document.querySelector('[data-utility="gamepad"]'));
    const socialStyle = getComputedStyle(document.querySelector('.news-actions [data-link="discord"]'));
    if (socialStyle.backgroundColor !== utilityStyle.backgroundColor || socialStyle.borderColor !== utilityStyle.borderColor || socialStyle.color !== utilityStyle.color) throw new Error('Recent News social buttons do not share utility-button colors');
    if (document.querySelectorAll('[title]').length) throw new Error('tooltip title overlays are still present');
    document.querySelector('.topbar-actions [data-link="github"]').click();
    document.querySelector('.news-actions [data-link="discord"]').click();
    document.querySelector('.news-actions [data-link="youtube"]').click();
    await new Promise(resolve => setTimeout(resolve, 20));
    const socialTargets = state.calls.filter(call => call.command === 'open_external').map(call => call.args.target).join(',');
    if (socialTargets !== 'github,discord,youtube') throw new Error('Recent News social actions did not reach the native allowlist: ' + socialTargets);
    const discordPath = document.querySelector('[data-link="discord"] [data-icon="discord"] path');
    if (discordPath?.getAttribute('fill') !== 'currentColor' || discordPath.getAttribute('stroke') !== 'none') throw new Error('Discord mark is not fill-only');
    const outlinedSocialPaths = [...document.querySelectorAll('[data-link="github"] path, [data-link="youtube"] path')];
    if (outlinedSocialPaths.some(path => path.hasAttribute('fill') || path.hasAttribute('stroke'))) throw new Error('Discord fill-only correction changed an outlined social glyph');
    if (document.querySelector('.home-meta') || document.querySelector('.home-left').querySelectorAll('#launcher-version').length !== 1 || document.querySelector('#launcher-version').textContent !== 'vbrowser-test') throw new Error('Home footer is not version-only');
    const fit = state.calls.find(call => call.command === 'fit_window_to_work_area');
    if (!fit || fit.args.availableWidth !== 1280 || fit.args.availableHeight !== 800) throw new Error('work-area sizing was not requested at boot');
    state.calls.length = 0;
    document.querySelector('.brand').dispatchEvent(new PointerEvent('pointerdown', { bubbles:true, button:0 }));
    document.querySelector('.topbar-actions').dispatchEvent(new PointerEvent('pointerdown', { bubbles:true, button:0 }));
    document.querySelector('[data-route="home"]').dispatchEvent(new PointerEvent('pointerdown', { bubbles:true, button:0 }));
    document.querySelector('.route-tabs').dispatchEvent(new PointerEvent('pointerdown', { bubbles:true, button:0 }));
    document.querySelector('.utility-cluster').dispatchEvent(new PointerEvent('pointerdown', { bubbles:true, button:0 }));
    document.querySelector('.window-cluster').dispatchEvent(new PointerEvent('pointerdown', { bubbles:true, button:0 }));
    await new Promise(resolve => setTimeout(resolve, 20));
    const drags = state.calls.filter(call => call.command === 'control_window' && call.args.action === 'start-dragging');
    if (drags.length !== 2) throw new Error('dragging did not include both non-interactive topbar regions');
  `);

  await assertUi(`
    const state = window.__launcherBrowserState;
    state.noValidInstall = true;
    state.patchRequired = false;
    localStorage.removeItem('bahamut-install-destination');
    window.__launcherHome.installDestination = '';
    window.__launcherHome.installError = '';
    state.calls.length = 0;
    await window.refreshHomeStatus();
    if (document.querySelector('#home-title').textContent !== 'Account Login' || !document.querySelector('#login-form') || !document.querySelector('#login-submit').disabled) throw new Error('pre-install state replaced or enabled the Account Login card');
    if (document.querySelector('#lifecycle-strip').dataset.mode !== 'location' || document.querySelector('#lifecycle-location').hidden || !document.querySelector('#lifecycle-status').hidden) throw new Error('install location did not move into the Home lifecycle strip');
    const idleInstallStrip = document.querySelector('#lifecycle-strip').getBoundingClientRect();
    if (Math.abs(idleInstallStrip.height - 137) >= .1) throw new Error('install-location strip is not 137px tall');
    state.locationPickerOffset = document.querySelector('.lifecycle-path-picker').getBoundingClientRect().top - document.querySelector('#lifecycle-strip').getBoundingClientRect().top;
    if (Math.abs(document.querySelector('.news-panel').getBoundingClientRect().height - state.newsPanelHeight) >= .1 || Math.abs(document.querySelector('#lifecycle-strip').getBoundingClientRect().bottom - document.querySelector('.home-right').getBoundingClientRect().bottom) >= .1) throw new Error('Recent News grew or the location strip is not bottom-pinned');
    if (document.querySelector('#home-primary').textContent !== 'Install' || document.querySelector('#lifecycle-location-path').textContent !== 'C:/Games/FINAL FANTASY XIV' || document.querySelector('#lifecycle-path-action').textContent !== 'PATH' || document.querySelector('#lifecycle-location-help').textContent !== 'Install the client. Change the destination with PATH.' || document.querySelector('#lifecycle-secondary-action')) throw new Error('Home default install destination and PATH choice drifted');
    const installError = 'Base-game installation is not configured for this build.';
    state.installQuoteError = installError;
    document.querySelector('#home-primary').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (state.calls.some(call => call.command === 'pick_directory') || !state.calls.some(call => call.command === 'install_quote' && call.args.destination === 'C:/Games/FINAL FANTASY XIV') || document.querySelector('#lifecycle-location-help').textContent !== installError || state.calls.some(call => call.command === 'install_game')) throw new Error('default install preflight failure was not surfaced without starting an install');
    state.nextDirectory = 'C:/new-game';
    document.querySelector('#lifecycle-path-action').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (!state.calls.some(call => call.command === 'pick_directory') || state.calls.some(call => call.command === 'install_quote' && call.args.destination === 'C:/new-game') || document.querySelector('#lifecycle-location-path').textContent !== 'C:/new-game') throw new Error('PATH did not replace the default install target without starting installation');
    state.installQuoteError = null;
    document.querySelector('#home-primary').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (document.querySelector('#install-quote-dialog')) throw new Error('install confirmation dialog was not removed');
    if (!state.calls.some(call => call.command === 'install_quote' && call.args.destination === 'C:/new-game') || !state.calls.some(call => call.command === 'install_game' && call.args.destination === 'C:/new-game') || !window.__launcherHome.patchSnapshot.is_running || !document.querySelector('#home-primary').disabled || document.querySelector('#lifecycle-title').textContent !== 'Game Installation' || document.querySelector('#lifecycle-pause').hidden || state.calls.some(call => call.command === 'set_game_dir')) throw new Error('install did not start after inline preflight or its operation was hidden');
    state.patch = { phase:'downloading', download_idx:0, patch_idx:0, total_patches:4, bytes_downloaded:15, previous_completed_bytes:25, total_download_bytes:100, is_running:true, is_paused:false, pause_requested:false, is_terminal:false, error:null };
    await window.refreshPatchSnapshot();
    if (document.querySelector('#lifecycle-copy').textContent !== 'Downloading game files.' || document.querySelector('#lifecycle-metric').textContent !== '40 B / 100 B' || document.querySelector('#home-layout').dataset.operationActive !== 'true') throw new Error('install download progress did not use real byte counters');
    const activeInstallStrip = document.querySelector('#lifecycle-strip').getBoundingClientRect();
    if (Math.abs(activeInstallStrip.height - idleInstallStrip.height) >= .1 || Math.abs(activeInstallStrip.top - idleInstallStrip.top) >= .1) throw new Error('install strip moved or resized when download started');
    state.patch = { ...state.patch, phase:'extracting', patch_idx:2 };
    await window.refreshPatchSnapshot();
    if (document.querySelector('#lifecycle-metric').textContent !== 'File 3 of 4' || document.querySelector('#lifecycle-progress-text').textContent !== '50%') throw new Error('install extraction did not show file progress');
    const strip = document.querySelector('#lifecycle-strip');
    const actions = document.querySelector('#lifecycle-actions');
    const track = document.querySelector('#lifecycle-progress .progress-track');
    const percent = document.querySelector('#lifecycle-progress-text');
    const layout = () => [strip.getBoundingClientRect().width, strip.getBoundingClientRect().height, actions.getBoundingClientRect().left, track.getBoundingClientRect().width];
    percent.textContent = '0%';
    const atZero = layout();
    percent.textContent = '100%';
    const atHundred = layout();
    if (atZero.some((value, index) => Math.abs(value - atHundred[index]) > .1)) throw new Error('progress percentage shifted the install strip or controls');
    state.patch = { ...state.patch, phase:'validating-files', patch_idx:2 };
    await window.refreshPatchSnapshot();
    if (document.querySelector('#lifecycle-copy').textContent !== 'Checking the installed client before finishing.') throw new Error('final install check was labeled as repair');
    document.querySelector('#lifecycle-cancel').click();
    await window.refreshPatchSnapshot();
    if (document.querySelector('#lifecycle-strip').dataset.mode !== 'location' || !document.querySelector('#lifecycle-location-help').textContent.includes('installation was cancelled') || document.querySelector('#lifecycle-path-action').textContent !== 'PATH') throw new Error('cancel did not leave actionable installation recovery');
    const recoveryInstallStrip = document.querySelector('#lifecycle-strip').getBoundingClientRect();
    if (Math.abs(recoveryInstallStrip.height - idleInstallStrip.height) >= .1 || Math.abs(recoveryInstallStrip.top - idleInstallStrip.top) >= .1) throw new Error('install strip moved or resized during recovery');
    const recoveredDestination = localStorage.getItem('bahamut-install-destination');
    window.__launcherHome.installDestination = '';
    window.__launcherHome.patchSnapshot = { phase:'idle', is_running:false, is_paused:false, pause_requested:false, is_terminal:false };
    window.__launcherHome.installDestination = localStorage.getItem('bahamut-install-destination') || '';
    await window.refreshHomeStatus();
    if (recoveredDestination !== 'C:/new-game' || window.__launcherHome.installDestination !== 'C:/new-game' || document.querySelector('#home-primary').textContent !== 'Retry Install') throw new Error('reopening the launcher did not recover the saved destination hint');
    state.installQuoteError = 'The required base manifest is unavailable.';
    document.querySelector('#home-primary').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (document.querySelector('#lifecycle-location-help').textContent !== state.installQuoteError || document.querySelector('#lifecycle-path-action').textContent !== 'PATH') throw new Error('a fresh install preflight failure was hidden behind the previous terminal state');
    state.installQuoteError = null;
    state.installStartError = 'The installer could not start.';
    document.querySelector('#home-primary').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (state.calls.filter(call => call.command === 'install_game').length !== 2 || document.querySelector('#lifecycle-location-help').textContent !== state.installStartError || document.querySelector('#lifecycle-path-action').textContent !== 'PATH') throw new Error('a fresh installer start failure was hidden behind the previous cancellation');
    state.installStartError = null;
    state.nextDirectory = 'C:/alternate-game';
    document.querySelector('#lifecycle-path-action').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (document.querySelector('#lifecycle-location-path').textContent !== 'C:/alternate-game') throw new Error('PATH did not select the new destination');
    document.querySelector('#home-primary').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (state.calls.filter(call => call.command === 'install_game').length !== 3 || !state.calls.some(call => call.command === 'install_game' && call.args.destination === 'C:/alternate-game')) throw new Error('new destination was not used for the recovered install');
    state.noValidInstall = false;
    state.patch = { ...state.patch, phase:'done', is_running:false, is_paused:false, pause_requested:false, is_terminal:true, error:null };
    await window.refreshPatchSnapshot();
    if (localStorage.getItem('bahamut-install-destination') || document.querySelector('#home-layout').dataset.lifecycle !== 'logged-out') throw new Error('successful installation did not select the client and clear its recovery hint');
  `);

  await assertUi(`
    const state = window.__launcherBrowserState;
    state.patchRequired = false;
    state.calls.length = 0;
    await window.refreshHomeStatus();
    state.mode = 'invalid';
    document.querySelector('#login-form').dispatchEvent(new Event('submit', { bubbles:true, cancelable:true }));
    await new Promise(resolve => setTimeout(resolve, 50));
    if (document.querySelector('#login-alert').textContent !== 'Incorrect username or password.') throw new Error('invalid credentials copy drifted from Avalon');
    const alert = document.querySelector('#login-alert');
    alert.textContent = 'A server supplied a deliberately long login error. '.repeat(30);
    const card = document.querySelector('#session-card');
    if (alert.scrollHeight <= alert.clientHeight || alert.getBoundingClientRect().bottom > card.getBoundingClientRect().bottom || card.scrollHeight > card.clientHeight) throw new Error('long login errors escape the fixed account card');
    alert.textContent = '';
  `);

  await assertUi(`
    const state = window.__launcherBrowserState;
    state.mode = 'rate';
    document.querySelector('#login-form').dispatchEvent(new Event('submit', { bubbles:true, cancelable:true }));
    await new Promise(resolve => setTimeout(resolve, 50));
    if (!document.querySelector('#login-submit').disabled) throw new Error('rate limit did not disable login');
    if (!document.querySelector('#login-alert').textContent.includes('2 seconds')) throw new Error('retry countdown was not rendered');
    await new Promise(resolve => setTimeout(resolve, 2200));
    if (document.querySelector('#login-submit').disabled) throw new Error('retry countdown did not re-enable login');
  `);

  await assertUi(`
    const state = window.__launcherBrowserState;
    const savedSession = {
      username:'orphaned',
      token:'0123456789abcdef0123456789abcdef0123456789abcdef01234567',
      expiresAt:Date.now() + 60000,
      server:'Removed Profile',
      authEndpoint:'https://auth.example.test/api/v1/',
    };
    const unbound = { ...savedSession };
    delete unbound.authEndpoint;
    const validationsBefore = state.calls.filter(call => call.command === 'validate_session').length;
    localStorage.setItem('bahamut-session', JSON.stringify(unbound));
    await window.restoreSession();
    if (localStorage.getItem('bahamut-session') || state.calls.filter(call => call.command === 'validate_session').length !== validationsBefore) throw new Error('unbound saved session was validated');
    state.sessionValidation = 'missing-profile';
    localStorage.setItem('bahamut-session', JSON.stringify(savedSession));
    await window.restoreSession();
    await window.refreshHomeStatus();
    if (localStorage.getItem('bahamut-session') || document.querySelector('#home-layout').dataset.lifecycle !== 'logged-out' || !document.querySelector('#login-form')) throw new Error('missing saved profile did not return to login');
    state.mode = 'invalid';
    document.querySelector('#login-username').value = 'aesh';
    document.querySelector('#login-password').value = 'password';
    document.querySelector('#login-form').dispatchEvent(new Event('submit', { bubbles:true, cancelable:true }));
    await new Promise(resolve => setTimeout(resolve, 50));
    const fallbackLogin = state.calls.filter(call => call.command === 'login').at(-1);
    if (!fallbackLogin || fallbackLogin.args.server !== state.serverSettings.selected_server) throw new Error('missing saved profile did not select the current server for login');

    state.sessionValidation = 'unknown';
    savedSession.server = state.serverSettings.selected_server;
    localStorage.setItem('bahamut-session', JSON.stringify(savedSession));
    await window.restoreSession();
    await window.refreshHomeStatus();
    if (!localStorage.getItem('bahamut-session') || document.querySelector('#home-layout').dataset.lifecycle !== 'ready') throw new Error('inconclusive session validation discarded a retained login');
    document.querySelector('[data-action="logout"]').click();
    await new Promise(resolve => setTimeout(resolve, 50));

    const revoke = state.calls.filter(call => call.command === 'logout').at(-1);
    if (revoke.args.authEndpoint !== savedSession.authEndpoint) throw new Error('logout lost the issuing endpoint');
    state.sessionValidation = 'endpoint-mismatch';
    localStorage.setItem('bahamut-session', JSON.stringify(savedSession));
    await window.restoreSession();
    await window.refreshHomeStatus();
    if (localStorage.getItem('bahamut-session') || document.querySelector('#home-layout').dataset.lifecycle !== 'logged-out') throw new Error('changed auth endpoint retained the session');

    state.sessionValidation = 'invalid';
    state.mode = 'success';
    state.loginGate = true;
    state.loginStarted = false;
    document.querySelector('#login-username').value = 'aesh';
    document.querySelector('#login-password').value = 'password';
    document.querySelector('#login-form').dispatchEvent(new Event('submit', { bubbles:true, cancelable:true }));
    await new Promise(resolve => setTimeout(resolve, 10));
    if (!state.loginStarted) throw new Error('login fixture did not remain pending');
    if (state.lastLoginRecipient !== 'http://127.0.0.1:8080/api/v1/') throw new Error('login did not capture the selected profile endpoint');
    document.querySelector('[data-action="open-profiles"]').click();
    await new Promise(resolve => setTimeout(resolve, 10));
    const profileSelect = document.querySelector('#profile-server-select');
    profileSelect.value = 'Bahamut';
    profileSelect.dispatchEvent(new Event('change', { bubbles:true }));
    await new Promise(resolve => setTimeout(resolve, 10));
    if (state.serverSettings.selected_server !== 'Bahamut') throw new Error('profile selection did not change while login was pending');
    document.querySelector('[data-action="close-profiles"]').click();
    const credentialUseCommands = ['validate_session', 'logout', 'launch_game'];
    const usesBeforeCompletion = state.calls.filter(call => credentialUseCommands.includes(call.command) || (call.command === 'get_home_status' && call.args.authenticated)).length;
    state.finishLogin();
    await new Promise(resolve => setTimeout(resolve, 50));
    if (state.lastLoginResponseEndpoint !== 'http://127.0.0.1:8080/api/v1/') throw new Error('login response was rebound to the later profile selection');
    if (localStorage.getItem('bahamut-session') || document.querySelector('#home-layout').dataset.lifecycle !== 'logged-out') throw new Error('stale login completion retained a session');
    if (!document.querySelector('#login-alert').textContent.includes('changed during login')) throw new Error('stale login completion was not reported');
    const usesAfterCompletion = state.calls.filter(call => credentialUseCommands.includes(call.command) || (call.command === 'get_home_status' && call.args.authenticated)).length;
    if (usesAfterCompletion !== usesBeforeCompletion) throw new Error('stale login token was used after profile selection changed');
    savedSession.authEndpoint = state.lastLoginRecipient;
    document.querySelector('[data-action="open-profiles"]').click();
    const resetProfileSelect = document.querySelector('#profile-server-select');
    resetProfileSelect.value = 'Local';
    resetProfileSelect.dispatchEvent(new Event('change', { bubbles:true }));
    await new Promise(resolve => setTimeout(resolve, 10));
    document.querySelector('[data-action="close-profiles"]').click();
    state.mode = 'success';
    document.querySelector('#login-form').dispatchEvent(new Event('submit', { bubbles:true, cancelable:true }));
    await new Promise(resolve => setTimeout(resolve, 50));
    if (document.querySelector('#home-layout').dataset.lifecycle !== 'ready') throw new Error('authenticated ready state was not shown');
    if (JSON.parse(localStorage.getItem('bahamut-session')).authEndpoint !== savedSession.authEndpoint) throw new Error('remembered login lost the backend endpoint');
    if (document.querySelector('#home-title').textContent !== 'Account Login' || !document.querySelector('.account-ready-name') || document.querySelector('.account-ready-name').textContent !== 'aesh') throw new Error('authenticated account card did not retain the account login presentation');
    if (document.querySelector('.account-ready img,.account-ready [class*="character"],.account-ready [class*="portrait"]')) throw new Error('authenticated card exposed character identity instead of the account name');
    if (document.querySelector('[data-action="logout"]').textContent !== 'Logout' || getComputedStyle(document.querySelector('#lifecycle-strip')).display !== 'none') throw new Error('ready state logout or lifecycle visibility drifted');
    const readyCard = document.querySelector('#session-card');
    let launchAlert = document.querySelector('#launch-alert');
    const readyLogout = document.querySelector('[data-action="logout"]');
    const readyCardHeight = readyCard.getBoundingClientRect().height;
    const readyLogoutTop = readyLogout.getBoundingClientRect().top;
    if (Math.abs(readyCardHeight - state.sessionCardHeight) >= .1) throw new Error('logged-in and logged-out account card sizes differ');
    if (Math.abs(launchAlert.getBoundingClientRect().height - 60) >= .1 || Math.abs(readyLogout.getBoundingClientRect().top - launchAlert.getBoundingClientRect().bottom - 6) >= .1) throw new Error('launch error did not reserve the fixed gap above Logout');
    launchAlert.textContent = 'Client extensions were not loaded: bootstrap helper failed (RenderBoundaryFailed): target_terminated pid=15148 stage=8 owner_path="C:\\WINDOWS\\SYSTEM32\\apphelp.dll"';
    await new Promise(resolve => requestAnimationFrame(resolve));
    if (Math.abs(readyCard.getBoundingClientRect().height - readyCardHeight) >= .1 || Math.abs(readyLogout.getBoundingClientRect().top - readyLogoutTop) >= .1 || launchAlert.scrollHeight > launchAlert.clientHeight || Math.abs(readyLogout.getBoundingClientRect().top - launchAlert.getBoundingClientRect().bottom - 6) >= .1) throw new Error('launch error changed the account card or Logout geometry');
    launchAlert.textContent = '';
    if (Math.abs(document.querySelector('.news-panel').getBoundingClientRect().height - state.newsPanelHeight) >= .1) throw new Error('Recent News grew when the lifecycle strip stayed hidden');
    state.gameStatusDelayMs = 40;
    state.launchDelayMs = 80;
    let play = document.querySelector('#home-primary');
    const staleGameStatus = window.refreshHomeStatus();
    await new Promise(resolve => setTimeout(resolve, 5));
    play.click();
    play.click();
    await new Promise(resolve => setTimeout(resolve, 20));
    const launchCalls = state.calls.filter(call => call.command === 'launch_game');
    const launch = launchCalls[0];
    if (!launch || Object.keys(launch.args).sort().join(',') !== 'authEndpoint,server,token' || launch.args.authEndpoint !== savedSession.authEndpoint) throw new Error('Play did not use the endpoint-bound launch contract');
    if (launchCalls.length !== 1 || !play.disabled) throw new Error('Play did not reserve one launch while startup was pending');
    await staleGameStatus;
    play = document.querySelector('#home-primary');
    launchAlert = document.querySelector('#launch-alert');
    if (!play.disabled) throw new Error('a stale game status re-enabled Play during startup');
    await new Promise(resolve => setTimeout(resolve, 60));
    if (!play.disabled || !state.gameRunning) throw new Error('Play re-enabled while the launched client was active');
    state.gameRunning = false;
    state.gameStatusDelayMs = 0;
    await window.refreshGameStatus();
    if (play.disabled || play.textContent !== 'Play') throw new Error('client exit did not restore the unchanged Play action');
    state.launchDelayMs = 0;
    state.launchError = { kind:'extensions', message:'fixture launch failure' };
    const launchCountBeforeFailure = state.calls.filter(call => call.command === 'launch_game').length;
    play.click();
    await new Promise(resolve => setTimeout(resolve, 20));
    const launchCountAfterFailure = state.calls.filter(call => call.command === 'launch_game').length;
    if (play.disabled || !launchAlert.textContent.includes('fixture launch failure')) throw new Error('launch failure did not release Play (disabled=' + play.disabled + ', alert=' + launchAlert.textContent + ', launches=' + launchCountBeforeFailure + '->' + launchCountAfterFailure + ')');
    state.launchError = { kind:'session-endpoint-changed', message:'Server address changed' };
    play.click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (localStorage.getItem('bahamut-session') || !document.querySelector('#login-form') || !document.querySelector('#login-alert').textContent.includes('Log in again')) throw new Error('launch endpoint mismatch did not clear the session');
    state.launchError = null;
  `);

  await assertUi(`
    const state = window.__launcherBrowserState;
    await window.restoreSession();
    await new Promise(resolve => setTimeout(resolve, 50));
    state.patchRequired = true;
    state.patch = { phase:'idle', download_idx:0, patch_idx:0, total_patches:4, bytes_downloaded:0, previous_completed_bytes:0, total_download_bytes:100, is_running:false, is_paused:false, pause_requested:false, is_terminal:false, error:null };
    await window.refreshHomeStatus();
    if (!document.querySelector('#login-form') || !document.querySelector('#login-submit').disabled) throw new Error('patch-required state replaced or enabled the Account Login card');
    if (document.querySelector('#lifecycle-strip').dataset.mode !== 'location' || document.querySelector('#lifecycle-location-title').textContent !== 'Client Update' || document.querySelector('#lifecycle-location-path').textContent !== 'C:/patches') throw new Error('idle update strip did not use the path-picker composition');
    if (document.querySelector('#lifecycle-location-help').textContent !== 'Download verified patches, or apply files from your selected patch folder.') throw new Error('Client Update helper copy drifted');
    const locationStripRect = document.querySelector('#lifecycle-strip').getBoundingClientRect();
    const locationHelpRect = document.querySelector('#lifecycle-location-help').getBoundingClientRect();
    const locationPickerRect = document.querySelector('.lifecycle-path-picker').getBoundingClientRect();
    if (locationHelpRect.bottom >= locationPickerRect.top || locationPickerRect.bottom > locationStripRect.bottom - 19) throw new Error('Client Update help overflowed the fixed location strip');
    if (Math.abs(locationPickerRect.top - locationStripRect.top - state.locationPickerOffset) >= .1) throw new Error('Client Update shifted the Path row below the Install Location position');
    if (Math.abs(document.querySelector('#lifecycle-strip').getBoundingClientRect().height - 137) >= .1 || Math.abs(document.querySelector('.news-panel').getBoundingClientRect().height - state.newsPanelHeight) >= .1 || Math.abs(document.querySelector('#lifecycle-strip').getBoundingClientRect().bottom - document.querySelector('.home-right').getBoundingClientRect().bottom) >= .1) throw new Error('idle update strip is not bottom-pinned independently of Recent News');
    const updateAction = document.querySelector('#home-primary');
    if (updateAction.textContent !== 'Update' || updateAction.disabled) throw new Error('idle patch action is not the stable Update button');
    state.patchStartDelayMs = 60;
    state.calls.length = 0;
    updateAction.click();
    updateAction.dispatchEvent(new MouseEvent('click', { bubbles:true }));
    await new Promise(resolve => setTimeout(resolve, 60));
    updateAction.click();
    if (!updateAction.disabled || updateAction.textContent !== 'Update' || state.calls.filter(call => call.command === 'start_patch_download').length !== 1) throw new Error('pending Update did not fence a delayed double click');
    await new Promise(resolve => setTimeout(resolve, 100));
    if (!updateAction.disabled || state.calls.filter(call => call.command === 'start_patch_download').length !== 1) throw new Error('Update request released its fence before rendering active state');
    state.patchStartDelayMs = 0;
    if (!state.patch.is_running || !updateAction.disabled) throw new Error('accepted update was not retained as an active operation');
    state.patch = { phase:'idle', download_idx:0, patch_idx:0, total_patches:4, bytes_downloaded:0, previous_completed_bytes:0, total_download_bytes:100, is_running:false, is_paused:false, pause_requested:false, is_terminal:false, error:null };
    await window.refreshHomeStatus();
    if (updateAction.disabled) throw new Error('idle Update action remained disabled after the fixture operation ended');
    state.calls.length = 0;
    document.querySelector('#lifecycle-path-action').click();
    await new Promise(resolve => setTimeout(resolve, 50));
    if (!state.calls.some(call => call.command === 'start_local_patch')) throw new Error('Use Local Files did not start the local patch flow');
    if (document.querySelector('#lifecycle-title').textContent !== 'Client Update' || document.querySelector('#lifecycle-cancel').hidden) throw new Error('local patch work did not use the shared operation status');
    state.patch = { ...state.patch, phase:'cancelled', is_running:false, is_paused:false, pause_requested:false, is_terminal:true, error:null };
    await window.refreshPatchSnapshot();
    state.calls.length = 0;
    updateAction.click();
    await new Promise(resolve => setTimeout(resolve, 50));
    if (!state.calls.some(call => call.command === 'reset_patch') || !state.calls.some(call => call.command === 'start_local_patch') || state.calls.some(call => call.command === 'start_patch_download')) throw new Error('Retry after local patch cancellation changed the selected source');
    state.patch = { phase:'idle', download_idx:0, patch_idx:0, total_patches:4, bytes_downloaded:0, previous_completed_bytes:0, total_download_bytes:100, is_running:false, is_paused:false, pause_requested:false, is_terminal:false, error:null };
    await window.refreshHomeStatus();
    const accountHeight = document.querySelector('#session-card').getBoundingClientRect().height;
    const patchStartError = 'The content host rejected the patch request.';
    state.patchStartError = patchStartError;
    state.calls.length = 0;
    updateAction.click();
    await new Promise(resolve => setTimeout(resolve, 40));
    const errorStrip = document.querySelector('#lifecycle-strip');
    const errorDetail = document.querySelector('#lifecycle-detail');
    if (errorStrip.dataset.mode !== 'error' || !errorDetail.textContent.includes(patchStartError) || Math.abs(errorStrip.getBoundingClientRect().height - 137) >= .1) throw new Error('patch acquisition failure did not show its backend reason in the fixed recovery strip');
    await window.refreshPatchSnapshot();
    await window.refreshHomeStatus();
    if (document.querySelector('#lifecycle-strip').dataset.mode !== 'error' || !document.querySelector('#lifecycle-detail').textContent.includes(patchStartError)) throw new Error('IPC start failure disappeared when idle patch status was polled');
    if (Math.abs(document.querySelector('#session-card').getBoundingClientRect().height - accountHeight) >= .1 || Math.abs(document.querySelector('.news-panel').getBoundingClientRect().height - state.newsPanelHeight) >= .1 || Math.abs(errorStrip.getBoundingClientRect().bottom - document.querySelector('.home-right').getBoundingClientRect().bottom) >= .1) throw new Error('patch error changed Home card geometry');
    if (document.querySelector('#home-primary').textContent !== 'Retry' || document.querySelector('#home-primary').disabled) throw new Error('patch failure did not expose primary Retry');
    state.patchStartError = null;
    state.calls.length = 0;
    document.querySelector('#home-primary').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (!state.calls.some(call => call.command === 'reset_patch') || !state.calls.some(call => call.command === 'start_patch_download') || state.patch.is_terminal) throw new Error('Retry did not reset the terminal run and start a new download');
    state.patch = { phase:'downloading', download_idx:0, patch_idx:0, total_patches:4, bytes_downloaded:40, previous_completed_bytes:0, total_download_bytes:100, is_running:true, is_paused:false, pause_requested:false, is_terminal:false, error:null };
    await window.refreshHomeStatus();
    if (document.querySelector('#lifecycle-pause').textContent !== 'Pause' || document.querySelector('#lifecycle-pause').hidden || document.querySelector('#lifecycle-cancel').hidden || updateAction.textContent !== 'Update' || !updateAction.disabled || document.querySelector('#lifecycle-metric').textContent !== '40 B / 100 B') throw new Error('active update controls or real download progress drifted');
    if (Math.abs(document.querySelector('#lifecycle-strip').getBoundingClientRect().height - 137) >= .1 || Math.abs(document.querySelector('.news-panel').getBoundingClientRect().height - state.newsPanelHeight) >= .1 || Math.abs(document.querySelector('#lifecycle-strip').getBoundingClientRect().bottom - document.querySelector('.home-right').getBoundingClientRect().bottom) >= .1) throw new Error('active update strip is not bottom-pinned independently of Recent News');
    document.querySelector('#lifecycle-pause').click();
    await new Promise(resolve => setTimeout(resolve, 50));
    if (document.querySelector('#lifecycle-pause').textContent !== 'Resume' || !state.calls.some(call => call.command === 'pause_patch') || updateAction.textContent !== 'Update' || document.querySelector('#lifecycle-copy').textContent !== 'Pausing after the current step.') throw new Error('a requested pause was reported as acknowledged');
    state.patch.is_paused = true;
    state.calls.length = 0;
    await window.refreshHomeStatus();
    if (document.querySelector('#lifecycle-copy').textContent !== 'Update paused.') throw new Error('update pause acknowledgement was not displayed');
    document.querySelector('#lifecycle-pause').click();
    await new Promise(resolve => setTimeout(resolve, 50));
    if (state.patch.pause_requested || state.patch.is_paused || document.querySelector('#lifecycle-pause').textContent !== 'Pause') throw new Error('Resume did not clear patch pause state');
    state.patch = { ...state.patch, phase:'extracting', is_paused:false, pause_requested:false, patch_idx:2 };
    await window.refreshPatchSnapshot();
    if (document.querySelector('#lifecycle-metric').textContent !== 'File 3 of 4') throw new Error('patch extraction did not show file progress');
    state.patchStartError = null;
    state.patchStatusDelayMs = 80;
    state.calls.length = 0;
    const stalePatchRefresh = window.refreshPatchSnapshot();
    await new Promise(resolve => setTimeout(resolve, 10));
    state.patchStatusDelayMs = 0;
    state.patch = { ...state.patch, phase:'error', is_running:false, is_paused:false, pause_requested:false, is_terminal:true, error:'fixture patch failure' };
    await window.refreshPatchSnapshot();
    await new Promise(resolve => setTimeout(resolve, 30));
    await stalePatchRefresh;
    if (document.querySelector('#lifecycle-strip').dataset.mode !== 'error' || !document.querySelector('#lifecycle-detail').textContent.includes('fixture patch failure') || updateAction.textContent !== 'Retry' || updateAction.disabled) throw new Error('stale patch status replaced the newest terminal recovery');
    if (document.querySelector('.news-panel').getBoundingClientRect().height > state.newsPanelHeight + .1) throw new Error('Recent News grew when the lifecycle strip appeared');
    if (Math.abs(document.querySelector('#lifecycle-strip').getBoundingClientRect().height - 137) >= .1) throw new Error('terminal recovery strip is not 137px tall');
    if (document.querySelector('#home-primary').textContent !== 'Retry' || document.querySelector('#home-primary').disabled) throw new Error('terminal recovery did not move Retry to the primary action');
    if (!document.querySelector('#lifecycle-actions').hidden || !document.querySelector('#lifecycle-cancel').hidden || !document.querySelector('#lifecycle-pause').hidden) throw new Error('terminal recovery retained update controls');
    if (!document.querySelector('#lifecycle-detail').textContent.includes('Press Retry to try again.')) throw new Error('terminal recovery guidance drifted');
    document.querySelector('#home-primary').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (!state.calls.some(call => call.command === 'reset_patch') || !state.calls.some(call => call.command === 'start_patch_download') || state.patch.is_terminal) throw new Error('terminal Retry did not restart the shared update operation');
    document.querySelector('#lifecycle-cancel').click();
    await window.refreshPatchSnapshot();
    if (document.querySelector('#lifecycle-title').textContent !== 'Update Cancelled' || !document.querySelector('#lifecycle-detail').textContent.includes('The operation was cancelled.')) throw new Error('cancel did not render terminal update recovery');
  `);

  await assertUi(`
    const state = window.__launcherBrowserState;
    window.resetPatchStatus();
    state.patch = { phase:'cancelled', is_running:false, is_paused:false, is_terminal:true, error:null };
    await window.refreshHomeStatus();
    if (document.querySelector('#lifecycle-title').textContent !== 'Update Cancelled' || !document.querySelector('#lifecycle-detail').textContent.includes('The operation was cancelled.') || !document.querySelector('#lifecycle-detail').textContent.includes('Press Retry to try again.')) throw new Error('cancelled patch state was not rendered');
  `);

  await assertUi(`
    const state = window.__launcherBrowserState;
    state.hostedPatches = false;
    state.patchRequired = true;
    state.noValidInstall = false;
    state.patch = { phase:'idle', is_install:false, is_running:false, is_paused:false, is_terminal:false, error:null };
    window.__launcherHome.installDestination = '';
    await window.refreshHomeStatus();
    if (document.querySelector('#home-primary').textContent !== 'Install Fresh' || document.querySelector('#lifecycle-location-title').textContent !== 'Install Game' || document.querySelector('#lifecycle-path-action').textContent !== 'PATH' || document.querySelector('#lifecycle-location-help').textContent !== 'Install the client. Change the destination with PATH.' || document.querySelector('#lifecycle-location').textContent.includes('Use Local Files')) throw new Error('older client still exposed a patch install strip');
    state.calls.length = 0;
    await window.startUpdate(false, false);
    if (state.calls.some(call => call.command === 'start_patch_download')) throw new Error('unhosted deployment requested absent patch objects');
    state.nextDirectory = 'C:/fresh-final';
    await window.chooseInstallFolder();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (document.querySelector('#lifecycle-location-path').textContent !== 'C:/fresh-final' || state.calls.some(call => call.command === 'install_quote' || call.command === 'install_game')) throw new Error('older client PATH did not select an idle destination');
    document.querySelector('#home-primary').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (!state.calls.some(call => call.command === 'install_game' && call.args.destination === 'C:/fresh-final') || document.querySelector('#lifecycle-title').textContent !== 'Game Installation') throw new Error('older-client fresh install was not identified as install work');
    document.querySelector('#lifecycle-cancel').click();
    await window.refreshPatchSnapshot();
    if (document.querySelector('#lifecycle-location-title').textContent !== 'Install Cancelled' || document.querySelector('#home-primary').textContent !== 'Retry Install') throw new Error('older-client install recovery lost the install source');
    state.hostedPatches = true;
    state.patch = { phase:'idle', is_install:false, is_running:false, is_terminal:false, error:null };
  `);

  await assertUi(`
    const state = window.__launcherBrowserState;
    const home = window.__launcherHome;
    home.token = '';
    home.installDestination = 'C:/fresh-final';
    state.hostedPatches = false;
    state.patchRequired = true;
    state.patch = { phase:'error', is_install:true, is_running:false, is_terminal:true, error:'fixture install failure' };
    await window.refreshHomeStatus();
    if (document.querySelector('#home-primary').textContent !== 'Retry Install') throw new Error('older-client failure did not offer Retry Install');
    document.querySelector('[data-route="settings"]').click();
    document.querySelector('[data-settings-tab="misc"]').click();
    state.nextInstallDir = null;
    state.calls.length = 0;
    document.querySelector('[data-settings-action="browse-game"]').click();
    await new Promise(resolve => setTimeout(resolve, 50));
    if (state.calls.some(call => call.command === 'reset_patch') || home.installDestination !== 'C:/fresh-final' || !state.patch.is_terminal) throw new Error('cancelled install picker discarded the original retry');
    document.querySelector('[data-route="home"]').click();
    state.calls.length = 0;
    document.querySelector('#home-primary').click();
    await new Promise(resolve => setTimeout(resolve, 50));
    if (!state.calls.some(call => call.command === 'install_game' && call.args.destination === 'C:/fresh-final')) throw new Error('Retry Install changed the original destination');
    state.patch = { ...state.patch, phase:'cancelled', is_running:false, is_terminal:true, error:null };
    await window.refreshPatchSnapshot();
    state.nextInstallDir = 'C:/game';
    state.pickerSelectsFinal = true;
    state.calls.length = 0;
    document.querySelector('[data-route="settings"]').click();
    document.querySelector('[data-settings-tab="misc"]').click();
    document.querySelector('[data-settings-action="browse-game"]').click();
    await new Promise(resolve => setTimeout(resolve, 50));
    document.querySelector('[data-route="home"]').click();
    if (!state.calls.some(call => call.command === 'reset_patch') || state.patch.is_terminal || home.installDestination || home.installError || localStorage.getItem('bahamut-install-destination')) throw new Error('valid final client selection did not clear finished install recovery');
    if (document.querySelector('#home-layout').dataset.lifecycle !== 'logged-out' || document.querySelector('#home-primary').textContent !== 'Play' || !document.querySelector('#home-primary').disabled || !document.querySelector('#login-submit') || document.querySelector('#lifecycle-strip').dataset.recovery !== 'false') throw new Error('logged-out final client selection retained Retry Install or bypassed login');
    state.calls.length = 0;
    document.querySelector('#home-primary').click();
    if (state.calls.some(call => call.command === 'install_game' || call.command === 'launch_game')) throw new Error('logged-out selection started install or launch');

    home.token = 'fixture-session';
    home.username = 'Fixture';
    state.patchRequired = true;
    state.patch = { phase:'error', is_install:true, is_running:false, is_terminal:true, error:'fixture install failure' };
    await window.refreshHomeStatus();
    if (document.querySelector('#home-primary').textContent !== 'Retry Install') throw new Error('authenticated older-client failure did not offer Retry Install');
    state.calls.length = 0;
    document.querySelector('[data-route="settings"]').click();
    document.querySelector('[data-settings-tab="misc"]').click();
    document.querySelector('[data-settings-action="browse-game"]').click();
    await new Promise(resolve => setTimeout(resolve, 50));
    document.querySelector('[data-route="home"]').click();
    if (!state.calls.some(call => call.command === 'reset_patch') || document.querySelector('#home-layout').dataset.lifecycle !== 'ready' || document.querySelector('#home-primary').textContent !== 'Play' || document.querySelector('#home-primary').disabled || document.querySelector('#lifecycle-strip').dataset.recovery !== 'false') throw new Error('authenticated selection retained install recovery');
    state.calls.length = 0;
    document.querySelector('#home-primary').click();
    await new Promise(resolve => setTimeout(resolve, 30));
    if (!state.calls.some(call => call.command === 'launch_game') || state.calls.some(call => call.command === 'install_game')) throw new Error('authenticated final client did not use Play');
    home.token = '';
    state.gameRunning = false;
    state.patchRequired = false;
    state.hostedPatches = true;
    state.pickerSelectsFinal = false;
    await window.refreshHomeStatus();
  `);

  await assertUi(`
    const state = window.__launcherBrowserState;
    const dialog = document.querySelector('#launcher-closing-dialog');
    if (dialog.open) throw new Error('closing message appeared without a close request');
    state.eventHandlers['launcher-closing']({ payload:null });
    state.eventHandlers['launcher-closing']({ payload:null });
    if (!dialog.matches(':modal') || !dialog.contains(document.activeElement)) throw new Error('closing message did not block background input and take focus');
    if (!dialog.textContent.includes('Finishing current work safely') || !dialog.textContent.includes('close automatically')) throw new Error('closing message did not explain the wait');
    const route = document.querySelector('[data-screen][data-active]');
    const callsBefore = state.calls.length;
    runControllerAction('next-route');
    runControllerAction('confirm');
    runControllerAction('cancel');
    if (!dialog.open || route !== document.querySelector('[data-screen][data-active]') || state.calls.length !== callsBefore) throw new Error('controller input escaped the closing dialog');
  `);
  await devtools.send('Input.dispatchKeyEvent', { type:'keyDown', key:'Escape', code:'Escape', windowsVirtualKeyCode:27 });
  await devtools.send('Input.dispatchKeyEvent', { type:'keyUp', key:'Escape', code:'Escape', windowsVirtualKeyCode:27 });
  await assertUi(`
    if (!document.querySelector('#launcher-closing-dialog').matches(':modal')) throw new Error('Escape dismissed the closing message');
  `);

  await evaluate(devtools, `localStorage.setItem('bahamut-theme', 'light')`);
  await devtools.send('Page.navigate', { url: `http://127.0.0.1:${httpPort}/index.html?fit-fail` });
  await evaluate(devtools, `new Promise(resolve => {
    const check = () => location.search === '?fit-fail'
      && window.__launcherBrowserState?.calls.some(call => call.command === 'fit_window_to_work_area')
      && document.querySelector('#launcher-version')?.textContent === 'vbrowser-test'
      ? resolve(true)
      : setTimeout(check, 10);
    check();
  })`);
  await assertUi(`
    const state = window.__launcherBrowserState;
    if (!state.calls.some(call => call.command === 'fit_window_to_work_area')) throw new Error('resize failure fixture did not run');
    if (!state.calls.some(call => call.command === 'get_server_settings') || document.querySelector('#launcher-version').textContent !== 'vbrowser-test') throw new Error('resize failure aborted normal boot');
    if (document.querySelector('#shell-status,.status-line')) throw new Error('shell toast survived the no-overlay policy');
    if (document.body.dataset.theme !== 'light' || document.querySelector('[data-utility="theme"]').getAttribute('aria-label') !== 'Use night theme') throw new Error('saved theme did not synchronize the theme toggle');
  `);
});

test('settings_keyboard_and_accessibility_tree', async t => {
  const server = createServer(serveUiFixture);
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const httpPort = server.address().port;
  const debugPort = await freePort();
  const profileDir = mkdtempSync(join(tmpdir(), 'bahamut-launcher-browser-'));
  const browser = spawn(browserPath(), [
    '--headless=new', '--disable-gpu', '--no-sandbox',
    `--remote-debugging-port=${debugPort}`, `--user-data-dir=${profileDir}`,
    'about:blank',
  ], { stdio: 'ignore' });
  let devtools = null;
  t.after(async () => {
    if (devtools) {
      await devtools.send('Browser.close').catch(() => {});
      devtools.socket.close();
    }
    await stopChild(browser);
    await delay(1000);
    await new Promise(resolve => server.close(resolve));
    removeTempRoot(profileDir);
  });
  devtools = await connectDevTools(debugPort);
  await devtools.send('Page.addScriptToEvaluateOnNewDocument', { source: evaluateScript() });
  await devtools.send('Page.enable');
  await devtools.send('Runtime.enable');
  await devtools.send('Accessibility.enable');
  await devtools.send('Emulation.setDeviceMetricsOverride', {
    width:1280, height:800, screenWidth:1280, screenHeight:800,
    deviceScaleFactor:1, mobile:false,
  });
  await devtools.send('Page.navigate', { url:`http://127.0.0.1:${httpPort}/index.html` });
  await evaluate(devtools, `new Promise(resolve => {
    const check = () => document.querySelector('#home-layout')?.dataset.lifecycle ? resolve(true) : setTimeout(check, 10);
    check();
  })`);
  await exposeFrontendModules(devtools);

  const assertUi = expression => evaluate(devtools, `(async () => { ${expression} })()`);
  const press = async (key, modifiers = 0) => {
    const keyCodes = { Tab:9, Enter:13, ' ':32, ArrowLeft:37, ArrowRight:39, ArrowDown:40 };
    const params = { key, code:key === ' ' ? 'Space' : key, windowsVirtualKeyCode:keyCodes[key], modifiers };
    await devtools.send('Input.dispatchKeyEvent', { ...params, type:'keyDown', text:key === 'Enter' ? '\r' : key === ' ' ? ' ' : '' });
    await devtools.send('Input.dispatchKeyEvent', { ...params, type:'keyUp' });
  };
  const hasFocus = selector => evaluate(devtools, `document.activeElement.matches(${JSON.stringify(selector)})`);
  const tabTo = async selector => {
    for (let step = 0; step < 80; step += 1) {
      if (await hasFocus(selector)) return;
      await press('Tab');
      await assertUi(`
        if (document.activeElement.matches('[data-gamepad-settings-row]')) throw new Error('Tab focused a settings wrapper instead of its control');
      `);
    }
    throw new Error(`Keyboard focus did not reach ${selector}`);
  };
  const assertAx = async (selector, role, name) => {
    const { root } = await devtools.send('DOM.getDocument');
    const { nodeId } = await devtools.send('DOM.querySelector', { nodeId:root.nodeId, selector });
    if (!nodeId) throw new Error(`Missing accessibility target ${selector}`);
    const { node } = await devtools.send('DOM.describeNode', { nodeId });
    const { nodes } = await devtools.send('Accessibility.getPartialAXTree', { nodeId, fetchRelatives:false });
    const ax = nodes.find(candidate => candidate.backendDOMNodeId === node.backendNodeId);
    if (!ax || ax.ignored || ax.role?.value !== role || (name !== undefined && ax.name?.value !== name)) {
      throw new Error(`Wrong accessibility node for ${selector}: ${JSON.stringify(ax)}`);
    }
    return ax;
  };

  await tabTo('[data-route="home"]');
  await press('ArrowRight');
  await press('ArrowRight');
  await assertUi(`if (!document.querySelector('[data-route="settings"][aria-selected="true"]')) throw new Error('Arrow keys did not open Settings');`);
  await tabTo('[data-settings-tab="general"]');
  const closeOn = '[data-launcher-setting-choice="close_on_game_start"][data-launcher-setting-value="true"]';
  const closeOff = '[data-launcher-setting-choice="close_on_game_start"][data-launcher-setting-value="false"]';
  await tabTo(closeOn);
  await press('Tab');
  if (!await hasFocus(closeOff)) throw new Error('Tab skipped the second choice');
  await press('Tab', 8);
  if (!await hasFocus(closeOn)) throw new Error('Shift+Tab did not return to the previous choice');
  await press('Tab');
  await press(' ');
  await assertUi(`if (window.__launcherBrowserState.launcherBehavior.close_on_game_start !== false) throw new Error('Space did not save the focused choice');`);
  const offAx = await assertAx(closeOff, 'button', 'Off \u2713');
  if (offAx.properties.find(property => property.name === 'pressed')?.value.value !== 'true') throw new Error('Accessibility tree did not expose the pressed choice');
  await assertAx('[aria-labelledby="close-launcher-label"]', 'group', 'Close Launcher on Game Start');
  await assertAx('[data-settings-pane="general"] [data-gamepad-settings-row]', 'group');

  await tabTo('#game-resolution');
  const oldResolution = await evaluate(devtools, 'document.querySelector("#game-resolution").value');
  await press('ArrowDown');
  await press('Tab');
  await assertUi(`
    const selected = document.querySelector('#game-resolution').value;
    if (selected === ${JSON.stringify(oldResolution)} || !window.__launcherBrowserState.calls.some(call => call.command === 'set_game_settings' && call.args.settings.width + 'x' + call.args.settings.height === selected)) throw new Error('Keyboard resolution change did not reach the settings command');
  `);
  await assertAx('#game-resolution', 'combobox', 'Window Resolution');

  await tabTo('[data-settings-tab="general"]');
  await press('ArrowRight');
  await tabTo('#game-general-quality');
  const oldQuality = await evaluate(devtools, 'Number(document.querySelector("#game-general-quality").value)');
  await press('ArrowRight');
  await assertUi(`
    if (Number(document.querySelector('#game-general-quality').value) !== ${oldQuality + 1} || window.__launcherBrowserState.gameSettings.settings.graphics.general_quality !== ${oldQuality + 1}) throw new Error('Arrow key did not save the native slider value');
  `);
  await assertAx('#game-general-quality', 'slider', 'General Drawing Quality');
  await assertAx('#game-shadow-detail', 'slider', 'Shadow Detail');

  await tabTo('[data-settings-tab="graphics"]');
  await press('ArrowRight');
  const folder = '[data-settings-pane="misc"] [data-extension-folder="backups"]';
  await tabTo(folder);
  await assertAx(folder, 'button', 'Open Backup Folder');
  await press('Enter');
  await assertUi(`if (!window.__launcherBrowserState.calls.some(call => call.command === 'open_extension_folder' && call.args.target === 'backups')) throw new Error('Enter did not activate the folder button');`);

  await tabTo('[data-utility="gamepad"]');
  await press('Enter');
  await tabTo('[data-gamepad-choice="false"]');
  await assertAx('[data-gamepad-choice="false"]', 'button', 'Off');
  await press(' ');
  await assertUi(`
    if (document.querySelector('[data-gamepad-choice="false"]').getAttribute('aria-pressed') !== 'true') throw new Error('Space did not toggle the gamepad setting');
    if (document.body.dataset.inputMode !== 'keyboard') throw new Error('Keyboard input lost its focus mode');
  `);
});

test('flat_settings_and_extensions_match_backend_contract', async t => {
  const server = createServer(serveUiFixture);
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const httpPort = server.address().port;
  const debugPort = await freePort();
  const profileDir = mkdtempSync(join(tmpdir(), 'bahamut-launcher-browser-'));
  const browser = spawn(browserPath(), [
    '--headless=new', '--disable-gpu', '--no-sandbox',
    `--remote-debugging-port=${debugPort}`, `--user-data-dir=${profileDir}`,
    'about:blank',
  ], { stdio: 'ignore' });
  let devtools = null;

  t.after(async () => {
    if (devtools) {
      await devtools.send('Browser.close').catch(() => {});
      devtools.socket.close();
    }
    await stopChild(browser);
    await delay(1000);
    await new Promise(resolve => server.close(resolve));
    removeTempRoot(profileDir);
  });

  devtools = await connectDevTools(debugPort);
  await devtools.send('Page.addScriptToEvaluateOnNewDocument', { source: evaluateScript() });
  await devtools.send('Page.enable');
  await devtools.send('Runtime.enable');
  await devtools.send('Emulation.setDeviceMetricsOverride', {
    width: 1280,
    height: 800,
    screenWidth: 1280,
    screenHeight: 800,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await devtools.send('Page.navigate', { url: `http://127.0.0.1:${httpPort}/index.html` });
  await evaluate(devtools, `new Promise(resolve => {
    const check = () => document.querySelector('#home-layout')?.dataset.lifecycle ? resolve(true) : setTimeout(check, 10);
    check();
  })`);
  await exposeFrontendModules(devtools);
  await delay(100);

  const assertUi = expression => evaluate(devtools, `(async () => { ${expression} })()`);

  await assertUi(`
    const state = window.__launcherBrowserState;
    state.calls.length = 0;
    document.querySelector('[data-route="settings"]').click();
    await new Promise(resolve => setTimeout(resolve, 80));
    const settingsPage = document.querySelector('.settings-page').getBoundingClientRect();
    if (settingsPage.width > 1120.1 || settingsPage.width < 900) throw new Error('Settings did not use the centered capped Avalon shell: ' + settingsPage.width);
    const patchDownloadsCard = document.querySelector('.settings-card--locations');
    const clientMaintenanceCard = document.querySelector('.settings-card--client-patches');
    if (/Game installation|selected path is shared|Existing Final Fantasy|Replace the current portable|Open the native XIV Config utility/i.test(patchDownloadsCard.textContent)) throw new Error('Misc retained removed install helper copy');
    if ([...patchDownloadsCard.querySelectorAll('.settings-field')].some(field => getComputedStyle(field).borderBottomWidth !== '0px')) throw new Error('Misc retained row separators');
    if ([...clientMaintenanceCard.querySelectorAll('h2')].map(heading => heading.textContent).join(',') !== 'User Settings and Macros,Extensions' || clientMaintenanceCard.querySelector('[data-extension-folder="screenshots"]') !== clientMaintenanceCard.querySelector('[data-gamepad-settings-row]:last-child button') || /Delivery|Apply a local patch source/i.test(clientMaintenanceCard.textContent)) throw new Error('Right Misc card has the wrong backup headings, final action, or removed patch-operation copy');
    const generalPane = document.querySelector('[data-settings-pane="general"]');
    const generalCards = [...generalPane.querySelectorAll('.settings-card')];
    const generalLabels = generalCards.map(card => [...card.querySelectorAll('.settings-label')].map(label => label.textContent).join(','));
    const cardBindings = cards => cards.map(card => [...card.querySelectorAll('.settings-field')].map(field => {
      const label = field.querySelector('.settings-label').textContent;
      const control = field.querySelector('[data-game-setting-choice],[data-game-setting],[data-launcher-setting-choice],[data-object-distance-selection],[data-camera-zoom-selection],#borderless-monitor');
      const binding = control.dataset.gameSettingChoice || control.dataset.gameSetting || control.dataset.launcherSettingChoice || (control.hasAttribute('data-object-distance-selection') ? 'object-distance' : '') || (control.hasAttribute('data-camera-zoom-selection') ? 'camera-zoom' : '');
      return label + ':' + (binding ? (control.dataset.launcherSettingChoice ? 'launcher.' + binding : binding) : 'monitor.selected');
    }).join(','));
    const generalBindings = cardBindings(generalCards);
    if (generalCards.length !== 2 || generalLabels.join('|') !== 'Close Launcher on Game Start,Hardware Mouse,Sound,Always Play Sound|Display Mode,Borderless Monitor,Window Resolution,Native Resolution Override') throw new Error('General settings did not use the intended row order: ' + generalLabels.join('|'));
    if (generalBindings.join('|') !== 'Close Launcher on Game Start:launcher.close_on_game_start,Hardware Mouse:graphics.hardware_mouse,Sound:audio.enabled,Always Play Sound:audio.play_in_background|Display Mode:display_mode,Borderless Monitor:monitor.selected,Window Resolution:resolution,Native Resolution Override:launcher.native_resolution_override') throw new Error('General settings labels are not bound to the intended owners: ' + generalBindings.join('|'));
    if (generalCards.map(card => card.dataset.gamepadRegionOrder).join(',') !== '1,2' || getComputedStyle(generalPane.querySelector('.settings-field')).minHeight !== '76px') throw new Error('General settings card order or row spacing drifted');
    const displayModeChoices = generalPane.querySelectorAll('[data-game-setting-choice="display_mode"]');
    if (displayModeChoices.length !== 3 || !generalPane.querySelector('[data-game-setting-choice="display_mode"][data-game-setting-value="borderless"]') || !displayModeChoices[0].parentElement.classList.contains('choice-row--compact') || /Borderless Windowed|Language/.test(generalPane.textContent)) throw new Error('General settings display mode choices drifted');
    const monitorSelect = document.querySelector('#borderless-monitor');
    const monitorRow = monitorSelect.closest('.settings-field');
    const monitorFixture = state.borderlessMonitors;
    if (monitorRow.hidden || !monitorSelect.disabled || monitorSelect.value !== 'removed-display-id' || monitorSelect.selectedOptions[0].textContent !== 'Unavailable display (uses primary)' || ![...monitorSelect.options].some(option => option.value === 'display-primary-id') || state.calls.some(call => call.command === 'set_borderless_monitor')) throw new Error('monitor initial values ' + JSON.stringify({fixture:state.borderlessMonitors,hidden:monitorRow.hidden,disabled:monitorSelect.disabled,value:monitorSelect.value,selected:monitorSelect.selectedOptions[0]?.textContent,options:[...monitorSelect.options].map(option=>[option.value,option.textContent]),mode:state.gameSettings.settings.display_mode,settingsError:document.querySelector('#settings-patch-status').textContent,gameSettingsError:document.querySelector('#game-settings-status').textContent,apiCalls:state.calls.filter(call=>call.command.includes('borderless_monitor'))}));
    generalPane.querySelector('[data-game-setting-choice="display_mode"][data-game-setting-value="borderless"]').click();
    await new Promise(resolve => setTimeout(resolve, 60));
    if (state.gameSettings.settings.display_mode !== 'borderless' || monitorSelect.disabled || monitorSelect.value !== 'removed-display-id') throw new Error('Borderless mode did not enable the retained monitor identity');
    const monitorRegion = generalCards[1];
    monitorRegion.focus();
    for (let step = 0; step < 2; step += 1) window.runControllerAction('down');
    if (document.activeElement !== monitorRow) throw new Error('gamepad row navigation skipped the enabled Borderless Monitor selector');
    window.runControllerAction('confirm');
    if (document.activeElement !== monitorSelect) throw new Error('gamepad confirm did not focus the monitor selector');
    window.runControllerAction('confirm');
    await new Promise(resolve => setTimeout(resolve, 60));
    if (state.borderlessMonitors.selected !== 'display-primary-id' || !state.calls.some(call => call.command === 'set_borderless_monitor' && call.args.monitorId === 'display-primary-id')) throw new Error('gamepad monitor selection did not round-trip its OS identity');
    monitorSelect.value = 'display-secondary-id';
    monitorSelect.dispatchEvent(new Event('change', { bubbles:true }));
    await new Promise(resolve => setTimeout(resolve, 60));
    if (state.borderlessMonitors.selected !== 'display-secondary-id' || monitorSelect.value !== 'display-secondary-id' || !state.calls.some(call => call.command === 'set_borderless_monitor' && call.args.monitorId === 'display-secondary-id')) throw new Error('monitor selection did not round-trip the selected display identity');
    monitorSelect.value = '';
    monitorSelect.dispatchEvent(new Event('change', { bubbles:true }));
    await new Promise(resolve => setTimeout(resolve, 60));
    if (state.borderlessMonitors.selected !== null || monitorSelect.value !== '') throw new Error('Default Monitor did not clear the persisted display identity');
    monitorFixture.selected = 'unavailable-after-reopen';
    window.renderBorderlessMonitorSettings(structuredClone(monitorFixture));
    if (monitorSelect.value !== 'unavailable-after-reopen' || monitorSelect.selectedOptions[0].textContent !== 'Unavailable display (uses primary)') throw new Error('missing display identity was not preserved after re-render');
    generalPane.querySelector('[data-game-setting-choice="display_mode"][data-game-setting-value="windowed"]').click();
    await new Promise(resolve => setTimeout(resolve, 60));
    await window.queueGameSettingsUpdate(() => {});
    if (state.gameSettings.settings.display_mode !== 'windowed' || !monitorSelect.disabled) throw new Error('monitor selector remained enabled outside Borderless mode');
    window.renderBorderlessMonitorSettings({ supported:false, monitors:[], selected:null });
    if (!monitorRow.hidden || !monitorSelect.disabled) throw new Error('unsupported monitor selector was not hidden and disabled');
    window.renderBorderlessMonitorSettings(structuredClone(monitorFixture));
    const waitForHydrationCall = async command => {
      for (let attempt = 0; attempt < 40; attempt += 1) {
        if (state.calls.some(call => call.command === command)) return;
        await new Promise(resolve => setTimeout(resolve, 5));
      }
      throw new Error('delayed settings hydration did not call ' + command);
    };
    state.calls.length = 0;
    document.querySelector('#settings-patch-status').textContent = '';
    state.installPath = 'C:/newer-game';
    state.settingsHydrationPlan = { command:'detect_game_install_command', delayMs:80, error:'stale settings hydration failure' };
    const staleFailedHydration = window.hydrateSettings();
    await waitForHydrationCall('detect_game_install_command');
    await window.hydrateSettings();
    await staleFailedHydration;
    if (document.querySelector('#settings-patch-status').textContent || document.querySelector('#settings-game-path').textContent !== 'C:/newer-game') throw new Error('a stale failed settings hydration overwrote the newer successful result');
    state.installPath = 'C:/game';
    await window.hydrateSettings();

    state.calls.length = 0;
    document.querySelector('#game-settings-status').textContent = '';
    state.settingsHydrationPlan = { command:'get_borderless_monitors', delayMs:80, error:'stale monitor enumeration failure' };
    const staleMonitorHydration = window.hydrateSettings();
    await waitForHydrationCall('get_borderless_monitors');
    await window.queueBorderlessMonitorUpdate('display-secondary-id');
    await staleMonitorHydration;
    if (state.borderlessMonitors.selected !== 'display-secondary-id' || document.querySelector('#borderless-monitor').value !== 'display-secondary-id' || document.querySelector('#game-settings-status').textContent.includes('monitor selection is unavailable')) throw new Error('a stale monitor enumeration error overwrote the saved display selection');
    if (generalPane.querySelector('[data-theme-choice]') || /Appearance|Launcher behavior|Account routing|Active server|Login, registration|Portable configuration|Edit profile|Saved to config/i.test(generalPane.textContent)) throw new Error('General settings retained removed titles or helper copy');
    if (generalPane.querySelector('.section-eyebrow,.settings-description,.settings-hint') || [...generalPane.querySelectorAll('.settings-field')].some(field => getComputedStyle(field).borderBottomWidth !== '0px')) throw new Error('General settings retained headings, hints, or row separators');
    const graphicsPane = document.querySelector('[data-settings-pane="graphics"]');
    const graphicsCards = [...graphicsPane.querySelectorAll('.settings-card')];
    const graphicsLabels = graphicsCards.map(card => [...card.querySelectorAll('.settings-label')].map(label => label.textContent).join(','));
    const graphicsBindings = cardBindings(graphicsCards);
    if (graphicsLabels.join('|') !== 'Multisampling,General Drawing Quality,Background Drawing Quality,Extended Draw Distance,Shadow Detail,Cutscene Effects|Texture Quality,Texture Filtering,Ambient Occlusion,Depth Of Field,Extended Camera Zoom') throw new Error('Graphics labels or Cutscene Effects placement drifted: ' + graphicsLabels.join('|'));
    if (graphicsBindings.join('|') !== 'Multisampling:graphics.multisampling,General Drawing Quality:graphics.general_quality,Background Drawing Quality:graphics.background_quality,Extended Draw Distance:object-distance,Shadow Detail:graphics.shadow_detail,Cutscene Effects:graphics.cutscene_effects|Texture Quality:graphics.texture_quality,Texture Filtering:graphics.texture_filtering,Ambient Occlusion:graphics.ambient_occlusion,Depth Of Field:graphics.depth_of_field,Extended Camera Zoom:camera-zoom') throw new Error('Graphics labels are not bound to the intended owners: ' + graphicsBindings.join('|'));
    if (graphicsCards.length !== 2 || graphicsCards.map(card => card.dataset.gamepadRegionOrder).join(',') !== '1,2') throw new Error('Graphics card ownership or order drifted');
    if (graphicsPane.querySelectorAll('select').length || graphicsPane.querySelectorAll('input[type="range"]').length !== 6 || graphicsPane.querySelectorAll('.choice-row').length !== 5) throw new Error('Graphics did not keep discrete choices and ordered range bars');
    if (graphicsPane.querySelector('[data-game-setting-output]') || [...graphicsPane.querySelectorAll('.settings-range-labels')].filter(labels => labels.closest('.settings-field').querySelector('[data-game-setting]')).some(labels => labels.textContent !== 'LowHigh')) throw new Error('Retail graphics range bars retained numeric or inconsistent endpoint labels');
    const filteringBar = graphicsPane.querySelector('[data-game-setting="graphics.texture_filtering"]');
    if (filteringBar?.type !== 'range' || filteringBar.min !== '0' || filteringBar.max !== '3' || filteringBar.step !== '1' || filteringBar.dataset.gameSettingValues !== 'low,standard,high,highest' || filteringBar.nextElementSibling?.textContent !== 'LowHigh') throw new Error('Texture Filtering did not use the four-step Low-High bar');
    const distanceBar = graphicsPane.querySelector('[data-object-distance-selection]');
    const zoomBar = graphicsPane.querySelector('[data-camera-zoom-selection]');
    if (![distanceBar, zoomBar].every(range => range.type === 'range' && range.min === '0' && range.step === '1' && !range.closest('.settings-range')?.querySelector('output')) || distanceBar.max !== '4' || distanceBar.dataset.graphicsValues !== 'off,125,150,175,200' || zoomBar.max !== '5' || zoomBar.dataset.graphicsValues !== 'off,11,12,13,14,15') throw new Error('Graphics stepped bars are missing or out of order');
    if ([distanceBar, zoomBar].some(range => getComputedStyle(range.closest('.settings-range').querySelector('.settings-range-labels')).gridTemplateColumns.split(' ').length !== 2) || distanceBar.nextElementSibling.textContent !== 'Off2x' || zoomBar.nextElementSibling.textContent !== 'Off15') throw new Error('Graphics bars do not match the two-endpoint Shadow Detail layout');
    const miscPane = document.querySelector('[data-settings-pane="misc"]');
    const gamepadSettingsRows = [...document.querySelectorAll('[data-gamepad-settings-row]')];
    if (gamepadSettingsRows.some(row => row.getAttribute('role') !== 'group' || row.tabIndex !== -1 || !row.querySelector('button,select,input'))) throw new Error('Settings rows retained fake button semantics or lost their real controls');
    const miscCards = [...miscPane.querySelectorAll('.settings-card')];
    if (miscCards.length !== 2 || [...miscCards[0].querySelectorAll('h2')].map(heading => heading.textContent).join(',') !== 'Install Location' || [...miscCards[1].querySelectorAll('h2')].map(heading => heading.textContent).join(',') !== 'User Settings and Macros,Extensions' || miscCards[0].dataset.gamepadRegionOrder !== '1' || miscCards[1].dataset.gamepadRegionOrder !== '2') throw new Error('Misc did not keep its two original cards');
    const existingMiscContent = miscCards.slice(0, 2);
    if (existingMiscContent.some(card => card.querySelector('.settings-label'))) throw new Error('Misc contains an unexpected setting label');
    if (existingMiscContent.flatMap(card => [...card.querySelectorAll('.settings-action')]).map(button => button.textContent).join(',') !== 'Path,Check for Updates,Repair Install,Backup,Restore,Backup,Restore,Open Backup Folder,Open Install Folder,Open Screenshots Folder') throw new Error('Misc action labels or order drifted');
    const accentActionLabels = [...document.querySelectorAll('.accent-action')].map(button => button.textContent.trim()).join(',');
    if (accentActionLabels !== 'PATH,Pause,Cancel,Open Plugins Folder,Open Addons Folder,Path,Check for Updates,Repair Install,Backup,Backup,Open Backup Folder,Open Screenshots Folder,XIV Config,Apply Steam Deck Defaults,Confirm') throw new Error('accent action mapping drifted: ' + accentActionLabels);
    const prominentActions = [...document.querySelectorAll('.primary-action,.login-action,.accent-action,.help-action.primary,.choice-option[aria-pressed="true"]')].filter(button => !button.disabled);
    const actionEdges = { dark: 'rgb(240, 154, 145)', light: 'rgb(188, 238, 255)' };
    for (const theme of ['dark','light']) {
      document.body.dataset.theme = theme;
      const actionEdge = actionEdges[theme];
      const edgeMismatch = prominentActions.find(button => getComputedStyle(button).borderTopWidth !== '1px' || getComputedStyle(button).borderTopColor !== actionEdge);
      if (edgeMismatch || actionEdge === getComputedStyle(document.body).color) throw new Error(theme + ' prominent action lost its theme-matched one-pixel edge: ' + (edgeMismatch?.textContent.trim() || 'edge matched text') + ' / ' + (edgeMismatch ? getComputedStyle(edgeMismatch).borderTopColor : actionEdge) + ' expected ' + actionEdge);
    }
    document.body.dataset.theme = 'dark';
    const darkThemeStyle = getComputedStyle(document.body);
    if (darkThemeStyle.getPropertyValue('--extension-selection-outer').trim() !== '#f09a91' || darkThemeStyle.getPropertyValue('--extension-hover-ring').trim() !== '#ffc0b8' || !darkThemeStyle.getPropertyValue('--focus').includes('#f09a91')) throw new Error('night focus family drifted from its coral palette');
    const primaryHoverRule = [...document.styleSheets].flatMap(sheet => [...sheet.cssRules]).find(rule => rule.selectorText?.includes('.primary-action:hover') && rule.style.transform === 'translateY(-1px)');
    const primaryGamepadRule = [...document.styleSheets].flatMap(sheet => [...sheet.cssRules]).find(rule => rule.selectorText?.includes('data-input-mode="gamepad"') && rule.selectorText.includes('.primary-action') && rule.selectorText.includes(':focus') && rule.style.transform === 'translateY(-1px)');
    const settingsLiftRule = [...document.styleSheets].flatMap(sheet => [...sheet.cssRules]).find(rule => rule.style?.transform === 'translateY(-1px)' && /accent-action|help-action|choice-option/.test(rule.selectorText || ''));
    if (!primaryHoverRule || !primaryGamepadRule || settingsLiftRule) throw new Error('primary-only mouse/gamepad lift contract drifted');
    if ([...miscPane.querySelectorAll('[data-backup-action="restore"],[data-extension-folder="install"]')].some(button => button.classList.contains('accent-action')) || !document.querySelector('[data-help-action="copy"]').classList.contains('primary')) throw new Error('secondary restore/install or existing Copy Logs action changed surface tier');
    const accentBackupStyle = getComputedStyle(miscPane.querySelector('[data-backup-action="create"]'));
    const secondaryRestoreStyle = getComputedStyle(miscPane.querySelector('[data-backup-action="restore"]'));
    const disabledSteamActionStyle = getComputedStyle(document.querySelector('.gamepad-card--stub .accent-action'));
    const enabledAccentActions = [...document.querySelectorAll('.accent-action:not(:disabled)')];
    if (!enabledAccentActions.length || enabledAccentActions.some(button => !getComputedStyle(button).backgroundImage.includes('linear-gradient') || getComputedStyle(button).color !== 'rgb(25, 10, 13)') || !accentBackupStyle.backgroundImage.includes('linear-gradient') || secondaryRestoreStyle.backgroundImage !== 'none' || disabledSteamActionStyle.backgroundImage !== 'none' || disabledSteamActionStyle.backgroundColor !== 'rgba(90, 103, 120, 0.28)') throw new Error('night accent, secondary, or disabled action surfaces bypassed their semantic tiers');
    if (!miscCards[0].querySelector('#settings-game-path') || miscCards[0].querySelector('#settings-patch-path,[data-settings-action="browse-patches"],[data-extension-folder="screenshots"]') || miscCards[1].querySelector('[data-gamepad-settings-row]:last-child button')?.dataset.extensionFolder !== 'screenshots' || miscCards[1].querySelector('[data-settings-action="browse-game"]') || miscCards[1].querySelectorAll('[data-backup-action]').length !== 4) throw new Error('Misc actions are not grouped with their owning content');
    if (miscPane.querySelector('[data-lifecycle-action],[data-settings-action="download-patches"],[data-settings-action="local-patches"],[data-settings-action="cancel-patches"]')) throw new Error('Misc retained operational patch controls');
    if (miscPane.textContent.includes('Patch downloads and active update controls')) throw new Error('Misc retained the redundant Home patch-work explanation');
    if ([...document.querySelectorAll('.settings-field')].some(field => Number.parseFloat(getComputedStyle(field).minHeight) < 76) || [...miscPane.querySelectorAll('.settings-action')].some(button => getComputedStyle(button).minHeight !== '48px') || getComputedStyle(miscPane.querySelector('h2')).marginBottom !== '16px') throw new Error('Settings spacing is not consistent across tabs');
    if (miscPane.querySelector('.settings-progress,#settings-patch-fill') || document.querySelector('#settings-patch-status').textContent) throw new Error('Misc retained operational patch progress');
    if (getComputedStyle(document.querySelector('#game-settings-status')).display !== 'none') throw new Error('empty game-setting feedback still reserves layout space');
    if ([...document.querySelectorAll('[data-settings-tab]')].map(tab => tab.textContent).join(',') !== 'General,Graphics,Misc') throw new Error('flat Settings categories drifted');
    if (generalPane.querySelector('#profile-form,#profile-server-select') || document.querySelector('[data-settings-pane="controls"],#screenshot-hotkey')) throw new Error('retired profile or Controls content remains in Settings');
    if (document.querySelector('[data-game-settings-tab],[data-settings-pane="runtime"]') || /Diagnostics|Client runtime/i.test(document.querySelector('#screen-settings').textContent)) throw new Error('retired nested or runtime settings remain visible');
    for (const tab of ['general','graphics','misc']) {
      document.querySelector('[data-settings-tab="' + tab + '"]').click();
      const pane = document.querySelector('[data-settings-pane="' + tab + '"]');
      const grid = pane.querySelector('.settings-grid');
      const cards = [...grid.querySelectorAll('.settings-card')];
      if (cards.some(card => getComputedStyle(card).overflowY === 'auto' || card.getBoundingClientRect().height >= pane.getBoundingClientRect().height - 8)) throw new Error(tab + ' Settings cards still own viewport scrolling');
    }
    document.querySelector('[data-settings-tab="misc"]').click();
    await new Promise(resolve => setTimeout(resolve, 100));
    const storageCard = document.querySelector('.settings-card--locations');
    const gamePicker = storageCard.querySelector('#settings-game-path').closest('.settings-path-picker');
    if (storageCard.querySelectorAll('h2').length !== 1 || gamePicker.querySelector('#settings-game-path').textContent !== 'C:/game' || gamePicker.querySelector('[data-settings-action="browse-game"]').textContent !== 'Path' || document.querySelector('#reset-patch-storage,#settings-patch-path')) throw new Error('Misc install folder did not use the path-bar contract');
    if (!document.querySelector('.settings-card--client-patches [data-extension-folder="screenshots"]') || document.querySelectorAll('[data-settings-pane="misc"] .settings-card').length !== 2) throw new Error('Misc did not keep screenshots on the right card');
    if (document.querySelector('[data-extension-folder="logs"]')) throw new Error('Open Logs Folder retained duplicate ownership');
    const screenshotsRow = clientMaintenanceCard.querySelector('.settings-field--screenshots').getBoundingClientRect();
    const rightCard = clientMaintenanceCard.getBoundingClientRect();
    if (Math.abs((rightCard.bottom - 14) - screenshotsRow.bottom) > 1) throw new Error('Open Screenshots Folder is not pinned to the bottom of the right Misc card');
    state.calls.length = 0;
    gamePicker.querySelector('[data-settings-action="browse-game"]').click();
    await new Promise(resolve => setTimeout(resolve, 100));
    if (!state.calls.some(call => call.command === 'pick_install_dir') || state.calls.some(call => call.command === 'set_game_dir')) throw new Error('Misc Install Location did not reuse the existing picker persistence contract');

    state.calls.length = 0;
    const userBackup = clientMaintenanceCard.querySelector('[data-backup-action="create"][data-backup-target="user-settings"]');
    userBackup.click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (!state.calls.some(call => call.command === 'create_backup' && call.args.target === 'user-settings') || document.querySelector('#settings-backup-status').textContent !== 'User Settings and Macros backup created.') throw new Error('User Settings backup did not report backend success');

    const extensionRestore = clientMaintenanceCard.querySelector('[data-backup-action="restore"][data-backup-target="extensions"]');
    state.calls.length = 0;
    extensionRestore.focus();
    extensionRestore.click();
    const restoreDialog = document.querySelector('#settings-confirmation-dialog');
    extensionRestore.click();
    if (!restoreDialog.open || document.activeElement !== document.querySelector('#settings-confirmation-cancel') || state.calls.some(call => call.command === 'restore_backup')) throw new Error('Restore did not open one cancel-focused confirmation');
    if (!document.querySelector('#settings-confirmation-copy').textContent.includes('latest backup?') || [...restoreDialog.querySelectorAll('.confirmation-dialog-actions button')].map(button => button.textContent).join(',') !== 'Confirm,Cancel') throw new Error('Extensions restore did not use the latest-backup Confirm/Cancel wording');
    document.querySelector('#settings-confirmation-cancel').click();
    if (restoreDialog.open || document.activeElement !== extensionRestore || state.calls.some(call => call.command === 'restore_backup')) throw new Error('Restore cancellation did not preserve state and restore focus');
    await Promise.resolve();

    extensionRestore.click();
    document.querySelector('#settings-confirmation-confirm').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (!state.calls.some(call => call.command === 'restore_backup' && call.args.target === 'extensions') || document.querySelector('#settings-backup-status').textContent !== 'Extensions restored.' || document.activeElement !== extensionRestore) throw new Error('Confirmed Extensions restore did not report success and restore focus');
    const userRestore = clientMaintenanceCard.querySelector('[data-backup-action="restore"][data-backup-target="user-settings"]');
    userRestore.click();
    if (!restoreDialog.open || !document.querySelector('#settings-confirmation-copy').textContent.includes('latest backup?') || document.querySelector('#settings-confirmation-confirm').textContent !== 'Confirm') throw new Error('User Settings restore did not use the latest-backup Confirm wording');
    document.querySelector('#settings-confirmation-cancel').click();
  `);

  await assertUi(`
    const state = window.__launcherBrowserState;
    document.querySelector('[data-settings-tab="general"]').click();
    await new Promise(resolve => setTimeout(resolve, 80));
    const settingsScreen = document.querySelector('#screen-settings');
    if (!state.calls.some(call => call.command === 'get_game_settings') || !state.calls.some(call => call.command === 'get_launcher_behavior')) throw new Error('Settings did not hydrate from both typed owners');
    const resolution = document.querySelector('#game-resolution');
    if (resolution.options.length !== 27 || resolution.value !== '1280x720') throw new Error('verified resolution domain did not hydrate');
    if (settingsScreen.querySelector('[data-game-setting-choice="display_mode"][data-game-setting-value="windowed"]').getAttribute('aria-pressed') !== 'true') throw new Error('display mode did not hydrate');
    const borderless = settingsScreen.querySelector('[data-game-setting-choice="display_mode"][data-game-setting-value="borderless"]');
    if (!borderless || borderless.getAttribute('aria-pressed') === 'true') throw new Error('borderless display mode did not hydrate as unselected');
    if (settingsScreen.querySelector('[data-game-setting-choice="graphics.hardware_mouse"][data-game-setting-value="true"]').getAttribute('aria-pressed') !== 'true') throw new Error('hardware mouse did not hydrate');
    const closeOff = settingsScreen.querySelector('[data-launcher-setting-choice="close_on_game_start"][data-launcher-setting-value="false"]');
    if (closeOff.getAttribute('aria-pressed') === 'true') throw new Error('close-on-launch did not hydrate to its enabled default');
    const nativeResolutionOn = settingsScreen.querySelector('[data-launcher-setting-choice="native_resolution_override"][data-launcher-setting-value="true"]');
    const nativeResolutionOff = settingsScreen.querySelector('[data-launcher-setting-choice="native_resolution_override"][data-launcher-setting-value="false"]');
    if (nativeResolutionOff.getAttribute('aria-pressed') !== 'true' || nativeResolutionOn.getAttribute('aria-pressed') === 'true') throw new Error('native resolution override did not hydrate to its disabled default');

    state.calls.length = 0;
    closeOff.click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (!state.calls.some(call => call.command === 'set_close_on_game_start' && call.args.closeOnGameStart === false) || state.launcherBehavior.close_on_game_start || closeOff.getAttribute('aria-pressed') !== 'true') throw new Error('close-on-launch did not persist through its launcher-owned command');

    state.calls.length = 0;
    nativeResolutionOn.click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (!state.calls.some(call => call.command === 'set_native_resolution_override' && call.args.nativeResolutionOverride === true) || !state.launcherBehavior.native_resolution_override || nativeResolutionOn.getAttribute('aria-pressed') !== 'true') throw new Error('native resolution override did not persist through its launcher-owned command');

    state.calls.length = 0;
    borderless.click();
    await new Promise(resolve => setTimeout(resolve, 40));
    let save = state.calls.find(call => call.command === 'set_game_settings');
    if (!save || save.args.settings.display_mode !== 'borderless' || state.gameSettings.settings.display_mode !== 'borderless' || 'initialized' in save.args.settings) throw new Error('borderless display mode did not persist through the strict typed save contract');

    state.calls.length = 0;
    settingsScreen.querySelector('[data-game-setting-choice="display_mode"][data-game-setting-value="fullscreen"]').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    save = state.calls.find(call => call.command === 'set_game_settings');
    if (!save || save.args.settings.display_mode !== 'fullscreen' || 'initialized' in save.args.settings) throw new Error('display mode did not use the strict typed save contract');
    if (state.gameSettings.settings.display_mode !== 'fullscreen' || document.querySelector('#game-settings-status').textContent) throw new Error('display mode did not persist silently');

    state.calls.length = 0;
    resolution.value = '1920x1080';
    resolution.dispatchEvent(new Event('change', { bubbles:true }));
    await new Promise(resolve => setTimeout(resolve, 40));
    save = state.calls.find(call => call.command === 'set_game_settings');
    if (!save || save.args.settings.width !== 1920 || save.args.settings.height !== 1080) throw new Error('resolution did not persist as its verified width/height pair');

    state.calls.length = 0;
    settingsScreen.querySelector('[data-game-setting-choice="graphics.hardware_mouse"][data-game-setting-value="false"]').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    save = state.calls.find(call => call.command === 'set_game_settings');
    if (!save || save.args.settings.graphics.hardware_mouse !== false || state.gameSettings.settings.graphics.hardware_mouse !== false) throw new Error('Hardware Mouse did not persist its typed value');

    state.calls.length = 0;
    settingsScreen.querySelector('[data-game-setting-choice="audio.enabled"][data-game-setting-value="false"]').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    save = state.calls.find(call => call.command === 'set_game_settings');
    if (!save || save.args.settings.audio.enabled !== false || state.gameSettings.settings.audio.enabled !== false) throw new Error('Sound did not persist its typed value');

    state.calls.length = 0;
    settingsScreen.querySelector('[data-game-setting-choice="audio.play_in_background"][data-game-setting-value="true"]').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    save = state.calls.find(call => call.command === 'set_game_settings');
    if (!save || save.args.settings.audio.play_in_background !== true || state.gameSettings.settings.audio.play_in_background !== true || 'initialized' in save.args.settings) throw new Error('Always Play Sound did not persist its typed value');

    document.querySelector('[data-settings-tab="graphics"]').click();
    if (!document.querySelector('[data-settings-pane="graphics"]').hasAttribute('data-active')) throw new Error('flat Settings tab navigation failed');
    const distanceRange = settingsScreen.querySelector('[data-object-distance-selection]');
    const zoomRange = settingsScreen.querySelector('[data-camera-zoom-selection]');
    const chooseGraphicsStep = (range, step) => {
      range.value = String(step);
      range.dispatchEvent(new Event('input', { bubbles:true }));
      range.dispatchEvent(new Event('change', { bubbles:true }));
    };
    if (!state.calls.some(call => call.command === 'get_object_distance_selection') || distanceRange.disabled || distanceRange.value !== '0' || distanceRange.getAttribute('aria-valuetext') !== 'Off') throw new Error('Draw Distance did not hydrate to Off');
    state.calls.length = 0;
    chooseGraphicsStep(distanceRange, 2);
    await new Promise(resolve => setTimeout(resolve, 40));
    if (state.calls.filter(call => call.command === 'set_object_distance_selection').length !== 1 || state.calls.find(call => call.command === 'set_object_distance_selection').args.percent !== 150 || !state.objectDistanceEnabled || state.objectDistancePercent !== 150 || distanceRange.value !== '2' || distanceRange.getAttribute('aria-valuetext') !== '1.5x') throw new Error('Draw Distance bar did not save its multiplier');
    await hydrateSettings();
    if (distanceRange.value !== '2') throw new Error('Draw Distance did not retain its saved selection after hydration');
    state.failObjectDistanceWrite = true;
    chooseGraphicsStep(distanceRange, 0);
    await new Promise(resolve => setTimeout(resolve, 40));
    if (!state.objectDistanceEnabled || distanceRange.value !== '2' || !document.querySelector('#object-distance-status').textContent.includes('fixture draw distance write failure')) throw new Error('failed Draw Distance write did not restore the saved selection');
    chooseGraphicsStep(distanceRange, 0);
    await new Promise(resolve => setTimeout(resolve, 40));
    if (state.objectDistanceEnabled || distanceRange.value !== '0' || state.objectDistancePercent !== 150) throw new Error('Draw Distance Off did not disable the hook and retain the saved multiplier');
    if (!state.calls.some(call => call.command === 'get_camera_zoom_selection') || zoomRange.disabled || zoomRange.value !== '0' || zoomRange.getAttribute('aria-valuetext') !== 'Off') throw new Error('Camera Zoom did not hydrate to Off');
    state.calls.length = 0;
    chooseGraphicsStep(zoomRange, 2);
    await new Promise(resolve => setTimeout(resolve, 40));
    if (state.calls.filter(call => call.command === 'set_camera_zoom_selection').length !== 1 || state.calls.find(call => call.command === 'set_camera_zoom_selection').args.limit !== 12 || !state.cameraZoomEnabled || state.cameraZoomLimit !== 12 || zoomRange.value !== '2' || zoomRange.getAttribute('aria-valuetext') !== '12') throw new Error('Camera Zoom bar did not save its cap');
    await hydrateSettings();
    if (zoomRange.value !== '2') throw new Error('Camera Zoom did not retain its saved selection after hydration');
    state.failCameraZoomWrite = true;
    chooseGraphicsStep(zoomRange, 0);
    await new Promise(resolve => setTimeout(resolve, 40));
    if (!state.cameraZoomEnabled || zoomRange.value !== '2' || !document.querySelector('#camera-zoom-status').textContent.includes('fixture camera zoom write failure')) throw new Error('failed Camera Zoom write did not restore the saved selection');
    chooseGraphicsStep(zoomRange, 0);
    await new Promise(resolve => setTimeout(resolve, 40));
    if (state.cameraZoomEnabled || zoomRange.value !== '0' || state.cameraZoomLimit !== 12) throw new Error('Camera Zoom Off did not disable the hook and retain the saved cap');
    const textureQuality = settingsScreen.querySelector('[data-game-setting-choice="graphics.texture_quality"][data-game-setting-value="low"]');
    const textureFiltering = settingsScreen.querySelector('[data-game-setting="graphics.texture_filtering"]');
    if (textureFiltering.value !== '1' || getComputedStyle(textureFiltering.nextElementSibling).gridTemplateColumns.split(' ').length !== 2) throw new Error('Texture Filtering bar did not hydrate its Standard setting');
    if ([...textureQuality.closest('.choice-row').querySelectorAll('button')].map(button => button.dataset.gameSettingValue).join(',') !== 'low,standard,high') throw new Error('texture quality choices are not ordered lowest to highest');
    state.calls.length = 0;
    textureFiltering.value = '3';
    textureFiltering.dispatchEvent(new Event('input', { bubbles:true }));
    textureFiltering.dispatchEvent(new Event('change', { bubbles:true }));
    await new Promise(resolve => setTimeout(resolve, 40));
    if (!state.calls.some(call => call.command === 'set_game_settings' && call.args.settings.graphics.texture_filtering === 'highest') || state.gameSettings.settings.graphics.texture_filtering !== 'highest' || textureFiltering.value !== '3') throw new Error('Texture Filtering bar did not persist Highest');
    state.calls.length = 0;
    const multisampling = settingsScreen.querySelector('[data-game-setting-choice="graphics.multisampling"][data-game-setting-value="4x"]');
    multisampling.click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (!state.calls.some(call => call.command === 'set_game_settings' && call.args.settings.graphics.multisampling === '4x')) throw new Error('graphics enum did not persist');

    state.gameSettingsWriteDelayMs = 60;
    state.maxGameSettingsWritesInFlight = 0;
    const generalQuality = document.querySelector('#game-general-quality');
    generalQuality.value = '9';
    generalQuality.dispatchEvent(new Event('change', { bubbles:true }));
    textureQuality.click();
    await new Promise(resolve => setTimeout(resolve, 160));
    state.gameSettingsWriteDelayMs = 0;
    if (state.maxGameSettingsWritesInFlight !== 1 || state.gameSettings.settings.graphics.general_quality !== 9 || state.gameSettings.settings.graphics.texture_quality !== 'low') throw new Error('rapid full-payload writes were not serialized without losing an edit');

    state.failGameSettingsWrite = true;
    const failedChoice = settingsScreen.querySelector('[data-game-setting-choice="graphics.depth_of_field"][data-game-setting-value="true"]');
    failedChoice.click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (state.gameSettings.settings.graphics.depth_of_field || failedChoice.getAttribute('aria-pressed') === 'true' || !document.querySelector('#game-settings-status').textContent.includes('fixture game settings write failure')) throw new Error('failed game-setting write did not restore authoritative state');

    activateSettingsTab('general');
    runControllerAction('cycle-settings');
    if (!document.querySelector('[data-settings-pane="graphics"]').hasAttribute('data-active')) throw new Error('gamepad tab action did not cycle the flat Settings surface');

    const available = structuredClone(state.gameSettings);
    state.calls.length = 0;
    state.gameSettings.available = false;
    renderGameSettings(state.gameSettings);
    if (![...settingsScreen.querySelectorAll('[data-game-setting],[data-game-setting-choice]')].every(control => control.disabled) || !document.querySelector('#game-settings-status').textContent.includes('valid retail config.sys')) throw new Error('unavailable retail mapping did not disable every control truthfully');
    const availableGeneralRows = [...document.querySelector('[data-settings-pane="general"] .settings-card').querySelectorAll('[data-gamepad-settings-row]')]
      .filter(row => row.querySelector('button:not(:disabled), select:not(:disabled), input:not(:disabled)'));
    if (availableGeneralRows.length !== 1 || !availableGeneralRows[0].querySelector('[data-launcher-setting-choice]')) throw new Error('gamepad navigation did not exclude unavailable game-owned rows');
    const warningBeforeLauncherSave = document.querySelector('#game-settings-status').textContent;
    const availableBeforeLauncherSave = window.__launcherSettings.gameSettings?.available;
    await queueLauncherBehaviorUpdate(true);
    if (!document.querySelector('#game-settings-status').textContent.includes('valid retail config.sys')) throw new Error('launcher-owned setting save erased the unavailable game-settings warning: ' + document.querySelector('#game-settings-status').textContent + ' / ' + JSON.stringify({warningBeforeLauncherSave,availableBeforeLauncherSave,availableAfterLauncherSave:window.__launcherSettings.gameSettings?.available,calls:state.calls.map(call=>call.command)}));
    state.gameSettings = available;
    renderGameSettings(state.gameSettings);

    document.querySelector('[data-route="home"]').click();
    state.mode = 'success';
    document.querySelector('#login-username').value = 'aesh';
    document.querySelector('#login-password').value = 'password';
    document.querySelector('#login-form').dispatchEvent(new Event('submit', { bubbles:true, cancelable:true }));
    await new Promise(resolve => setTimeout(resolve, 50));
    document.querySelector('[data-route="settings"]').click();
    await new Promise(resolve => setTimeout(resolve, 80));
    if (document.querySelector('#game-resolution').value !== '1920x1080' || document.querySelector('[data-game-setting-choice="graphics.multisampling"][data-game-setting-value="4x"]').getAttribute('aria-pressed') !== 'true') throw new Error('logged-in Settings did not retain authoritative game values');
    document.querySelector('[data-route="home"]').click();
    state.readyStatusDelayMs = 80;
    const staleReadyRefresh = window.refreshHomeStatus();
    await new Promise(resolve => setTimeout(resolve, 5));
    document.querySelector('[data-action="logout"]').click();
    await staleReadyRefresh;
    state.readyStatusDelayMs = 0;
    if (document.querySelector('#home-layout').dataset.lifecycle !== 'logged-out' || !document.querySelector('#login-form')) throw new Error('a stale authenticated refresh restored Ready after logout');
    document.querySelector('[data-route="settings"]').click();
    await new Promise(resolve => setTimeout(resolve, 80));
    if (!document.querySelector('[data-screen="settings"][data-active]') || document.querySelector('#game-resolution').value !== '1920x1080' || document.querySelector('[data-game-setting-choice="graphics.multisampling"][data-game-setting-value="4x"]').getAttribute('aria-pressed') !== 'true') throw new Error('logged-out Settings did not retain authoritative game values');
  `);

  await devtools.send('Emulation.setDeviceMetricsOverride', {
    width: 600,
    height: 400,
    screenWidth: 600,
    screenHeight: 400,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await assertUi(`
    const state = window.__launcherBrowserState;
    document.querySelector('[data-settings-tab="general"]').click();
    await new Promise(resolve => requestAnimationFrame(resolve));
    const pane = document.querySelector('[data-settings-pane="general"]');
    const grid = pane.querySelector('.settings-grid');
    const cards = [...grid.querySelectorAll('.settings-card')];
    if (cards.length !== 2 || getComputedStyle(grid).gridTemplateColumns.split(' ').length !== 1) throw new Error('narrow General settings did not preserve two cards in one column');
    if (getComputedStyle(pane).overflowY !== 'auto' || pane.scrollHeight <= pane.clientHeight) throw new Error('narrow Settings pane is not the sole scroll owner');
    if (cards.some(card => card.scrollWidth > card.clientWidth) || [...pane.querySelectorAll('.settings-field')].some(field => field.scrollWidth > field.clientWidth)) throw new Error('narrow Settings content overflows its cards');
    document.querySelector('[data-settings-tab="graphics"]').click();
    await new Promise(resolve => requestAnimationFrame(resolve));
    const graphicsPane = document.querySelector('[data-settings-pane="graphics"]');
    const graphicsCards = [...graphicsPane.querySelectorAll('.settings-card')];
    if (graphicsCards.length !== 2 || graphicsPane.scrollWidth > graphicsPane.clientWidth || graphicsCards.some(card => card.scrollWidth > card.clientWidth) || [...graphicsPane.querySelectorAll('.settings-field')].some(field => field.scrollWidth > field.clientWidth || Number.parseFloat(getComputedStyle(field).minHeight) < 76)) throw new Error('narrow Graphics settings did not preserve consistent bounded rows');
    document.querySelector('[data-settings-tab="misc"]').click();
    await new Promise(resolve => requestAnimationFrame(resolve));
    const miscPane = document.querySelector('[data-settings-pane="misc"]');
    const miscCards = [...miscPane.querySelectorAll('.settings-card')];
    if (miscCards.length !== 2 || miscPane.scrollWidth > miscPane.clientWidth || miscCards.some(card => card.scrollWidth > card.clientWidth) || [...miscPane.querySelectorAll('.settings-field,.settings-path-picker,.settings-actions-stack')].some(item => item.scrollWidth > item.clientWidth)) throw new Error('narrow Misc cards did not preserve consistent bounded spacing');
    document.querySelector('[data-route="home"]').click();
    document.querySelector('[data-action="open-profiles"]').click();
    await new Promise(resolve => requestAnimationFrame(resolve));
    const profilesPage = document.querySelector('.profiles-page');
    const profilesGrid = document.querySelector('.profiles-grid');
    if (getComputedStyle(profilesGrid).gridTemplateColumns.split(' ').length !== 1 || profilesPage.scrollWidth > profilesPage.clientWidth) throw new Error('narrow Profiles did not collapse without horizontal overflow');
    document.querySelector('[data-route="extensions"]').click();
    await new Promise(resolve => setTimeout(resolve, 80));
    const narrowExtensions = document.querySelector('.extensions-page');
    if (getComputedStyle(narrowExtensions).gridTemplateColumns.split(' ').length !== 1 || narrowExtensions.scrollWidth > narrowExtensions.clientWidth) throw new Error('narrow Extensions did not collapse without horizontal overflow');
    document.querySelector('[data-utility="gamepad"]').click();
    await new Promise(resolve => requestAnimationFrame(resolve));
    const narrowGamepadPage = document.querySelector('.gamepad-page');
    const narrowGamepadGrid = document.querySelector('.gamepad-grid');
    if (getComputedStyle(narrowGamepadGrid).gridTemplateColumns.split(' ').length !== 1 || getComputedStyle(narrowGamepadPage).overflowY !== 'auto' || narrowGamepadPage.scrollHeight <= narrowGamepadPage.clientHeight || narrowGamepadPage.scrollWidth > narrowGamepadPage.clientWidth || [...narrowGamepadGrid.querySelectorAll('.gamepad-card')].some(card => card.scrollWidth > card.clientWidth)) throw new Error('narrow Gamepad cards did not collapse into one scrolling page');
    document.querySelector('[data-route="settings"]').click();
    const installCard = document.querySelector('.settings-card--locations');
    const updateButton = installCard?.querySelector('[data-launcher-update-action]');
    const repairButton = installCard?.querySelector('[data-game-files-action="repair"]');
    if (!installCard || !updateButton || !repairButton || document.querySelector('.settings-card--launcher-updates,.settings-card--game-files,[data-launcher-repair-action],#game-files-search')) throw new Error('Misc did not keep one update and one repair button in Install Location');
    if (state.launcherUpdateChecks < 1) throw new Error('launcher did not start a quiet metadata check at startup');
    state.calls.length = 0;
    state.launcherUpdateStatus = { state:'ready', message:'Ready to check.', installedVersion:'1.0.0', offeredVersion:null };
    state.launcherUpdateCheckResult = { state:'update_available', message:'Version 1.1.0 available.', installedVersion:'1.0.0', offeredVersion:'1.1.0' };
    await window.hydrateLauncherUpdates();
    if (updateButton.textContent !== 'Check for Updates' || updateButton.dataset.launcherUpdateAction !== 'check' || updateButton.disabled) throw new Error('launcher update button did not start as a check');
    state.calls.length = 0;
    await window.handleLauncherUpdateAction({ target:updateButton });
    if (!state.calls.some(call => call.command === 'check_launcher_update') || state.calls.some(call => call.command === 'apply_launcher_update') || updateButton.textContent !== 'Update Launcher' || updateButton.dataset.launcherUpdateAction !== 'apply' || !document.querySelector('#launcher-update-status').textContent.includes('1.1.0')) throw new Error('metadata check did not reveal the second-click update action');
    state.launcherUpdateRestartAvailable = false;
    await window.refreshLauncherUpdateAvailability();
    if (!updateButton.disabled) throw new Error('launcher update remained enabled while another operation was active');
    state.launcherUpdateRestartAvailable = true;
    await window.refreshLauncherUpdateAvailability();
    if (updateButton.disabled) throw new Error('launcher update did not become available again');
    const updateNotice = installCard.querySelector('#launcher-update-status');
    const cardHeightBeforeError = installCard.getBoundingClientRect().height;
    const repairTopBeforeError = repairButton.getBoundingClientRect().top;
    state.calls.length = 0;
    state.failLauncherUpdateAction = 'apply';
    await window.handleLauncherUpdateAction({ target:updateButton });
    if (!state.calls.some(call => call.command === 'apply_launcher_update') || state.calls.some(call => call.command === 'control_window') || !document.querySelector('#launcher-update-status').textContent.includes('fixture launcher update apply failure')) throw new Error('failed launcher update closed the launcher or hid its error');
    if (updateNotice.parentElement !== installCard || Math.abs(installCard.getBoundingClientRect().height - cardHeightBeforeError) > 1 || Math.abs(repairButton.getBoundingClientRect().top - repairTopBeforeError) > 1 || Math.abs(updateNotice.getBoundingClientRect().bottom - (installCard.getBoundingClientRect().bottom - 14)) > 2) throw new Error('launcher update error moved card controls instead of staying at the bottom');
    const shortError = updateNotice.textContent;
    updateNotice.textContent = shortError.repeat(20);
    if (Math.abs(installCard.getBoundingClientRect().height - cardHeightBeforeError) > 1 || Math.abs(repairButton.getBoundingClientRect().top - repairTopBeforeError) > 1 || getComputedStyle(updateNotice).overflowY !== 'hidden') throw new Error('long launcher update error grew the card or added scrolling');
    updateNotice.textContent = shortError;
    state.calls.length = 0;
    state.failLauncherUpdateAction = null;

    state.calls.length = 0;
    repairButton.click();
    const dialog = document.querySelector('#settings-confirmation-dialog');
    const dialogActions = [...dialog.querySelectorAll('.confirmation-dialog-actions button')];
    if (!dialog.open || document.querySelector('#settings-confirmation-copy').textContent !== 'This may require downloading the full 7.2 GB game archive. Proceed?' || dialogActions.map(button => button.textContent).join(',') !== 'Confirm,Cancel' || document.activeElement !== dialogActions[1]) throw new Error('Repair Install did not show concise Confirm/Cancel dialog with safe Cancel focus');
    document.querySelector('#settings-confirmation-cancel').click();
    await new Promise(resolve => setTimeout(resolve, 20));
    if (state.calls.some(call => call.command === 'start_game_repair')) throw new Error('cancelled Repair Install started work');
    state.gameRepairStartError = 'fixture repair start failure';
    repairButton.click();
    document.querySelector('#settings-confirmation-confirm').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    await window.refreshGameRepairStatus();
    window.__launcherModules.homeModule.renderLifecycleStrip();
    if (document.querySelector('#lifecycle-title').textContent !== 'Repair Failed' || !document.querySelector('#lifecycle-copy').textContent.includes('fixture repair start failure')) throw new Error('repair start error disappeared after idle backend status');
    state.gameRepairStartError = null;
    document.querySelector('[data-route="settings"]').click();
    document.querySelector('[data-settings-tab="misc"]').click();
    state.calls.length = 0;
    repairButton.click();
    document.querySelector('#settings-confirmation-confirm').click();
    await new Promise(resolve => setTimeout(resolve, 40));
    if (!state.calls.some(call => call.command === 'start_game_repair') || !document.querySelector('[data-screen="home"][data-active]') || document.querySelector('#lifecycle-title').textContent !== 'Repair Install' || document.querySelector('#lifecycle-pause').hidden || document.querySelector('#lifecycle-cancel').hidden) throw new Error('Repair Install did not open the Home Install Strip with Pause and Cancel');
    document.querySelector('#lifecycle-pause').click();
    await new Promise(resolve => setTimeout(resolve, 30));
    if (!state.calls.some(call => call.command === 'pause_game_repair') || document.querySelector('#lifecycle-pause').textContent !== 'Resume') throw new Error('Repair Install pause was not shown');
    document.querySelector('#lifecycle-pause').click();
    await new Promise(resolve => setTimeout(resolve, 30));
    if (!state.calls.some(call => call.command === 'resume_game_repair') || document.querySelector('#lifecycle-pause').textContent !== 'Pause') throw new Error('Repair Install did not resume');
    document.querySelector('#lifecycle-cancel').click();
    await new Promise(resolve => setTimeout(resolve, 30));
    if (!state.calls.some(call => call.command === 'cancel_game_repair') || document.querySelector('#lifecycle-title').textContent !== 'Repair Cancelled' || getComputedStyle(document.querySelector('#lifecycle-strip')).display === 'none') throw new Error('Repair Install did not leave a visible cancelled result in the strip');
    state.gameRepairStatus = { ...state.gameRepairStatus, phase:'idle', is_running:false, is_terminal:false };
    await window.refreshGameRepairStatus();
    window.__launcherModules.homeModule.renderLifecycleStrip();
  `);
  await devtools.send('Emulation.setDeviceMetricsOverride', {
    width: 1280,
    height: 800,
    screenWidth: 1280,
    screenHeight: 800,
    deviceScaleFactor: 1,
    mobile: false,
  });

  await assertUi(`
    const state = window.__launcherBrowserState;
    if (document.querySelector('#login-server')) throw new Error('Home still owns the server selector');
    document.querySelector('[data-route="home"]').click();
    document.querySelector('[data-action="open-profiles"]').click();
    await new Promise(resolve => setTimeout(resolve, 80));
    const profilesScreen = document.querySelector('#screen-profiles');
    const select = document.querySelector('#profile-server-select');
    if (!profilesScreen.hasAttribute('data-active') || document.querySelector('[data-route][aria-selected="true"]')) throw new Error('Profiles did not open its dedicated page');
    if (select.value !== 'Local' || select.options.length !== 2) throw new Error('Profiles did not hydrate server profiles');
    const profileCards = [...profilesScreen.querySelectorAll('.profile-card')];
    if (document.querySelector('.profiles-page > .profiles-back')?.dataset.action !== 'close-profiles') throw new Error('Profiles back action is not above the cards');
    const initialCardHeights = profileCards.map(card => card.getBoundingClientRect().height);
    state.calls.length = 0;
    select.value = 'Bahamut';
    select.dispatchEvent(new Event('change', { bubbles:true }));
    await new Promise(resolve => setTimeout(resolve, 50));
    if (!state.calls.some(call => call.command === 'set_selected_server' && call.args.displayName === 'Bahamut')) throw new Error('Profiles server selection did not persist');
    if (document.querySelector('#profile-host').value !== 'bahamut.example' || !document.querySelector('#profile-https').checked) throw new Error('selected server did not populate the editor: ' + JSON.stringify({ host:document.querySelector('#profile-host').value, https:document.querySelector('#profile-https').checked, selected:select.value, calls:state.calls }));
    if (document.querySelector('#profile-server-select-status').textContent || document.querySelector('#profile-server-select-status').dataset.tone || profileCards.some((card, index) => card.getBoundingClientRect().height !== initialCardHeights[index])) throw new Error('successful server selection produced redundant feedback or changed profile geometry');
    state.profileMode = 'select-error';
    select.value = 'Local';
    select.dispatchEvent(new Event('change', { bubbles:true }));
    await new Promise(resolve => setTimeout(resolve, 50));
    if (document.querySelector('#profile-server-select-status').dataset.tone !== 'error' || profileCards.some((card, index) => card.getBoundingClientRect().height !== initialCardHeights[index])) throw new Error('server selection error changed profile geometry or lacked error semantics');
    state.profileMode = 'success';
    select.value = 'Bahamut';
    select.dispatchEvent(new Event('change', { bubbles:true }));
    await new Promise(resolve => setTimeout(resolve, 50));
    state.calls.length = 0;
    document.querySelector('#profile-host').value = 'new.example';
    document.querySelector('#profile-form').requestSubmit();
    await new Promise(resolve => setTimeout(resolve, 50));
    const save = state.calls.find(call => call.command === 'save_server_profile');
    if (!save || save.args.originalDisplayName !== 'Bahamut' || save.args.profile.host !== 'new.example') throw new Error('Profiles server editor did not persist the profile');
    if (!document.querySelector('#profile-save-status').textContent.includes('saved')) throw new Error('server save success was not reported');
    if (document.querySelector('#profile-save-status').dataset.tone !== 'success' || profileCards.some((card, index) => card.getBoundingClientRect().height !== initialCardHeights[index])) throw new Error('profile save status changed profile geometry or used error semantics');
    await new Promise(resolve => setTimeout(resolve, 2700));
    if (document.querySelector('#profile-save-status').textContent || document.querySelector('#profile-server-select-status').textContent) throw new Error('profile success feedback did not clear promptly');
    if (profileCards.some((card, index) => card.getBoundingClientRect().height !== initialCardHeights[index])) throw new Error('clearing profile feedback changed profile geometry');
  `);

  await assertUi(`
    const state = window.__launcherBrowserState;
    state.calls.length = 0;
    document.querySelector('[data-route="extensions"]').click();
    await new Promise(resolve => setTimeout(resolve, 80));
    const extensions = document.querySelector('#screen-extensions');
    if (extensions.querySelectorAll('.extension-library,.extension-detail-column').length !== 2 || getComputedStyle(extensions.querySelector('.extensions-page')).gridTemplateColumns.split(' ').length !== 2) throw new Error('Extensions is not a two-pane view');
    if (Math.abs(extensions.querySelector('.extension-library').getBoundingClientRect().width - 404) >= .1) throw new Error('Extensions library is not using the measured Avalon width');
    const extensionsPage = extensions.querySelector('.extensions-page');
    const extensionsPageRect = extensionsPage.getBoundingClientRect();
    const extensionRegions = [...extensions.querySelectorAll('[data-gamepad-region]')];
    const folderDock = extensions.querySelector('.extension-folder-actions');
    const folderDockRect = folderDock.getBoundingClientRect();
    if (folderDock.parentElement !== extensionsPage || extensionRegions.some(region => {
      const rect = region.getBoundingClientRect();
      return rect.left - extensionsPageRect.left < 3 || extensionsPageRect.right - rect.right < 3 || rect.top - extensionsPageRect.top < 3 || extensionsPageRect.bottom - rect.bottom < 3;
    })) throw new Error('Extensions regions do not reserve a stable, unclipped focus-ring gutter');
    if (/Local components|Only installed first-party components are shown/.test(extensions.textContent)) throw new Error('Extensions retained the removed library intro');
    if ([...extensions.querySelectorAll('.extension-group-title')].map(node => node.textContent).join(',') !== 'Overlays,Plugins,Addons') throw new Error('Extensions package groups drifted');
    if ([...extensions.querySelectorAll('[data-extension-group="plugins"] .extension-row-name')].map(node => node.textContent).join(',') !== 'DiscordRPC,Screenshot' || [...extensions.querySelectorAll('[data-extension-group="addons"] .extension-row-name')].map(node => node.textContent).join(',') !== 'fps,wiki') throw new Error('Plugins and addons are not alphabetically ordered');
    if (extensions.querySelectorAll('.extension-row').length !== 5 || extensions.querySelector('[data-extension-item="overlays:dats-overlay"] .extension-row-name').textContent !== 'Dats-Overlay' || extensions.querySelector('[data-extension-item^="overlays:"]:not([data-extension-item="overlays:dats-overlay"])') || extensions.querySelector('[data-extension-item="plugins:screenshot"] .extension-row-name').textContent !== 'Screenshot' || extensions.querySelector('[data-extension-item="plugins:discord-rpc"] .extension-row-name').textContent !== 'DiscordRPC' || extensions.querySelector('[data-extension-item="addons:fps"] .extension-row-name').textContent !== 'fps' || extensions.querySelector('[data-extension-item="addons:wiki"] .extension-row-name').textContent !== 'wiki') throw new Error('Extensions did not keep DAT packages inside Dats-Overlay');
    const addonToggle = extensions.querySelector('[data-addon-enabled="fps"]');
    const screenshotToggle = extensions.querySelector('[data-plugin-enabled="screenshot"]');
    const discordToggle = extensions.querySelector('[data-plugin-enabled="discord-rpc"]');
    if (!addonToggle || !addonToggle.checked || !screenshotToggle || !screenshotToggle.checked || !discordToggle || !discordToggle.checked) throw new Error('Extensions did not render persisted addon and plugin enable controls');
    if (extensions.querySelector('[data-extension-group="overlays"] input')) throw new Error('Dats-Overlay must remain the only locked extension row');
    if (extensions.textContent.includes('Bahamut Client Runtime') || extensions.textContent.includes('Core')) throw new Error('Extensions retained the Settings-owned Core runtime item');
    if (extensions.querySelector('[data-runtime-enabled],[data-settings-action],[data-extension-action]')) throw new Error('Extensions still owns writable runtime controls');
    if (/Refresh diagnostics|Enable client runtime|Update channel|Overlay hotkey|Overlay scale/.test(extensions.textContent)) throw new Error('Extensions retained Settings-owned or unsupported controls');
    let datsDetail = extensions.querySelector('#extension-detail');
    if (datsDetail.querySelector('h2').textContent !== 'Dats-Overlay' || datsDetail.querySelector('.extension-description').textContent !== 'Rearrange installed overlays to set DAT load priority.' || datsDetail.querySelectorAll('.extension-overlay-package').length !== 3) throw new Error('Extensions did not render the Dats-Overlay package editor');
    if (datsDetail.querySelector('.extension-overlay-editor-heading') || datsDetail.textContent.includes('Overlay packages') || datsDetail.textContent.includes('Enabled packages are applied in first-hit order.')) throw new Error('Dats-Overlay retained the removed visible package copy');
    if (datsDetail.querySelector('.extension-overlay-editor')?.getAttribute('aria-label') !== 'Overlay package controls') throw new Error('Dats-Overlay package controls lost their accessible label');
    if (!datsDetail.querySelector('[data-dat-package-enabled="base-world"]')?.checked || !datsDetail.querySelector('[data-dat-package-enabled="detail-world"]')?.checked || datsDetail.querySelector('[data-dat-package-enabled="bahamut-dats-overlay"]')) throw new Error('Dats package enablement did not hydrate or official control remained');
    if (datsDetail.querySelector('.extension-homepage') || !datsDetail.querySelector('[data-dat-conflict-summary]')?.textContent.includes('data/2A/08.DAT')) throw new Error('Dats package metadata or conflicts are not visible or safe');
    if (datsDetail.querySelector('.extension-overlay-package .extension-meta,.extension-overlay-package-description,.extension-overlay-package-head,.extension-overlay-order') || /Author:|Version:|First-hit order|Move earlier|Move later/.test([...datsDetail.querySelectorAll('.extension-overlay-package')].map(row => row.textContent).join(' '))) throw new Error('Dats package rows retained manifest fluff');
    if ([...datsDetail.querySelectorAll('.extension-overlay-order-label')].map(node => node.textContent).join(',') !== '#1,#2,#3' || datsDetail.querySelector('[data-dat-move][data-dat-package-id="bahamut-dats-overlay"]') || datsDetail.querySelector('[data-dat-move="earlier"][data-dat-package-id="base-world"]')?.textContent !== '↑' || datsDetail.querySelector('[data-dat-move="later"][data-dat-package-id="base-world"]')?.textContent !== '↓' || !datsDetail.querySelector('[data-dat-move="earlier"][data-dat-package-id="base-world"]').disabled || !datsDetail.querySelector('[data-dat-move="later"][data-dat-package-id="detail-world"]').disabled) throw new Error('Dats priority controls did not lock official slot #1');
    const datsRow = extensions.querySelector('[data-extension-item="overlays:dats-overlay"]').closest('.extension-row').getBoundingClientRect();
    const datsName = extensions.querySelector('[data-extension-item="overlays:dats-overlay"] .extension-row-name').getBoundingClientRect();
    const fpsName = extensions.querySelector('[data-extension-item="addons:fps"] .extension-row-name').getBoundingClientRect();
    if (Math.abs(datsName.left - fpsName.left) >= 1 || extensions.querySelector('.extension-kind,.extension-row-reason')) throw new Error('Dats-Overlay is not aligned with addon and plugin labels');
    const libraryLeft = extensions.querySelector('.extension-library').getBoundingClientRect().left;
    const rowInset = datsRow.left - libraryLeft;
    const datsRowElement = extensions.querySelector('[data-extension-item="overlays:dats-overlay"]').closest('.extension-row');
    if (rowInset < 11.9 || rowInset > 13.1 || datsRowElement.dataset.current !== 'true' || datsRowElement.dataset.enabled !== 'true') throw new Error('Extensions built-in overlay is not enabled, inset, and initialized like Avalon (' + rowInset + 'px)');
    document.body.dataset.inputMode = 'gamepad';
    const extensionLibrary = extensions.querySelector('.extension-library');
    extensionLibrary.focus();
    runControllerAction('down');
    if (document.activeElement?.dataset.extensionItem !== 'plugins:discord-rpc') throw new Error('D-pad did not move from Dats-Overlay to the next service when entering Extensions');
    runControllerAction('up');
    if (document.activeElement?.dataset.extensionItem !== 'overlays:dats-overlay') throw new Error('Extensions D-pad moved above the first row into Search');
    runControllerAction('down');
    if (document.activeElement?.dataset.extensionItem !== 'plugins:discord-rpc' || document.activeElement.closest('.extension-row').dataset.current !== 'true') throw new Error('Extensions D-pad did not move and select whole rows');
    const selectedExtensionButton = document.activeElement;
    const selectionBeforeHorizontalRow = extensions.querySelector('.extension-row[data-current="true"] .extension-row-select')?.dataset.extensionItem;
    runControllerAction('left');
    runControllerAction('right');
    if (document.activeElement !== selectedExtensionButton || extensions.querySelector('.extension-row[data-current="true"] .extension-row-select')?.dataset.extensionItem !== selectionBeforeHorizontalRow || extensions.querySelectorAll('.extension-row[data-current="true"]').length !== 1) throw new Error('Extensions row accepted horizontal D-pad navigation or rendered multiple selections');
    runControllerAction('up');
    if (document.activeElement?.dataset.extensionItem !== 'overlays:dats-overlay' || extensions.querySelectorAll('.extension-row[data-current="true"]').length !== 1) throw new Error('Extensions D-pad did not move upward to one selected row');
    runControllerAction('down');
    if (document.activeElement?.dataset.extensionItem !== 'plugins:discord-rpc' || extensions.querySelectorAll('.extension-row[data-current="true"]').length !== 1) throw new Error('Extensions D-pad did not return downward to one selected row');
    runControllerAction('down');
    if (document.activeElement?.dataset.extensionItem !== 'plugins:screenshot') throw new Error('Extensions D-pad did not reach Screenshot');
    runControllerAction('down');
    if (document.activeElement?.dataset.extensionItem !== 'addons:fps') throw new Error('Extensions D-pad did not reach FPS');
    runControllerAction('down');
    if (document.activeElement?.dataset.extensionItem !== 'addons:wiki') throw new Error('Extensions D-pad did not reach the final row');
    const finalExtensionButton = document.activeElement;
    runControllerAction('down');
    if (document.activeElement !== finalExtensionButton || extensions.querySelectorAll('.extension-row[data-current="true"]').length !== 1) throw new Error('Extensions D-pad did not clamp at the final row');
    runControllerAction('next-route');
    if (!document.querySelector('[data-screen="settings"][data-active]')) throw new Error('RT did not leave Extensions for Settings');
    runControllerAction('previous-route');
    await new Promise(resolve => setTimeout(resolve, 30));
    if (!document.querySelector('[data-screen="extensions"][data-active]') || document.activeElement !== extensionLibrary || extensions.querySelector('.extension-row[data-current="true"] .extension-row-select')?.dataset.extensionItem !== 'addons:wiki') throw new Error('LT did not restore the Extensions library and selected row');
    runControllerAction('up');
    if (document.activeElement?.dataset.extensionItem !== 'addons:fps') throw new Error('Extensions route return consumed the first row movement');
    runControllerAction('up');
    if (document.activeElement?.dataset.extensionItem !== 'plugins:screenshot') throw new Error('Extensions route return consumed the first row movement');
    runControllerAction('down');
    if (document.activeElement?.dataset.extensionItem !== 'addons:fps') throw new Error('Extensions route return did not move smoothly through FPS');
    runControllerAction('down');
    if (document.activeElement?.dataset.extensionItem !== 'addons:wiki') throw new Error('Extensions route return did not move smoothly back to the final row');
    const selectedExtensionBeforeSearch = document.activeElement;
    runControllerAction('social-mode');
    const extensionSearchControl = extensions.querySelector('#extension-search');
    if (document.activeElement !== extensionSearchControl || extensions.querySelector('.extension-row[data-current="true"] .extension-row-select') !== selectedExtensionBeforeSearch) throw new Error('View did not enter Extensions Search while retaining the selected row');
    if (getComputedStyle(selectedExtensionBeforeSearch.closest('.extension-row')).boxShadow !== 'none') throw new Error('selected extension retained its selection ring while Search had focus');
    const selectionBeforeDirectionalSearch = extensions.querySelector('.extension-row[data-current="true"] .extension-row-select')?.dataset.extensionItem;
    runControllerAction('up');
    runControllerAction('down');
    runControllerAction('left');
    runControllerAction('right');
    if (document.activeElement !== extensionSearchControl || extensions.querySelector('.extension-row[data-current="true"] .extension-row-select')?.dataset.extensionItem !== selectionBeforeDirectionalSearch) throw new Error('Extensions Search accepted directional gamepad navigation');
    extensionSearchControl.dispatchEvent(new KeyboardEvent('keydown', { key:'a', bubbles:true }));
    if (document.activeElement !== extensionSearchControl || document.body.dataset.inputMode !== 'keyboard') throw new Error('keyboard input did not remain in Extensions Search');
    runControllerAction('social-mode');
    if (document.activeElement !== selectedExtensionBeforeSearch || getComputedStyle(selectedExtensionBeforeSearch.closest('.extension-row')).boxShadow === 'none') throw new Error('View did not restore the previously focused extension row and its selection ring');
    runControllerAction('social-mode');
    if (document.activeElement !== extensionSearchControl) throw new Error('View did not re-enter Extensions Search');
    runControllerAction('cancel');
    if (document.activeElement !== selectedExtensionBeforeSearch) throw new Error('B did not restore the previously focused extension row');
    runControllerAction('social-mode');
    runControllerAction('utility-mode');
    if (document.activeElement !== document.querySelector('.utility-cluster button:not(:disabled)')) throw new Error('Utilities did not open from Extensions Search');
    runControllerAction('utility-mode');
    if (document.activeElement !== selectedExtensionBeforeSearch) throw new Error('Utilities did not return to the selected extension row');
    if (getComputedStyle(extensionLibrary).boxShadow === 'none' || getComputedStyle(document.activeElement).boxShadow !== 'none') throw new Error('Extensions lost its card ring or drew the stray child-button divider');
    extensions.querySelector('[data-extension-item="overlays:dats-overlay"]').click();
    datsDetail = extensions.querySelector('#extension-detail');
    const officialRow = datsDetail.querySelector('[data-dat-package="bahamut-dats-overlay"]');
    if (!officialRow || officialRow.dataset.locked !== 'true' || officialRow.dataset.enabled !== 'true' || officialRow.querySelector('input,button') || officialRow.querySelector('.extension-overlay-order-label')?.textContent !== '#1') throw new Error('bundled official overlay is not locked and enabled in slot #1');
    if (datsDetail.querySelector('.extension-overlay-update,[data-official-overlay-update],[data-official-overlay-verify],[data-official-overlay-repair]') || /Official overlay updates|Verify overlay|Repair overlay|Check and install/.test(datsDetail.textContent)) throw new Error('retired official overlay controls remain visible');
    if (state.calls.some(call => /official_overlay/.test(call.command))) throw new Error('Extensions called the removed official overlay updater');
    state.calls.length = 0;
    const detailWorldMoveEarlier = datsDetail.querySelector('[data-dat-move="earlier"][data-dat-package-id="detail-world"]');
    detailWorldMoveEarlier.focus();
    detailWorldMoveEarlier.click();
    await new Promise(resolve => setTimeout(resolve, 30));
    const reorderCall = state.calls.find(call => call.command === 'reorder_dat_package');
    if (!reorderCall || reorderCall.args.id !== 'detail-world' || reorderCall.args.position !== 1 || state.extensionInventory.overlays.map(packageItem => packageItem.id).join(',') !== 'bahamut-dats-overlay,detail-world,base-world') throw new Error('Dats first-hit move earlier did not persist the package order');
    if (document.activeElement?.dataset.datPackageEnabled !== 'detail-world') throw new Error('Dats move did not restore focus to the moved package');
    datsDetail = extensions.querySelector('#extension-detail');
    const detailWorldMoveLater = datsDetail.querySelector('[data-dat-move="later"][data-dat-package-id="detail-world"]');
    detailWorldMoveLater.focus();
    detailWorldMoveLater.click();
    await new Promise(resolve => setTimeout(resolve, 30));
    const laterCall = state.calls.find(call => call.command === 'reorder_dat_package' && call.args.position === 2);
    if (!laterCall || laterCall.args.id !== 'detail-world' || state.extensionInventory.overlays.map(packageItem => packageItem.id).join(',') !== 'bahamut-dats-overlay,base-world,detail-world') throw new Error('Dats first-hit move later did not persist the package order');
    datsDetail = extensions.querySelector('#extension-detail');
    const detailToggle = datsDetail.querySelector('[data-dat-package-enabled="detail-world"]');
    detailToggle.focus();
    detailToggle.checked = false;
    detailToggle.dispatchEvent(new Event('change', { bubbles:true }));
    await new Promise(resolve => setTimeout(resolve, 30));
    const datDisableCall = state.calls.find(call => call.command === 'set_dat_package_enabled');
    if (!datDisableCall || datDisableCall.args.id !== 'detail-world' || datDisableCall.args.enabled !== false || state.extensionInventory.overlays.find(packageItem => packageItem.id === 'detail-world').enabled || datsDetail.querySelector('[data-dat-package-enabled="detail-world"]').checked || datsDetail.querySelector('[data-dat-conflict-summary]')) throw new Error('Dats package enable control did not persist through the backend contract');
    if (document.activeElement?.dataset.datPackageEnabled !== 'detail-world') throw new Error('Dats package toggle did not restore focus after inventory rehydration');
    datsDetail = extensions.querySelector('#extension-detail');
    const fpsToggle = extensions.querySelector('[data-addon-enabled="fps"]');
    const toggleStyle = getComputedStyle(fpsToggle.closest('.extension-toggle'));
    if (fpsToggle.getBoundingClientRect().width !== 24 || toggleStyle.borderRightWidth !== '0px') throw new Error('extension checkbox retained the divided-cell treatment');
    extensions.querySelector('[data-extension-item="addons:fps"]').click();
    const selectedFpsButton = extensions.querySelector('[data-extension-item="addons:fps"]');
    selectedFpsButton.focus();
    await new Promise(resolve => setTimeout(resolve, 100));
    if (!selectedFpsButton.isConnected || document.activeElement !== selectedFpsButton || getComputedStyle(extensionLibrary).boxShadow === 'none') throw new Error('extension selection rebuilt the row or dropped the active card ring');
    const stableFolderDockRect = folderDock.getBoundingClientRect();
    if (Math.abs(stableFolderDockRect.left - folderDockRect.left) >= .1 || Math.abs(stableFolderDockRect.top - folderDockRect.top) >= .1 || Math.abs(stableFolderDockRect.width - folderDockRect.width) >= .1 || Math.abs(stableFolderDockRect.height - folderDockRect.height) >= .1) throw new Error('Extensions folder dock moved when the selected detail changed');
    const fpsDetail = extensions.querySelector('#extension-detail');
    const fpsMeta = [...fpsDetail.querySelectorAll('.extension-meta-item')].map(node => node.textContent).join('|');
    if (fpsDetail.querySelector('h2').textContent !== 'fps' || fpsMeta !== 'Author: Aeshur|Version: v1.0' || [...fpsDetail.querySelectorAll('.extension-command')].map(node => node.textContent).join('|') !== '/fps' || fpsDetail.querySelector('.extension-commands').getAttribute('aria-label') !== 'Commands') throw new Error('Extensions did not limit the addon card to its main command');
    if (fpsDetail.querySelector('.extension-description').textContent !== "Displays the game's current frame rate.") throw new Error('fps description is not using the concise presentation copy');
    if (fpsDetail.querySelector('.extension-order,[data-addon-move]') || /Load order|Move earlier|Move later/.test(fpsDetail.textContent)) throw new Error('Extensions retained the removed addon load-order panel');
    if (/Package|Runtime|Commands|Isolated Lua|Supported client build/.test(fpsDetail.textContent) || fpsDetail.querySelector('.extension-detail-grid,.detail-card')) throw new Error('normal extension detail retained inventory cards');
    document.querySelector('[data-extension-item="addons:wiki"]').click();
    const wikiDetail = document.querySelector('#extension-detail');
    const wikiCommands = [...wikiDetail.querySelectorAll('.extension-command')].map(node => node.textContent).join('|');
    if (wikiDetail.querySelector('h2').textContent !== 'wiki' || wikiCommands !== '/wiki' || wikiDetail.querySelector('.extension-commands').getAttribute('aria-label') !== 'Commands' || wikiDetail.querySelector('.extension-description').textContent !== 'Opens the Bahamut wiki and searches its MediaWiki pages in your default browser.') throw new Error('Wiki addon presentation or command inventory is missing');
    document.querySelector('[data-extension-item="addons:fps"]').click();
    document.querySelector('[data-extension-item="plugins:screenshot"]').click();
    const screenshotDetail = document.querySelector('#extension-detail');
    const screenshotCommands = [...screenshotDetail.querySelectorAll('.extension-command')].map(node => node.textContent).join('|');
    const screenshotMeta = [...screenshotDetail.querySelectorAll('.extension-meta-item')].map(node => node.textContent).join('|');
    if (screenshotDetail.querySelector('h2').textContent !== 'Screenshot' || screenshotMeta !== 'Author: Aeshur|Version: v1.0' || screenshotCommands !== '/screenshot' || screenshotDetail.querySelector('.extension-commands').getAttribute('aria-label') !== 'Commands') throw new Error('Screenshot plugin detail or command is missing');
    if ([...extensions.querySelectorAll('[data-extension-folder]')].map(button => button.dataset.extensionFolder).join(',') !== 'plugins,addons') throw new Error('Extensions folder actions are missing or out of order');
    if (extensions.querySelector('[data-extension-folder="dats"]') || extensions.textContent.includes('Open Overlay Folder')) throw new Error('Extensions retained the removed overlay folder action');
    state.calls.length = 0;
    const extensionSearch = document.querySelector('#extension-search');
    document.querySelector('[data-utility="theme"]').click();
    const searchStyle = getComputedStyle(extensionSearch);
    if (searchStyle.height !== '44px' || searchStyle.paddingLeft !== '14px' || searchStyle.backgroundColor !== 'rgba(244, 252, 255, 0.94)' || searchStyle.color !== 'rgb(24, 54, 69)') throw new Error('Extensions search geometry or semantic day palette drifted');
    const lightAccentActions = [...document.querySelectorAll('.accent-action:not(:disabled)')];
    if (!lightAccentActions.length || lightAccentActions.some(button => !getComputedStyle(button).backgroundImage.includes('linear-gradient') || getComputedStyle(button).color !== 'rgb(5, 48, 66)')) throw new Error('day accent actions bypassed the shared filled-action tier');
    const lightScreenshotRow = extensions.querySelector('[data-extension-item="plugins:screenshot"]').closest('.extension-row');
    const lightFpsRow = extensions.querySelector('[data-extension-item="addons:fps"]').closest('.extension-row');
    const lightScreenshotRowStyle = getComputedStyle(lightScreenshotRow);
    const lightFpsRowStyle = getComputedStyle(lightFpsRow);
    const lightExtensionStyles = {
      text: getComputedStyle(lightScreenshotRow.querySelector('.extension-row-name')).color,
      selectedBackground: lightScreenshotRowStyle.backgroundImage,
      enabledBackground: lightFpsRowStyle.backgroundImage,
      shadow: lightScreenshotRowStyle.boxShadow,
      command: getComputedStyle(screenshotDetail.querySelector('.extension-command')).backgroundColor,
    };
    if (lightExtensionStyles.text !== 'rgb(24, 54, 69)' || !lightExtensionStyles.selectedBackground.includes('rgba(57, 200, 240, 0.82)') || !lightExtensionStyles.enabledBackground.includes('rgba(57, 200, 240, 0.18)') || lightExtensionStyles.selectedBackground === lightExtensionStyles.enabledBackground || !lightExtensionStyles.shadow.includes('rgb(120, 221, 248)') || lightExtensionStyles.command !== 'rgba(57, 200, 240, 0.12)') throw new Error('Extensions day rows, selected fill, selection ring, or command pill bypassed semantic tokens: ' + JSON.stringify(lightExtensionStyles));
    lightScreenshotRow.style.transition = 'none';
    document.body.setAttribute('data-extension-search-focused', '');
    const searchFocusedRowBackground = getComputedStyle(lightScreenshotRow).backgroundImage;
    if (!searchFocusedRowBackground.includes('rgba(57, 200, 240, 0.18)') || searchFocusedRowBackground.includes('rgba(57, 200, 240, 0.82)')) throw new Error('Extensions Search retained the day selected-row fill: ' + searchFocusedRowBackground);
    document.body.removeAttribute('data-extension-search-focused');
    const restoredSelectedRowBackground = getComputedStyle(lightScreenshotRow).backgroundImage;
    if (!restoredSelectedRowBackground.includes('rgba(57, 200, 240, 0.82)')) throw new Error('Extensions row focus did not restore the day selected-row fill: ' + restoredSelectedRowBackground);
    lightScreenshotRow.style.removeProperty('transition');
    document.querySelector('[data-utility="theme"]').click();
    const nightSelectedRowBackground = getComputedStyle(lightScreenshotRow).backgroundImage;
    if (!nightSelectedRowBackground.includes('rgba(197, 42, 50, 0.58)') || !nightSelectedRowBackground.includes('rgba(92, 24, 34, 0.94)')) throw new Error('Extensions selected row lacks its distinct night gradient: ' + nightSelectedRowBackground);
    lightScreenshotRow.style.transition = 'none';
    document.body.setAttribute('data-extension-search-focused', '');
    const nightSearchRowBackground = getComputedStyle(lightScreenshotRow).backgroundImage;
    if (!nightSearchRowBackground.includes('rgba(197, 42, 50, 0.3)') || !nightSearchRowBackground.includes('rgba(62, 18, 26, 0.92)')) throw new Error('Extensions Search retained the night selected-row fill: ' + nightSearchRowBackground);
    document.body.removeAttribute('data-extension-search-focused');
    if (!getComputedStyle(lightScreenshotRow).backgroundImage.includes('rgba(197, 42, 50, 0.58)')) throw new Error('Extensions row focus did not restore the night selected-row fill');
    lightScreenshotRow.style.removeProperty('transition');
    state.calls.length = 0;
    const screenshotEnabled = extensions.querySelector('[data-plugin-enabled="screenshot"]');
    if (extensions.querySelector('[data-plugin-enabled="object-distance"], [data-extension-item="plugins:object-distance"]')) throw new Error('Draw Distance still has an Extensions control');
    screenshotEnabled.focus();
    screenshotEnabled.checked = false;
    screenshotEnabled.dispatchEvent(new Event('change', { bubbles:true }));
    await new Promise(resolve => setTimeout(resolve, 30));
    const screenshotDisableCall = state.calls.find(call => call.command === 'set_screenshot_enabled');
    if (!screenshotDisableCall || screenshotDisableCall.args.enabled !== false || state.extensionInventory.plugins[0].enabled || extensions.querySelector('[data-plugin-enabled="screenshot"]').checked) throw new Error('Screenshot enable control did not persist through the backend contract');
    if (document.activeElement !== extensions.querySelector('[data-plugin-enabled="screenshot"]')) throw new Error('Screenshot toggle did not restore focus after inventory rehydration');
    state.calls.length = 0;
    extensionSearch.value = 'missing';
    extensionSearch.dispatchEvent(new Event('input', { bubbles:true }));
    if (extensions.querySelectorAll('.extension-row').length !== 0 || !extensions.textContent.includes('No matches in this group.')) throw new Error('Extensions search did not filter the installed addon');
    extensionSearch.value = '/fps';
    extensionSearch.dispatchEvent(new Event('input', { bubbles:true }));
    const filteredFpsButton = extensions.querySelector('[data-extension-item="addons:fps"]');
    document.body.dataset.inputMode = 'gamepad';
    filteredFpsButton.focus();
    if (getComputedStyle(filteredFpsButton).boxShadow !== 'none' || getComputedStyle(filteredFpsButton.closest('.extension-row')).transitionDuration !== '0s' || getComputedStyle(extensions.querySelector('.extension-library')).boxShadow === 'none') throw new Error('gamepad Extensions focus lost the card ring, retained a child ring, or animated row transition');
    const filteredFpsIdentity = filteredFpsButton;
    filteredFpsButton.click();
    if (extensions.querySelectorAll('.extension-row').length !== 1 || state.calls.length) throw new Error('Extensions search or selection mutated backend state');
    if (document.activeElement !== filteredFpsIdentity || !filteredFpsIdentity.isConnected) throw new Error('Extensions selection rebuilt or dropped the focused row');
    const filteredAddonToggle = extensions.querySelector('[data-addon-enabled="fps"]');
    filteredAddonToggle.focus();
    filteredAddonToggle.checked = false;
    filteredAddonToggle.dispatchEvent(new Event('change', { bubbles:true }));
    await new Promise(resolve => setTimeout(resolve, 30));
    const disableCall = state.calls.find(call => call.command === 'set_addon_enabled');
    if (!disableCall || disableCall.args.id !== 'fps' || disableCall.args.enabled !== false || state.extensionInventory.addons[0].enabled) throw new Error('addon enable control did not persist through the backend contract');
    if (!extensions.querySelector('[data-addon-enabled="fps"]') || extensions.querySelector('[data-addon-enabled="fps"]').checked) throw new Error('addon write did not rehydrate the inventory');
    if (document.activeElement?.dataset.addonEnabled !== 'fps') throw new Error('addon toggle did not restore focus after inventory rehydration');
    const failedToggle = extensions.querySelector('[data-addon-enabled="fps"]');
    const inventoryReadsBeforeFailure = state.calls.filter(call => call.command === 'get_extension_inventory').length;
    state.failAddonWrite = true;
    failedToggle.focus();
    failedToggle.checked = true;
    failedToggle.dispatchEvent(new Event('change', { bubbles:true }));
    await new Promise(resolve => setTimeout(resolve, 30));
    const rehydratedToggle = extensions.querySelector('[data-addon-enabled="fps"]');
    const inventoryReadsAfterFailure = state.calls.filter(call => call.command === 'get_extension_inventory').length;
    const extensionActionAlert = extensions.querySelector('#extension-detail .extension-action-error[role="alert"]');
    if (document.activeElement?.dataset.addonEnabled !== 'fps' || state.extensionInventory.addons[0].enabled || rehydratedToggle.checked || inventoryReadsAfterFailure !== inventoryReadsBeforeFailure + 1 || !extensionActionAlert?.textContent.includes('fixture addon write failure')) throw new Error('failed addon write did not restore focus, authoritative rehydrated state, and visible feedback');
    document.body.dataset.inputMode = 'pointer';
    state.calls.length = 0;
    extensions.querySelector('[data-extension-folder="plugins"]').click();
    await new Promise(resolve => setTimeout(resolve, 20));
    const folderCall = state.calls.find(call => call.command === 'open_extension_folder');
    if (!folderCall || folderCall.args.target !== 'plugins') throw new Error('Extensions plugin folder action did not use the keyed backend command');
    state.calls.length = 0;
    extensions.querySelector('[data-extension-folder="addons"]').click();
    await new Promise(resolve => setTimeout(resolve, 20));
    const addonsFolderCall = state.calls.find(call => call.command === 'open_extension_folder');
    if (!addonsFolderCall || addonsFolderCall.args.target !== 'addons') throw new Error('Extensions addon folder action did not use the keyed backend command');

    if (getComputedStyle(document.body).userSelect !== 'none' || getComputedStyle(datsDetail.querySelector('h2')).userSelect !== 'none') throw new Error('launcher shell text can still be selected');
    if (getComputedStyle(extensionSearch).userSelect !== 'text' || getComputedStyle(document.querySelector('#help-log')).userSelect !== 'text') throw new Error('editable or diagnostic text lost selection support');

  `);

  await assertUi(`
    const state = window.__launcherBrowserState;
    state.calls.length = 0;
    document.querySelector('[data-utility="gamepad"]').click();
    await new Promise(resolve => setTimeout(resolve, 50));
    const screen = document.querySelector('#screen-gamepad');
    if (!screen.hasAttribute('data-active') || document.querySelector('[data-route][aria-selected="true"]')) throw new Error('Gamepad utility did not open its dedicated screen');
    if (screen.querySelectorAll('.gamepad-card').length !== 2 || screen.querySelectorAll('[data-gamepad-region]').length !== 1 || screen.querySelector('h1').textContent !== 'Gamepad Settings') throw new Error('Gamepad screen does not have the functional and placeholder cards');
    const gamepadScreenRect = screen.getBoundingClientRect();
    const gamepadGridRect = screen.querySelector('.gamepad-grid').getBoundingClientRect();
    const gamepadCardRects = [...screen.querySelectorAll('.gamepad-card')].map(card => card.getBoundingClientRect());
    if (gamepadGridRect.height >= gamepadScreenRect.height - 100 || Math.abs(gamepadCardRects[0].height - gamepadCardRects[1].height) >= .1 || Math.abs(gamepadGridRect.width - 1070) >= .1) throw new Error('Gamepad cards do not match the Avalon grid geometry');
    if (document.querySelector('[data-settings-tab="gamepad"]') || document.querySelector('[data-settings-pane="gamepad"]')) throw new Error('Gamepad preference still has a duplicate Settings owner');
    const cardHeadingStyle = getComputedStyle(screen.querySelector('.gamepad-card h1'));
    const settingStyle = getComputedStyle(screen.querySelector('.gamepad-setting'));
    const choiceRowStyle = getComputedStyle(screen.querySelector('.gamepad-card .choice-row'));
    const stubCardStyle = getComputedStyle(screen.querySelector('.gamepad-card--stub'));
    const stubSettingStyle = getComputedStyle(screen.querySelector('.gamepad-card--stub .gamepad-setting'));
    const functionalSettingRect = screen.querySelector('.gamepad-card:not(.gamepad-card--stub) .gamepad-setting').getBoundingClientRect();
    const stubStatusRect = screen.querySelector('.gamepad-card--stub .gamepad-status').getBoundingClientRect();
    const stubSettingRects = [...screen.querySelectorAll('.gamepad-card--stub .gamepad-setting')].map(row => row.getBoundingClientRect());
    const configRects = [...screen.querySelectorAll('.gamepad-config-action')].map(button => button.getBoundingClientRect());
    const separators = [...screen.querySelectorAll('.gamepad-setting')].filter(row => getComputedStyle(row).borderTopWidth !== '0px');
    if (screen.textContent.includes('Enable Gamepad') || !screen.textContent.includes('Steam Deck not detected.') || screen.textContent.includes('support is unavailable') || screen.querySelectorAll('.gamepad-card--stub button:not(:disabled)').length) throw new Error('Gamepad or Steam Deck support state is misleading');
    if (cardHeadingStyle.fontSize !== '22px' || cardHeadingStyle.lineHeight !== '33px' || settingStyle.minHeight !== '50px' || choiceRowStyle.gap !== '8px' || stubCardStyle.opacity !== '1' || stubSettingStyle.opacity !== '0.5' || separators.length) throw new Error('Gamepad card geometry drifted from Avalon: ' + JSON.stringify({ heading:cardHeadingStyle.fontSize, headingLine:cardHeadingStyle.lineHeight, settingHeight:settingStyle.minHeight, choiceGap:choiceRowStyle.gap, stubCardOpacity:stubCardStyle.opacity, stubSettingOpacity:stubSettingStyle.opacity, separators:separators.length }));
    const configCenters = configRects.map((rect, index) => Math.abs(rect.left + rect.width / 2 - (gamepadCardRects[index].left + gamepadCardRects[index].width / 2)));
    if (Math.abs(functionalSettingRect.top - stubSettingRects[0].top) >= .1 || Math.abs(stubSettingRects[1].top - stubSettingRects[0].top - 92) >= .1 || Math.abs(stubSettingRects[0].top - stubStatusRect.bottom - 22) >= .1 || Math.abs(configRects[0].top - configRects[1].top) >= .1 || Math.abs(configRects[1].top - stubSettingRects[1].bottom - 42) >= .1 || configCenters.some(offset => offset >= .1)) throw new Error('Gamepad rows or actions are not aligned to the Avalon rhythm');
    const configButton = document.querySelector('#gamepad-config-tool-button');
    if (!configButton || configButton.hidden || configButton.textContent !== 'XIV Config' || Math.abs(configButton.getBoundingClientRect().width - 280) >= .1 || Math.abs(configButton.getBoundingClientRect().height - 50) >= .1) throw new Error('Gamepad XIV Config action is missing or not centered to the Avalon size');
    configButton.click();
    await new Promise(resolve => setTimeout(resolve, 20));
    if (!state.calls.some(call => call.command === 'launch_config_tool')) throw new Error('Gamepad XIV Config action did not invoke the retail utility');
    if (screen.querySelector('.settings-description') || screen.textContent.includes('current controller remains active')) throw new Error('Gamepad page retained the removed ownership copy');
  `);

  await assertUi(`
    const makePad = (index = 0, id = 'Xbox Controller') => ({
      index, id, axes:[0, 0],
      buttons:Array.from({ length:16 }, () => ({ pressed:false, value:0 })),
    });
    updateControllerLabel([makePad()]);
    if (document.querySelector('#active-controller').textContent !== 'Gamepad detected: Xbox / XInput') throw new Error('detected gamepad copy drifted from Avalon');
    const mapped = [[0,'confirm'],[1,'cancel'],[2,'secondary'],[3,'cycle-settings'],[4,'previous-region'],[5,'next-region'],[6,'previous-route'],[7,'next-route'],[8,'social-mode'],[9,'utility-mode'],[12,'up'],[13,'down'],[14,'left'],[15,'right']];
    for (const [button, expected] of mapped) {
      controllerStates.clear();
      const pad = makePad(button);
      pad.buttons[button] = { pressed:true, value:1 };
      const actions = readControllerActions(pad, undefined, 0);
      if (actions.length !== 1 || actions[0] !== expected) throw new Error('button ' + button + ' mapped to ' + actions.join(','));
    }
    controllerStates.clear();
    const held = makePad(20);
    held.buttons[15] = { pressed:true, value:1 };
    if (readControllerActions(held, undefined, 0).join(',') !== 'right') throw new Error('held direction did not fire immediately');
    if (readControllerActions(held, controllerStates.get(20), 319).length) throw new Error('held control repeated before 320ms');
    if (readControllerActions(held, controllerStates.get(20), 320).join(',') !== 'right') throw new Error('held direction did not repeat at 320ms');
    if (readControllerActions(held, controllerStates.get(20), 439).length) throw new Error('held control repeated before the 120ms interval');
    if (readControllerActions(held, controllerStates.get(20), 440).join(',') !== 'right') throw new Error('held direction did not repeat at 120ms');
    controllerStates.clear();
    const heldConfirm = makePad(23);
    heldConfirm.buttons[0] = { pressed:true, value:1 };
    if (readControllerActions(heldConfirm, undefined, 0).join(',') !== 'confirm') throw new Error('confirm did not fire immediately');
    if (readControllerActions(heldConfirm, controllerStates.get(23), 320).length) throw new Error('held confirm repeated');
    const stick = makePad(21);
    stick.axes[0] = .56;
    if (readControllerActions(stick, undefined, 0).join(',') !== 'right') throw new Error('stick did not engage at 0.55');
    stick.axes[0] = .4;
    readControllerActions(stick, controllerStates.get(21), 1);
    if (controllerStates.get(21).horizontal !== 1) throw new Error('stick did not remain latched above 0.35');
    stick.axes[0] = .34;
    readControllerActions(stick, controllerStates.get(21), 2);
    if (controllerStates.get(21).horizontal !== 0) throw new Error('stick did not release at 0.35');
    const priority = makePad(22);
    priority.axes[0] = .9;
    priority.buttons[14] = { pressed:true, value:1 };
    if (readControllerActions(priority, undefined, 0).join(',') !== 'left') throw new Error('D-pad did not take priority over the stick');
    priority.buttons[14] = { pressed:false, value:0 };
    if (readControllerActions(priority, controllerStates.get(22), 1).length) throw new Error('suppressed stick direction fired when D-pad was released');
    const sourceSwitch = makePad(24);
    sourceSwitch.axes[1] = .9;
    if (readControllerActions(sourceSwitch, undefined, 0).join(',') !== 'down') throw new Error('stick source did not fire immediately');
    sourceSwitch.buttons[13] = { pressed:true, value:1 };
    if (readControllerActions(sourceSwitch, controllerStates.get(24), 1).join(',') !== 'down') throw new Error('D-pad source handoff swallowed the first press');
    const mainSource = await fetch('/js/main.js').then(response => response.text());
    const gamepadConstants = [
      ['GAMEPAD_ACTIVE_POLL_MS', '90'],
      ['GAMEPAD_IDLE_POLL_MS', '250'],
      ['GAMEPAD_INITIAL_REPEAT_MS', '320'],
      ['GAMEPAD_REPEAT_MS', '120'],
      ['GAMEPAD_AXIS_PRESS', '.55'],
      ['GAMEPAD_AXIS_RELEASE', '.35'],
    ];
    if (!gamepadConstants.every(([name, value]) => mainSource.includes('const ' + name + ' = ' + value + ';'))) throw new Error('gamepad timing or hysteresis constants drifted');
    const first = makePad(0, 'Xbox');
    const retained = makePad(4, 'DualSense');
    if (selectActiveGamepad([first,retained], 4) !== retained || selectActiveGamepad([first], 4) !== first) throw new Error('active-controller retention or disconnect fallback drifted');
  `);

  await assertUi(`
    const fakePad = { index:0, id:'DualSense Wireless Controller', axes:[0,0], buttons:Array.from({ length:16 }, () => ({ pressed:false, value:0 })) };
    const state = window.__launcherBrowserState;
    state.gamepads = [fakePad];
    document.querySelector('[data-route="home"]').click();
    updateGamepadPrompts([fakePad]);
    const prompts = document.querySelector('#gamepad-prompts');
    const visibleLabels = () => [...prompts.querySelectorAll('.gamepad-prompt-bar__item:not([hidden]) .gamepad-prompt-bar__label')].map(label => label.textContent).join(',');
    if (prompts.hidden || visibleLabels() !== 'Confirm,Toggle,Tabs,Back,Focus,Menu,Util,Social') throw new Error('Home prompt labels drifted: ' + visibleLabels());
    const confirmGlyph = document.querySelector('[data-gamepad-face="confirm"]');
    if (!confirmGlyph.src.endsWith('/assets/gamepad/playstation/cross.png')) throw new Error('PlayStation face asset was not rendered');
    if (document.querySelector('[data-gamepad-badge="previous-region"]').textContent !== 'L1' || document.querySelector('[data-gamepad-badge="utility"]').textContent !== 'Options') throw new Error('PlayStation prompt badges drifted');
    await new Promise(resolve => setTimeout(resolve, 200));
    if (!confirmGlyph.complete || confirmGlyph.naturalWidth !== 480) throw new Error('PlayStation prompt asset did not decode');
    const barStyle = getComputedStyle(prompts);
    const badgeStyle = getComputedStyle(document.querySelector('[data-gamepad-badge="previous-region"]'));
    if (barStyle.left !== '0px' || barStyle.bottom !== '0px' || barStyle.backgroundColor !== 'rgba(255, 252, 240, 0.96)' || barStyle.borderTopColor !== 'rgba(151, 121, 58, 0.2)' || getComputedStyle(confirmGlyph).width !== '20px' || badgeStyle.color !== 'rgb(245, 241, 252)') throw new Error('prompt bar geometry or fixed cream surface drifted');
    const shell = document.querySelector('.launcher-shell');
    const stagePadding = getComputedStyle(document.querySelector('.stage')).paddingBottom;
    if (!shell.hasAttribute('data-gamepad-bar-visible') || stagePadding !== '40px') throw new Error('Home workspace did not reserve Avalon prompt-bar space: visible=' + shell.hasAttribute('data-gamepad-bar-visible') + ', screen=' + shell.dataset.gamepadScreen + ', padding=' + stagePadding);

    fakePad.id = 'Xbox Controller';
    updateGamepadPrompts([fakePad]);
    if (!confirmGlyph.src.endsWith('/assets/gamepad/xbox/a.png') || document.querySelector('[data-gamepad-badge="previous-region"]').textContent !== 'LB' || document.querySelector('[data-gamepad-badge="utility"]').textContent.codePointAt(0) !== 0x2630) throw new Error('Xbox prompt assets or badges drifted');
    localStorage.setItem('bahamut-gamepad-enabled', 'false');
    updateGamepadPrompts([fakePad]);
    if (!prompts.hidden || document.querySelector('.launcher-shell').hasAttribute('data-gamepad-bar-visible')) throw new Error('disabled launcher navigation left gamepad prompts visible');
    localStorage.setItem('bahamut-gamepad-enabled', 'true');
    updateGamepadPrompts([fakePad]);
    const pointerX = window.__launcherModules.main.lastMouseX ?? 10;
    const pointerY = window.__launcherModules.main.lastMouseY ?? 10;
    trackMouseMovement({ screenX:pointerX, screenY:pointerY });
    if (prompts.hidden) throw new Error('first mouse movement did not act as the synthetic-move seed');
    trackMouseMovement({ screenX:pointerX + 20, screenY:pointerY + 12 });
    if (!prompts.hidden || document.body.dataset.inputMode !== 'pointer') throw new Error('real mouse movement did not hide the prompt bar');
    runControllerAction('next-region');
    updateGamepadPrompts([fakePad]);
    if (prompts.hidden || document.body.dataset.inputMode !== 'gamepad') throw new Error('gamepad action did not restore the prompt bar');
    const homeRegions = [...document.querySelectorAll('#screen-home [data-gamepad-region]')]
      .sort((left, right) => Number(left.dataset.gamepadRegionOrder) - Number(right.dataset.gamepadRegionOrder));
    if (homeRegions.length !== 4 || document.activeElement !== homeRegions[0]) throw new Error('Home did not expose and focus its four Avalon-style regions');
    runControllerAction('down');
    if (document.activeElement !== document.querySelector('#login-username')) throw new Error('D-pad did not enter the focused Home card');
    runControllerAction('next-region');
    if (document.activeElement !== homeRegions[1]) throw new Error('RB skipped the Recent News card-selection stage');
    const homePrimary = document.querySelector('#home-primary');
    if (!homePrimary.disabled) throw new Error('logged-out Home primary action was unexpectedly enabled');
    runControllerAction('next-region');
    if (document.activeElement !== homeRegions[0]) throw new Error('disabled Home primary action trapped RB region focus');
    homePrimary.disabled = false;
    homeRegions[1].focus();
    runControllerAction('next-region');
    if (document.activeElement !== homePrimary) throw new Error('enabled Home primary action was skipped by RB region focus');
    homePrimary.disabled = true;

    document.querySelector('[data-route="home"]').click();
    const loginSubmit = document.querySelector('#login-submit');
    const createAccount = document.querySelector('[data-action="open-register"]');
    const profiles = document.querySelector('[data-action="open-profiles"]');
    for (const control of ['#login-username', '#login-password', '#login-remember', '#login-submit'].map(selector => document.querySelector(selector))) {
      control.focus();
      runControllerAction('left');
      if (document.activeElement !== control) throw new Error('Login left escaped a form control');
      runControllerAction('right');
      if (document.activeElement !== control) throw new Error('Login right escaped a form control');
    }
    loginSubmit.focus();
    runControllerAction('down');
    if (document.activeElement !== createAccount) throw new Error('Login down did not stop on Create Account');
    runControllerAction('down');
    if (document.activeElement !== createAccount) throw new Error('Login down escaped Create Account');
    runControllerAction('right');
    if (document.activeElement !== profiles) throw new Error('Create Account right did not reach Profiles');
    runControllerAction('left');
    if (document.activeElement !== createAccount) throw new Error('Profiles left did not return to Create Account');
    const previous = document.querySelector('#login-username');
    previous.focus();
    runControllerAction('utility-mode');
    if (document.activeElement !== document.querySelector('[data-utility="gamepad"]')) throw new Error('Start did not enter utility controls');
    runControllerAction('next-route');
    if (document.activeElement !== document.querySelector('[data-utility="gamepad"]') || document.querySelector('[data-route="home"]').getAttribute('aria-selected') !== 'true') throw new Error('route action escaped utility mode');
    runControllerAction('right');
    if (document.activeElement !== document.querySelector('[data-utility="help"]')) throw new Error('utility D-pad movement drifted');
    runControllerAction('down');
    if (document.activeElement !== document.querySelector('[data-utility="help"]')) throw new Error('utility controls accepted vertical D-pad movement');
    runControllerAction('up');
    if (document.activeElement !== document.querySelector('[data-utility="help"]')) throw new Error('utility controls accepted reverse vertical D-pad movement');
    runControllerAction('cancel');
    if (document.activeElement !== previous) throw new Error('B did not restore focus from utility controls');
    runControllerAction('social-mode');
    if (document.activeElement !== document.querySelector('[data-link="discord"]')) throw new Error('View did not enter Home social controls');
    runControllerAction('next-route');
    if (document.activeElement !== document.querySelector('[data-link="discord"]') || document.querySelector('[data-route="home"]').getAttribute('aria-selected') !== 'true') throw new Error('route action escaped social mode');
    runControllerAction('down');
    if (document.activeElement !== document.querySelector('[data-link="discord"]')) throw new Error('social controls accepted vertical D-pad movement');
    runControllerAction('up');
    if (document.activeElement !== document.querySelector('[data-link="discord"]')) throw new Error('social controls accepted reverse vertical D-pad movement');
    runControllerAction('right');
    if (document.activeElement !== document.querySelector('[data-link="youtube"]')) throw new Error('social D-pad order drifted');
    runControllerAction('right');
    if (document.activeElement !== document.querySelector('[data-link="youtube"]')) throw new Error('social D-pad wrapped instead of stopping at the edge');
    runControllerAction('social-mode');
    if (document.activeElement !== previous) throw new Error('View did not restore focus from Home social controls');

    document.querySelector('[data-route="extensions"]').click();
    await new Promise(resolve => setTimeout(resolve, 80));
    updateGamepadPrompts([fakePad]);
    if (visibleLabels() !== 'Confirm,Toggle,Tabs,Back,Focus,Menu,Util,Search') throw new Error('Extensions prompt did not expose Search on View/Share: ' + visibleLabels());
    document.querySelector('[data-route="home"]').click();
    document.querySelector('[data-route="settings"]').click();
    updateGamepadPrompts([fakePad]);
    if (visibleLabels() !== 'Confirm,Toggle,Tabs,Back,Focus,Menu,Util' || getComputedStyle(document.querySelector('.stage')).paddingBottom !== '36px') throw new Error('non-Home prompt contract drifted');
    activateSettingsTab('general');
    focusAdjacentRegion(1);
    const generalCards = [...document.querySelectorAll('[data-settings-pane="general"] .settings-card')];
    const closeRow = document.querySelector('[data-launcher-setting-choice="close_on_game_start"]').closest('[data-gamepad-settings-row]');
    if (document.activeElement !== generalCards[0] || getComputedStyle(generalCards[0]).boxShadow === 'none') throw new Error('General region entry did not select the whole card');
    runControllerAction('down');
    const generalEntryFocus = { active:document.activeElement === closeRow, row:getComputedStyle(closeRow).boxShadow, card:getComputedStyle(generalCards[0]).boxShadow };
    if (!generalEntryFocus.active || generalEntryFocus.row === 'none' || generalEntryFocus.card.includes('rgb(240, 154, 145)')) throw new Error('Settings entry did not focus the first row without a second card ring: ' + JSON.stringify(generalEntryFocus));
    runControllerAction('right');
    if (document.activeElement !== closeRow) throw new Error('D-pad entered a choice button instead of retaining row focus');
    runControllerAction('confirm');
    if (document.activeElement !== closeRow) throw new Error('Confirm entered a choice button that should be controlled by X');
    state.calls.length = 0;
    runControllerAction('secondary');
    await new Promise(resolve => setTimeout(resolve, 40));
    if (document.activeElement !== closeRow || !state.calls.some(call => call.command === 'set_close_on_game_start')) throw new Error('X did not cycle the focused row and restore row focus');
    runControllerAction('down');
    if (!document.activeElement.textContent.includes('Hardware Mouse')) throw new Error('D-pad did not advance within the focused Settings card');
    runControllerAction('next-region');
    const displayRow = document.querySelector('[data-game-setting-choice="display_mode"]').closest('[data-gamepad-settings-row]');
    if (document.activeElement !== generalCards[1]) throw new Error('RB did not select the next General card');
    runControllerAction('previous-region');
    if (document.activeElement !== generalCards[0]) throw new Error('LB did not select the previous General card');
    runControllerAction('next-region');
    runControllerAction('confirm');
    if (document.activeElement !== displayRow) throw new Error('Confirm did not enter the selected General card at its first row');
    const previousMode = state.gameSettings.settings.display_mode;
    runControllerAction('secondary');
    await new Promise(resolve => setTimeout(resolve, 40));
    if (document.activeElement !== displayRow || state.gameSettings.settings.display_mode === previousMode) throw new Error('X did not cycle the selected game setting while retaining row focus');
    const resolutionRow = document.querySelector('#game-resolution').closest('[data-gamepad-settings-row]');
    const nativeResolutionRow = resolutionRow.nextElementSibling;
    if (!nativeResolutionRow || nativeResolutionRow.querySelector('.settings-label')?.textContent !== 'Native Resolution Override' || nativeResolutionRow.querySelectorAll('button').length !== 2) throw new Error('Native Resolution Override row did not sit directly below Window Resolution');
    if (nativeResolutionRow.nextElementSibling || document.querySelector('[data-settings-pane="general"]').textContent.includes('HUD Layouts')) throw new Error('removed HUD Layouts row is still visible');
    activateSettingsTab('graphics');
    focusAdjacentRegion(1);
    const graphicsCards = [...document.querySelectorAll('[data-settings-pane="graphics"] .settings-card')];
    if (document.activeElement !== graphicsCards[0] || getComputedStyle(graphicsCards[0]).boxShadow === 'none') throw new Error('Graphics region entry did not select the whole card');
    runControllerAction('next-region');
    if (document.activeElement !== graphicsCards[1]) throw new Error('RB did not select the next Graphics card');
    runControllerAction('confirm');
    if (document.activeElement !== graphicsCards[1].querySelector('[data-gamepad-settings-row]')) throw new Error('Confirm did not enter the selected Graphics card at its first row');
    runControllerAction('previous-region');
    if (document.activeElement !== graphicsCards[0]) throw new Error('LB did not select the previous Graphics card from its row');
    runControllerAction('down');
    if (document.activeElement !== graphicsCards[0].querySelector('[data-gamepad-settings-row]')) throw new Error('D-pad skipped the first Graphics row');
    runControllerAction('down');
    const qualityRange = document.querySelector('#game-general-quality');
    const qualityRow = qualityRange.closest('[data-gamepad-settings-row]');
    if (document.activeElement !== qualityRow) throw new Error('D-pad did not reach the General Drawing Quality row');
    qualityRange.value = qualityRange.max;
    runControllerAction('right');
    if (qualityRange.value !== qualityRange.max || document.activeElement !== qualityRow) throw new Error('right wrapped a settings slider past its maximum');
    runControllerAction('left');
    if (Number(qualityRange.value) !== Number(qualityRange.max) - Number(qualityRange.step)) throw new Error('left did not step a settings slider down from its maximum');
    await new Promise(resolve => setTimeout(resolve, 40));
    runControllerAction('down');
    runControllerAction('down');
    const distanceRange = document.querySelector('#object-distance-range');
    if (document.activeElement !== distanceRange.closest('[data-gamepad-settings-row]')) throw new Error('D-pad did not reach the Draw Distance bar');
    state.calls.length = 0;
    runControllerAction('right');
    await new Promise(resolve => setTimeout(resolve, 40));
    if (distanceRange.value !== '1' || !state.calls.some(call => call.command === 'set_object_distance_selection' && call.args.percent === 125)) throw new Error('D-pad did not enable the first Draw Distance value');
    activateSettingsTab('misc');
    focusAdjacentRegion(1);
    const miscLeftCard = document.querySelector('[data-settings-pane="misc"] [data-gamepad-region-order="1"]');
    const miscRightCard = document.querySelector('[data-settings-pane="misc"] [data-gamepad-region-order="2"]');
    const miscLeftRows = [...miscLeftCard.querySelectorAll('[data-gamepad-settings-row]')];
    if (document.activeElement !== miscLeftCard) throw new Error('LB/RB region entry did not select the Misc card');
    runControllerAction('down');
    if (document.activeElement !== miscLeftRows[0]) throw new Error('D-pad did not select the first whole Misc row');
    const miscFocusStyles = {
      padding: getComputedStyle(miscLeftRows[0]).paddingLeft,
      rowShadow: getComputedStyle(miscLeftRows[0]).boxShadow,
      cardShadow: getComputedStyle(miscLeftCard).boxShadow,
    };
    if (miscFocusStyles.padding !== '12px' || miscFocusStyles.rowShadow === 'none' || miscFocusStyles.cardShadow.includes('rgb(240, 154, 145)')) throw new Error('Misc row focus lacks its gutter or whole-row ring contract: ' + JSON.stringify(miscFocusStyles));
    let pathActivations = 0;
    miscLeftRows[0].querySelector('.settings-action').addEventListener('click', () => { pathActivations += 1; }, { once:true });
    runControllerAction('confirm');
    if (pathActivations !== 1 || document.activeElement !== miscLeftRows[0]) throw new Error('single-action Misc row did not activate on the first Confirm');
    for (const row of miscLeftRows.slice(1)) {
      runControllerAction('down');
      if (document.activeElement !== row) throw new Error('D-pad skipped a left Misc row');
    }
    runControllerAction('down');
    if (document.activeElement !== miscLeftRows.at(-1)) throw new Error('D-pad escaped the bottom of the left Misc card');
    runControllerAction('right');
    runControllerAction('left');
    runControllerAction('confirm');
    if (!document.querySelector('#settings-confirmation-dialog').open || document.activeElement !== document.querySelector('#settings-confirmation-cancel')) throw new Error('Misc Repair Install row did not open a safe confirmation');
    document.querySelector('#settings-confirmation-cancel').click();
    if (document.querySelector('#settings-confirmation-dialog').open) throw new Error('Misc Repair Install confirmation did not close');
    runControllerAction('next-region');
    const miscRightRows = [...miscRightCard.querySelectorAll('[data-gamepad-settings-row]')];
    if (document.activeElement !== miscRightCard) throw new Error('RB did not select the next Misc card');
    runControllerAction('down');
    if (document.activeElement !== miscRightRows[0]) throw new Error('D-pad did not select the first right Misc row');
    if (miscLeftRows.map(row => row.querySelector('.settings-action')?.textContent || row.querySelector('.settings-label')?.textContent).join(',') !== 'Path,Update Launcher,Repair Install') throw new Error('Misc install actions did not expose the requested gamepad order');
    if (miscRightRows.map(row => [...row.querySelectorAll('.settings-action')].map(button => button.textContent).join('/')).join(',') !== 'Backup/Restore,Backup/Restore,Open Backup Folder/Open Install Folder,Open Screenshots Folder') throw new Error('Right Misc rows did not expose the requested gamepad order');
    for (const row of miscRightRows.slice(1)) {
      runControllerAction('down');
      if (document.activeElement !== row) throw new Error('D-pad skipped a right Misc row');
    }
    runControllerAction('down');
    if (document.activeElement !== miscRightRows.at(-1)) throw new Error('D-pad escaped the bottom of the right Misc card');
    let screenshotActivations = 0;
    miscRightRows.at(-1).querySelector('.settings-action').addEventListener('click', () => { screenshotActivations += 1; }, { once:true });
    runControllerAction('confirm');
    if (screenshotActivations !== 1 || document.activeElement !== miscRightRows.at(-1)) throw new Error('single-button folder row did not activate on the first Confirm');
    miscRightRows[1].focus();
    runControllerAction('right');
    const extensionBackup = miscRightRows[1].querySelector('[data-backup-action="create"]');
    const extensionRestore = miscRightRows[1].querySelector('[data-backup-action="restore"]');
    if (document.activeElement !== extensionRestore) throw new Error('D-pad right did not enter the multi-action Misc row at Restore');
    if (getComputedStyle(miscRightRows[1]).boxShadow === 'none') throw new Error('entered Misc row did not preserve its whole-row focus context');
    runControllerAction('left');
    if (document.activeElement !== extensionBackup) throw new Error('D-pad left did not move from Restore to Backup');
    miscRightRows[1].focus();
    runControllerAction('left');
    if (document.activeElement !== extensionBackup) throw new Error('D-pad left did not enter the multi-action Misc row at Backup');
    runControllerAction('right');
    if (document.activeElement !== extensionRestore) throw new Error('D-pad right did not move from Backup to Restore');
    runControllerAction('confirm');
    if (!document.querySelector('#settings-confirmation-dialog').open || document.activeElement !== document.querySelector('#settings-confirmation-cancel')) throw new Error('Gamepad Restore did not open confirmation on safe Cancel');
    runControllerAction('left');
    if (document.activeElement !== document.querySelector('#settings-confirmation-confirm')) throw new Error('D-pad left did not reach the restore confirmation');
    runControllerAction('right');
    if (document.activeElement !== document.querySelector('#settings-confirmation-cancel')) throw new Error('D-pad right did not return to safe Cancel');
    runControllerAction('cancel');
    if (document.querySelector('#settings-confirmation-dialog').open || document.activeElement !== extensionRestore) throw new Error('B did not cancel restore confirmation and return focus');
    activateSettingsTab('general');
    runControllerAction('cycle-settings');
    if (document.querySelector('[data-settings-tab="graphics"]').getAttribute('aria-selected') !== 'true') throw new Error('Y did not cycle flat Settings tabs');
    runControllerAction('next-route');
    if (document.querySelector('[data-route="home"]').getAttribute('aria-selected') !== 'true' || document.activeElement !== document.querySelector('#session-card')) throw new Error('RT did not wrap routes and plant focus on the first Home card');

    document.querySelector('[data-utility="gamepad"]').click();
    const gamepadScreen = document.querySelector('#screen-gamepad');
    if (gamepadScreen.querySelectorAll('.gamepad-card').length !== 2 || getComputedStyle(gamepadScreen.querySelector('.gamepad-grid')).gridTemplateColumns.split(' ').length !== 2) throw new Error('Gamepad page is not the two-card layout');
    if (!gamepadScreen.textContent.includes('Steam Deck not detected.') || gamepadScreen.textContent.includes('support is unavailable') || gamepadScreen.querySelectorAll('.gamepad-card--stub button:not(:disabled)').length) throw new Error('Steam Deck placeholder is missing or implies working controls');
    if (gamepadScreen.textContent.includes('current controller remains active') || gamepadScreen.textContent.includes('Applies to the launcher only')) throw new Error('Gamepad page retained the removed explanatory fluff');
    focusAdjacentRegion(1);
    const gamepadCard = gamepadScreen.querySelector('[data-gamepad-region]');
    if (document.activeElement !== gamepadCard) throw new Error('Gamepad page did not begin at the full card');
    runControllerAction('down');
    const navigationRow = document.querySelector('[data-gamepad-choice="true"]').closest('[data-gamepad-settings-row]');
    if (document.activeElement !== navigationRow) throw new Error('D-pad did not enter the Launcher Gamepad Navigation row');
    runControllerAction('secondary');
    if (document.querySelector('[data-gamepad-choice="false"]').getAttribute('aria-pressed') !== 'true' || localStorage.getItem('bahamut-gamepad-enabled') !== 'false') throw new Error('X did not persist the cycled launcher-navigation choice');
    const on = document.querySelector('[data-gamepad-choice="true"]');
    on.click();
    if (localStorage.getItem('bahamut-gamepad-enabled') !== 'true') throw new Error('launcher-navigation choice did not persist when restored');
    runControllerAction('cancel');
    if (document.querySelector('[data-route="home"]').getAttribute('aria-selected') !== 'true') throw new Error('B did not follow the Gamepad page back route');

    document.querySelector('[data-action="open-register"]').click();
    const registerScreen = document.querySelector('#screen-register');
    if (!registerScreen.hasAttribute('data-active') || document.querySelector('[data-route][aria-selected="true"]')) throw new Error('Create Account did not open its dedicated page');
    if (registerScreen.textContent.includes('Create an account on the server selected on Home.') || registerScreen.querySelector('input[type="email"],button[data-password-toggle]')) throw new Error('Create Account retained removed copy, email, or password visibility controls');
    const registerBack = registerScreen.querySelector('[data-action="close-register"]');
    if (!registerBack.hasAttribute('data-gamepad-back') || !registerBack.querySelector('svg') || getComputedStyle(registerBack).borderRadius !== '50%') throw new Error('Create Account did not use the dedicated circular gamepad Back action');
    const registerCard = registerScreen.querySelector('.register-card');
    if (Math.round(registerCard.getBoundingClientRect().width) !== 440) throw new Error('Create Account card did not match Avalon width');
    if (getComputedStyle(registerBack).color !== getComputedStyle(document.querySelector('#register-username')).color) throw new Error('Create Account Back arrow does not use readable control text color');
    const submit = registerScreen.querySelector('[type="submit"]');
    if (!submit.disabled) throw new Error('Create Account submit was enabled for an empty form');
    const username = document.querySelector('#register-username');
    focusAdjacentRegion(1);
    runControllerAction('down');
    if (document.activeElement !== username) throw new Error('D-pad entered the Create Account Back arrow instead of Username');
    if (document.querySelector('#register-form').autocomplete !== 'off' || [...registerScreen.querySelectorAll('input:not([type="checkbox"])')].some(input => input.autocomplete !== 'off')) throw new Error('Create Account still advertises browser autocomplete');
    const password = document.querySelector('#register-password');
    const confirm = document.querySelector('#register-confirm');
    const usernameError = document.querySelector('#register-username-error');
    const reservedErrorHeight = usernameError.getBoundingClientRect().height;
    username.value = 'ab';
    username.dispatchEvent(new Event('input', { bubbles:true }));
    if (usernameError.textContent !== '3-32 characters.' || usernameError.getBoundingClientRect().height !== reservedErrorHeight) throw new Error('Username validation copy or reserved error slot drifted');
    username.value = 'BahamutHero';
    username.dispatchEvent(new Event('input', { bubbles:true }));
    password.value = 'short';
    password.dispatchEvent(new Event('input', { bubbles:true }));
    if (document.querySelector('#register-password-error').textContent !== '8-128 characters.') throw new Error('Password validation copy drifted');
    password.value = 'password';
    password.dispatchEvent(new Event('input', { bubbles:true }));
    confirm.value = 'different';
    confirm.dispatchEvent(new Event('input', { bubbles:true }));
    if (document.querySelector('#register-confirm-error').textContent !== 'Passwords do not match.') throw new Error('Confirm-password validation copy drifted from Avalon');
    confirm.value = 'password';
    confirm.dispatchEvent(new Event('input', { bubbles:true }));
    if (submit.disabled) throw new Error('Create Account submit stayed disabled for a valid form');
    state.calls.length = 0;
    window.__launcherBrowserState.registerMode = 'network';
    submit.click();
    submit.click();
    await new Promise(resolve => setTimeout(resolve, 20));
    if (state.calls.filter(call => call.command === 'register').length !== 1 || !document.querySelector('#register-alert').textContent) throw new Error('Registration allowed duplicate submissions while the first request was pending');
    window.__launcherBrowserState.registerMode = 'username-taken';
    submit.click();
    await new Promise(resolve => setTimeout(resolve, 20));
    if (document.querySelector('#register-alert').textContent !== 'That username is already taken.') throw new Error('Taken-username copy drifted from Avalon');
    window.__launcherBrowserState.registerMode = 'network';
    submit.click();
    await new Promise(resolve => setTimeout(resolve, 20));
    if (document.querySelector('#register-alert').textContent !== 'Server unavailable.') throw new Error('Registration network-error copy drifted from Avalon');
    window.__launcherBrowserState.registerMode = 'rate-limited';
    submit.click();
    await new Promise(resolve => setTimeout(resolve, 20));
    if (document.querySelector('#register-alert').textContent !== 'Too many attempts. Wait a few minutes.') throw new Error('Registration rate-limit copy drifted from Avalon');
    window.__launcherBrowserState.registerMode = 'create-failed';
    submit.click();
    await new Promise(resolve => setTimeout(resolve, 20));
    if (document.querySelector('#register-alert').textContent !== 'Account creation failed. Create a ticket on Discord.') throw new Error('Registration create-failure copy drifted from Avalon');
    window.__launcherBrowserState.registerMode = 'server';
    submit.click();
    await new Promise(resolve => setTimeout(resolve, 20));
    if (document.querySelector('#register-alert').textContent !== 'Something went wrong. Create a ticket on Discord.') throw new Error('Registration fallback copy drifted from Avalon');
    window.__launcherBrowserState.registerMode = 'username-invalid';
    submit.click();
    await new Promise(resolve => setTimeout(resolve, 20));
    if (usernameError.textContent !== 'Username is rejected by this server.' || username.getAttribute('aria-invalid') !== 'true') throw new Error('Server username validation did not use the reserved field-error slot');
    submit.focus();
    if (document.activeElement.type !== 'submit') throw new Error('Create Account navigation did not reach submit');
    runControllerAction('cancel');
    if (!document.querySelector('#screen-home').hasAttribute('data-active')) throw new Error('B did not return from Create Account');

    document.querySelector('[data-action="open-profiles"]').click();
    const profilesScreen = document.querySelector('#screen-profiles');
    if (!profilesScreen.hasAttribute('data-active') || document.querySelector('[data-route][aria-selected="true"]')) throw new Error('Profiles did not open as a direct screen');
    runControllerAction('next-region');
    const profileSelectorCard = profilesScreen.querySelector('[data-gamepad-region-order="1"]');
    const profileEditorCard = profilesScreen.querySelector('[data-gamepad-region-order="2"]');
    if (document.activeElement !== profileSelectorCard) throw new Error('Profiles gamepad focus did not stop on the selector card');
    runControllerAction('down');
    if (document.activeElement !== document.querySelector('#profile-server-select')) throw new Error('Profiles down did not enter the selector card');
    runControllerAction('left');
    runControllerAction('right');
    if (document.activeElement !== document.querySelector('#profile-server-select')) throw new Error('Profiles selector accepted horizontal D-pad movement');
    runControllerAction('next-region');
    if (document.activeElement !== profileEditorCard) throw new Error('Profiles gamepad focus did not stop on the editor card');
    runControllerAction('down');
    if (document.activeElement !== document.querySelector('#profile-name')) throw new Error('Profiles down did not enter the editor card');
    runControllerAction('right');
    runControllerAction('left');
    if (document.activeElement !== document.querySelector('#profile-name')) throw new Error('Profiles editor accepted horizontal D-pad movement');
    runControllerAction('down');
    if (document.activeElement !== document.querySelector('#profile-host')) throw new Error('Profiles down did not advance vertically');
    runControllerAction('up');
    if (document.activeElement !== document.querySelector('#profile-name')) throw new Error('Profiles up did not reverse vertically');
    runControllerAction('cancel');
    if (!document.querySelector('#screen-home').hasAttribute('data-active')) throw new Error('B did not return from Profiles');
  `);

  await assertUi(`
    const state = window.__launcherBrowserState;
    state.calls.length = 0;
    document.querySelector('[data-utility="help"]').click();
    await new Promise(resolve => setTimeout(resolve, 50));
    const screen = document.querySelector('#screen-help');
    const log = document.querySelector('#help-log');
    if (!screen.hasAttribute('data-active') || document.querySelector('[data-route][aria-selected="true"]')) throw new Error('Help utility did not open its dedicated screen');
    if (document.querySelector('#help-dialog') || screen.querySelector('h1').textContent !== 'Need help?') throw new Error('Help still uses a dialog or the wrong title');
    if ([...screen.querySelectorAll('[data-help-action]')].map(button => button.textContent).join(',') !== 'Copy Logs,Open Logs') throw new Error('Help actions drifted');
    const getLogCall = state.calls.find(call => call.command === 'get_launcher_log');
    if (!getLogCall || Object.keys(getLogCall.args).length) throw new Error('Help log read did not use the fixed no-argument command');
    const expected = state.supportLog.content.replace(/\\r\\n?/g, '\\n').replace(/[ \\t]+$/gm, '');
    if (log.textContent !== expected || log.textContent.includes('first line  ')) throw new Error('Help log display did not normalize only line endings and trailing spaces');
    const logStyle = getComputedStyle(log);
    if (logStyle.userSelect !== 'text' || logStyle.overflowY !== 'auto' || logStyle.fontFamily.toLowerCase().indexOf('consolas') < 0) throw new Error('Help log console is not selectable, scrollable, and monospace');
    const logColorProbe = document.createElement('span');
    logColorProbe.style.color = 'var(--help-log-bg)';
    document.body.append(logColorProbe);
    const expectedLogBackground = getComputedStyle(logColorProbe).color;
    logColorProbe.remove();
    if (logStyle.backgroundColor !== expectedLogBackground) throw new Error('Help log background bypassed its semantic theme token');
    const helpActionStyle = getComputedStyle(screen.querySelector('[data-help-action]'));
    const settingsActionStyle = getComputedStyle(document.querySelector('.settings-action'));
    const actionTier = style => ({ borderRadius:style.borderRadius, fontFamily:style.fontFamily, fontSize:style.fontSize, lineHeight:style.lineHeight });
    if (helpActionStyle.borderRadius !== settingsActionStyle.borderRadius || helpActionStyle.fontFamily !== settingsActionStyle.fontFamily || helpActionStyle.fontSize !== settingsActionStyle.fontSize || helpActionStyle.lineHeight !== settingsActionStyle.lineHeight) throw new Error('Help actions drifted from the shared launcher button tier: ' + JSON.stringify({ help:actionTier(helpActionStyle), settings:actionTier(settingsActionStyle) }));
    if (!screen.textContent.includes('#install-support') || screen.querySelector('.help-subtitle [data-link]')) throw new Error('Help support copy or plain-text Discord treatment drifted');
    if (!screen.querySelector('#help-status.help-status') || screen.querySelector('#help-status').textContent) throw new Error('Help status line is missing or not initially empty');
    document.body.dataset.inputMode = 'gamepad';
    const helpActions = screen.querySelector('.help-actions');
    helpActions.focus();
    runControllerAction('down');
    if (document.activeElement !== screen.querySelector('[data-help-action="copy"]') || getComputedStyle(helpActions).boxShadow === 'none') throw new Error('Help control focus did not preserve the active card selector');

    document.querySelector('[data-help-action="copy"]').click();
    await new Promise(resolve => setTimeout(resolve, 20));
    if (state.clipboardText !== log.textContent || document.querySelector('#help-status').textContent !== 'Logs copied to clipboard.') throw new Error('Copy Logs did not copy the rendered snapshot or report success');

    state.calls.length = 0;
    document.querySelector('[data-help-action="open"]').click();
    await new Promise(resolve => setTimeout(resolve, 20));
    const openLogsCall = state.calls.find(call => call.command === 'open_launcher_log');
    if (!openLogsCall || Object.keys(openLogsCall.args).length) throw new Error('Open Logs did not use the fixed log-file command');

    state.clipboardMode = 'error';
    document.querySelector('[data-help-action="copy"]').click();
    await new Promise(resolve => setTimeout(resolve, 20));
    if (document.querySelector('#help-status').textContent !== 'Unable to copy logs.') throw new Error('clipboard failure did not report visible feedback');
    state.clipboardMode = 'success';

    log.focus();
    log.scrollTop = 0;
    runControllerAction('down');
    if (log.scrollTop <= 0) throw new Error('D-pad did not scroll the focused Help log');
    runControllerAction('cancel');
    if (document.querySelector('[data-route="home"]').getAttribute('aria-selected') !== 'true') throw new Error('B did not follow the Help page back route');
  `);

});
