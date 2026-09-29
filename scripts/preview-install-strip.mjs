import { createServer } from 'node:http';
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { extname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const uiRoot = join(repositoryRoot, 'src-tauri', 'ui');
const browserCandidates = [
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
  'C:/Program Files/Microsoft/Edge/Application/msedge.exe',
];
const stages = [
  ['install', 'Install'],
  ['installing', 'Installing'],
  ['downloading', 'Downloading'],
  ['installing-files', 'Installing Files'],
  ['validating-files', 'Final Install Check'],
  ['paused', 'Paused'],
  ['cancelled', 'Install Cancelled'],
  ['failed', 'Install Failed'],
  ['ready', 'Ready'],
];
const contentTypes = new Map([
  ['.css', 'text/css; charset=utf-8'],
  ['.html', 'text/html; charset=utf-8'],
  ['.js', 'text/javascript; charset=utf-8'],
  ['.jpg', 'image/jpeg'],
  ['.png', 'image/png'],
  ['.ttf', 'font/ttf'],
]);

function browserPath() {
  const path = browserCandidates.find(candidate => existsSync(candidate));
  if (!path) throw new Error('Chrome or Edge is required for the install-strip preview.');
  return path;
}

function fixtureSource() {
  return String.raw`
    (() => {
      const stage = new URLSearchParams(location.search).get('stage') || 'install';
      const labels = Object.fromEntries(${JSON.stringify(stages)});
      const installByStage = {
        installing:{ phase:'starting', is_running:true, is_paused:false, pause_requested:false, is_terminal:false, error:null },
        downloading:{ phase:'downloading', is_running:true, is_paused:false, pause_requested:false, is_terminal:false, download_idx:0, previous_completed_bytes:0, bytes_downloaded:402653184, total_download_bytes:7770236904 },
        'installing-files':{ phase:'installing', is_running:true, is_paused:false, pause_requested:false, is_terminal:false, file_idx:38500, total_files:191958 },
        'validating-files':{ phase:'validating-files', is_running:true, is_paused:false, pause_requested:false, is_terminal:false, file_idx:100000, total_files:191960 },
        paused:{ phase:'downloading', is_running:true, is_paused:true, pause_requested:false, is_terminal:false, previous_completed_bytes:0, bytes_downloaded:2542620639, total_download_bytes:7770236904 },
        cancelled:{ phase:'cancelled', is_running:false, is_paused:false, pause_requested:false, is_terminal:true, error:null },
        failed:{ phase:'error', is_running:false, is_paused:false, pause_requested:false, is_terminal:true, error:'The content host could not provide a required file.' },
      };
      const idleInstall = { phase:'idle', download_idx:0, file_idx:0, total_files:0, bytes_downloaded:0, previous_completed_bytes:0, total_download_bytes:0, is_running:false, is_paused:false, pause_requested:false, is_terminal:false, error:null };
      const serverSettings = {
        selected_server:'Bahamut',
        servers:[{ display_name:'Bahamut', host:'bahamut.stegall.me', auth_port:443, lobby_port:54994, use_https:true }]
      };
      const borderlessMonitors = {
        supported:true,
        monitors:[{ id:'preview-primary', name:'Preview Display', width:1920, height:1080, primary:true }],
        selected:null
      };
      document.title = 'Bahamut Install Strip - ' + (labels[stage] || 'Install');
      window.__TAURI__ = { core:{ invoke:async (command, args = {}) => {
        if (command === 'fit_window_to_work_area') return null;
        if (command === 'get_server_settings') return structuredClone(serverSettings);
        if (command === 'list_news') return [{ date:'Preview', title:'Install Strip Simulation', body:'This isolated preview does not change launcher or client files.' }];
        if (command === 'launcher_version') return 'preview';
        if (command === 'validate_session') return 'invalid';
        if (command === 'get_home_status') {
          const state = stage === 'ready' ? 'logged-out' : 'no-valid-install';
          return {
            state, eyebrow:'', title:'Account Login',
            primary_action:state === 'no-valid-install' ? 'Install' : 'Play',
            game_dir:state === 'no-valid-install' ? null : 'C:/LegacyClient',
            default_game_dir:'C:/Games/FINAL FANTASY XIV',
            game_version:null
          };
        }
        if (command === 'install_status') return structuredClone(installByStage[stage] || idleInstall);
        if (command === 'get_borderless_monitors') return structuredClone(borderlessMonitors);
        if (command === 'set_borderless_monitor') {
          borderlessMonitors.selected = args.monitorId ?? null;
          return structuredClone(borderlessMonitors);
        }
        if (command === 'install_quote') return { download_bytes:7770236904, staging_bytes:14000000000, destination_bytes:12947445672, available_cache_bytes:21474836480, available_destination_bytes:34359738368 };
        if (command === 'pick_directory') return 'C:/Games/FINAL FANTASY XIV';
        if (command === 'get_launcher_behavior') return { close_on_game_start:true };
        if (command === 'get_game_settings') return { available:false, settings:null, supported_resolutions:[], error:'Not available in preview.' };
        if (command === 'get_extension_inventory') return { overlays:[], overlay_conflicts:[], plugins:[], addons:[] };
        if (command === 'get_launcher_log') return { content:'Install-strip preview. No launcher state is being changed.', log_path:'Preview only', truncated:false, updated_at:Date.now() };
        return null;
      } } };
    })();`;
}

function serve(request, response) {
  const requestUrl = new URL(request.url || '/', 'http://127.0.0.1');
  const requestPath = decodeURIComponent(requestUrl.pathname === '/' ? '/index.html' : requestUrl.pathname);
  const filePath = resolve(uiRoot, `.${requestPath}`);
  const relativePath = relative(uiRoot, filePath);
  if (relativePath.startsWith(`..${sep}`) || relativePath === '..' || !existsSync(filePath) || !statSync(filePath).isFile()) {
    response.writeHead(404);
    response.end();
    return;
  }

  const contentType = contentTypes.get(extname(filePath).toLowerCase()) || 'application/octet-stream';
  response.writeHead(200, { 'content-type':contentType, 'cache-control':'no-store' });
  if (filePath === join(uiRoot, 'index.html')) {
    const html = readFileSync(filePath, 'utf8').replace(
      '  <script type="module" src="js/main.js"></script>',
      `  <script>${fixtureSource()}</script>\n  <script type="module" src="js/main.js"></script>`,
    );
    response.end(html);
    return;
  }
  response.end(readFileSync(filePath));
}

const server = createServer(serve);
await new Promise(resolvePromise => server.listen(0, '127.0.0.1', resolvePromise));
const port = server.address().port;
const profileRoot = mkdtempSync(join(tmpdir(), 'bahamut-install-strip-preview-'));
const urls = stages.map(([stage]) => `http://127.0.0.1:${port}/index.html?stage=${stage}`);
const browser = spawn(browserPath(), [
  `--user-data-dir=${profileRoot}`,
  '--new-window',
  '--window-size=1280,800',
  ...urls,
], { stdio:'ignore' });

console.log(`Install-strip preview opened at ${urls[0]}. Use Ctrl+Tab to cycle through the labeled stages.`);

const cleanup = async () => {
  await new Promise(resolvePromise => server.close(resolvePromise));
  rmSync(profileRoot, { recursive:true, force:true, maxRetries:20, retryDelay:100 });
};

browser.once('exit', () => cleanup().catch(error => console.error(error)));
browser.once('error', async error => {
  await cleanup().catch(() => {});
  throw error;
});
