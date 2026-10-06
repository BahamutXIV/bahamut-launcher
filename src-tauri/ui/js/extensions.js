import { extensionState, tauriInvoke, invoke, escapeHtml } from './runtime.js';
import { feedbackToken, feedbackCurrent, clearFeedback, reportFailure, recordFailure } from './feedback.js';

const extensionGroupDefinitions = Object.freeze([
  { key:'overlays', label:'Overlays', empty:'No overlays installed.' },
  { key:'plugins', label:'Plugins', empty:'No plugins installed.' },
  { key:'addons', label:'Addons', empty:'No addons installed.' },
]);
const datsOverlayItem = Object.freeze({
  id:'dats-overlay',
  name:'Dats-Overlay',
  author:'',
  version:'',
  description:'Move overlays up or down. Higher overlays take priority.',
  commands:[],
  enabled:true,
  group:'overlays',
});
const plannedExtensionCommands = Object.freeze({
  'addons:fps':['/fps'],
});
const OFFICIAL_OVERLAY_ID = 'bahamut-dats-overlay';
const EXTENSION_INVENTORY_FAILURE = 'Could not load extensions. Copy logs from the Help page for support.';
let extensionHydrationRevision = 0;

function syncExtensionInventory(inventory) {
  extensionHydrationRevision += 1;
  extensionState.inventory = inventory;
  if (document.querySelector('[data-screen="extensions"][data-active]')) renderExtensionLibrary();
}

function extensionActionName(group, id, verb) {
  const item = installedItems(group).find(candidate => candidate.id === id);
  return `${verb} ${item?.name || id}`;
}

function detailDescriptionMarkup(description) {
  return `<p class="extension-description">${escapeHtml(description)}</p>`;
}

function focusExtensionControl(selector, fallbackSelector = '') {
  const control = selector ? document.querySelector(selector) : null;
  if (control && !control.disabled) {
    control.focus({ preventScroll:true });
    return;
  }
  if (fallbackSelector) document.querySelector(fallbackSelector)?.focus({ preventScroll:true });
}

function extensionItems() {
  if (!extensionState.inventory) return [];
  return [datsOverlayItem, ...extensionGroupDefinitions.filter(group => group.key !== 'overlays').flatMap(group =>
    installedItems(group.key).map(item => ({ ...item, group:group.key }))
  )];
}

function installedItems(groupKey) {
  const inventory = (extensionState.inventory && extensionState.inventory[groupKey]) || [];
  return inventory;
}

function extensionItemKey(item) {
  return `${item.group}:${item.id}`;
}

function renderExtensionDetail(item) {
  const detail = document.querySelector('#extension-detail');
  if (!item) {
    detail.innerHTML = `<article class="extension-detail-header glass-panel"><h2>No extension selected</h2>${detailDescriptionMarkup('Select an installed plugin or addon.')}</article>`;
    return;
  }
  if (item.group === 'overlays' && item.id === datsOverlayItem.id) {
    renderDatsOverlayDetail(detail);
    return;
  }
  const hasImplementedCommands = Boolean(item.commands && item.commands.length);
  const commandUsages = hasImplementedCommands
    ? item.commands.map(command => command.usage)
    : (plannedExtensionCommands[extensionItemKey(item)] || []);
  const visibleCommands = item.group === 'addons'
    ? [...new Set(commandUsages.map(usage => usage.trim().split(/\s+/)[0]))]
    : commandUsages;
  const commands = visibleCommands.length
    ? `<div class="extension-commands" aria-label="${hasImplementedCommands ? 'Commands' : 'Planned commands'}">${visibleCommands.map(command => `<code class="extension-command">${escapeHtml(command)}</code>`).join('')}</div>`
    : '';
  const version = String(item.version || '').trim();
  const versionLabel = version && !version.toLowerCase().startsWith('v') ? `v${version}` : version;
  const metaItems = [
    item.author ? `<span class="extension-meta-item">Author: ${escapeHtml(item.author)}</span>` : '',
    versionLabel ? `<span class="extension-meta-item">Version: ${escapeHtml(versionLabel)}</span>` : '',
  ].filter(Boolean).join('');
  const meta = metaItems ? `<div class="extension-meta">${metaItems}</div>` : '';
  const description = detailDescriptionMarkup(item.description || '');
  detail.innerHTML = `
        <article class="extension-detail-header glass-panel">
          <header class="extension-detail-title"><h2>${escapeHtml(item.name)}</h2>${meta}</header>
          ${description}
          ${commands}
        </article>`;
}

function renderDatsOverlayDetail(detail) {
  const packages = installedItems('overlays');
  const officialFirst = packages[0]?.id === OFFICIAL_OVERLAY_ID;
  const packageRows = packages.map((packageItem, index) => {
    const id = escapeHtml(packageItem.id);
    const name = escapeHtml(packageItem.name || packageItem.id);
    const locked = packageItem.id === OFFICIAL_OVERLAY_ID;
    const earlierDisabled = index === 0 || (officialFirst && index === 1) ? ' disabled' : '';
    const laterDisabled = index === packages.length - 1 ? ' disabled' : '';
    const toggle = locked
      ? '<span class="extension-overlay-toggle" aria-hidden="true"></span>'
      : `<label class="extension-toggle extension-overlay-toggle"><input type="checkbox" data-dat-package-enabled="${id}" aria-label="Enable ${name}"${packageItem.enabled ? ' checked' : ''}></label>`;
    const order = locked
      ? '<span class="extension-overlay-order-actions" aria-hidden="true"></span>'
      : `<div class="extension-overlay-order-actions" aria-label="Reorder ${name}">
              <button class="secondary-button extension-overlay-order-button" type="button" data-dat-move="earlier" data-dat-package-id="${id}" aria-label="Move ${name} earlier"${earlierDisabled}><span aria-hidden="true">&#8593;</span></button>
              <button class="secondary-button extension-overlay-order-button" type="button" data-dat-move="later" data-dat-package-id="${id}" aria-label="Move ${name} later"${laterDisabled}><span aria-hidden="true">&#8595;</span></button>
            </div>`;
    return `
          <li class="extension-overlay-package" data-dat-package="${id}" data-enabled="${locked || Boolean(packageItem.enabled)}" data-locked="${locked}">
            ${toggle}
            <div class="extension-overlay-package-copy"><h3>${name}</h3></div>
            ${order}
            <span class="extension-overlay-order-label" aria-hidden="true">#${index + 1}</span>
          </li>`;
  }).join('');
  const packageBody = packageRows || '<li class="extension-empty">No DAT overlay packages installed.</li>';
  detail.innerHTML = `
        <article class="extension-detail-header glass-panel extension-overlay-detail" data-extension-detail="dats-overlay">
          <header class="extension-detail-title"><h2>Dats-Overlay</h2></header>
          ${detailDescriptionMarkup(datsOverlayItem.description)}
          <section class="extension-overlay-editor" aria-label="Overlay package controls">
            <ol class="extension-overlay-packages">${packageBody}</ol>
          </section>
        </article>`;
  detail.querySelectorAll('[data-dat-package-enabled]').forEach(checkbox => {
    checkbox.addEventListener('change', () => setDatPackageEnabled(checkbox.dataset.datPackageEnabled, checkbox.checked));
  });
  detail.querySelectorAll('[data-dat-move]').forEach(button => {
    button.addEventListener('click', () => setDatPackageOrder(button.dataset.datPackageId, button.dataset.datMove));
  });
}
function renderExtensionLibrary() {
  const groups = document.querySelector('#extension-groups');
  const focused = document.activeElement;
  const focusedRow = focused?.closest?.('.extension-row');
  const focusedKey = focusedRow?.querySelector('[data-extension-item]')?.dataset.extensionItem || '';
  const focusedDatPackage = focused?.matches?.('[data-dat-move]')
    ? `[data-dat-package-enabled="${CSS.escape(focused.dataset.datPackageId)}"]`
    : '';
  const focusedControl = focused?.matches?.('[data-addon-enabled]')
    ? `[data-addon-enabled="${CSS.escape(focused.dataset.addonEnabled)}"]`
    : focused?.matches?.('[data-plugin-enabled]')
      ? `[data-plugin-enabled="${CSS.escape(focused.dataset.pluginEnabled)}"]`
      : focused?.matches?.('[data-dat-package-enabled]')
        ? `[data-dat-package-enabled="${CSS.escape(focused.dataset.datPackageEnabled)}"]`
        : focused?.matches?.('[data-dat-move]')
          ? `[data-dat-move="${CSS.escape(focused.dataset.datMove)}"][data-dat-package-id="${CSS.escape(focused.dataset.datPackageId)}"]`
      : focusedKey ? `[data-extension-item="${CSS.escape(focusedKey)}"]` : '';
  const query = extensionState.query.trim().toLowerCase();
  let firstVisible = null;
  const visibleKeys = new Set();
  groups.innerHTML = extensionGroupDefinitions.map(group => {
    const inventory = installedItems(group.key);
    const installed = group.key === 'overlays'
      ? [datsOverlayItem]
      : [...inventory].sort((left, right) => String(left.name || left.id).localeCompare(
        String(right.name || right.id), undefined, { sensitivity:'base' }
      ));
    const visible = installed.filter(item => {
      const plannedCommands = plannedExtensionCommands[extensionItemKey({ ...item, group:group.key })] || [];
      const commandText = `${(item.commands || []).map(command => `${command.name} ${command.usage} ${command.description}`).join(' ')} ${plannedCommands.join(' ')}`;
      const overlayPackageText = group.key === 'overlays'
        ? inventory.map(packageItem => `${packageItem.name} ${packageItem.id}`).join(' ')
        : '';
      return `${item.name} ${item.author} ${item.description} ${commandText} ${overlayPackageText}`.toLowerCase().includes(query);
    });
    const rows = visible.map(item => {
      const withGroup = { ...item, group:group.key };
      const key = extensionItemKey(withGroup);
      visibleKeys.add(key);
      if (!firstVisible) firstVisible = withGroup;
      let toggle = '<span class="extension-toggle" aria-hidden="true"></span>';
      if (group.key === 'addons') {
        toggle = `<label class="extension-toggle"><input type="checkbox" data-addon-enabled="${escapeHtml(item.id)}" aria-label="Enable ${escapeHtml(item.name)}" ${item.enabled ? 'checked' : ''}></label>`;
      } else if (group.key === 'plugins') {
        toggle = `<label class="extension-toggle"><input type="checkbox" data-plugin-enabled="${escapeHtml(item.id)}" aria-label="Enable ${escapeHtml(item.name)}" ${item.enabled ? 'checked' : ''}></label>`;
      }
      return `<div class="extension-row" data-extension-group="${group.key}" data-current="${key === extensionState.selectedKey}" data-enabled="${Boolean(item.enabled)}">${toggle}<button class="extension-row-select" type="button" data-extension-item="${escapeHtml(key)}" aria-current="${key === extensionState.selectedKey}"><div class="extension-row-copy"><span class="extension-row-name">${escapeHtml(item.name)}</span></div></button></div>`;
    }).join('');
    const empty = rows ? '' : `<div class="extension-empty">${escapeHtml(query && installed.length ? 'No matches in this group.' : group.empty)}</div>`;
    return `<section class="extension-group" data-extension-group="${group.key}"><div class="extension-group-title">${group.label}</div>${rows}${empty}</section>`;
  }).join('');

  if (!visibleKeys.has(extensionState.selectedKey)) {
    extensionState.selectedKey = firstVisible ? extensionItemKey(firstVisible) : null;
  }
  groups.querySelectorAll('[data-extension-item]').forEach(button => {
    const current = button.dataset.extensionItem === extensionState.selectedKey;
    button.closest('.extension-row').dataset.current = String(current);
    button.setAttribute('aria-current', String(current));
    button.addEventListener('click', () => {
      extensionState.selectedKey = button.dataset.extensionItem;
      groups.querySelectorAll('[data-extension-item]').forEach(candidate => {
        const selected = candidate.dataset.extensionItem === extensionState.selectedKey;
        candidate.closest('.extension-row').dataset.current = String(selected);
        candidate.setAttribute('aria-current', String(selected));
      });
      renderExtensionDetail(extensionItems().find(item => extensionItemKey(item) === extensionState.selectedKey));
    });
  });
  groups.querySelectorAll('[data-addon-enabled]').forEach(checkbox => {
    checkbox.addEventListener('change', () => setAddonEnabled(checkbox.dataset.addonEnabled, checkbox.checked));
  });
  groups.querySelectorAll('[data-plugin-enabled]').forEach(checkbox => {
    checkbox.addEventListener('change', () => setPluginEnabled(checkbox.dataset.pluginEnabled, checkbox.checked));
  });
  renderExtensionDetail(extensionItems().find(item => extensionItemKey(item) === extensionState.selectedKey));
  if (focusedControl) focusExtensionControl(focusedControl, focusedDatPackage);
}

async function setPluginEnabled(id, enabled) {
  extensionState.selectedKey = `plugins:${id}`;
  const token = feedbackToken('#extension-groups');
  const action = extensionActionName('plugins', id, enabled ? 'enable' : 'disable');
  clearFeedback(token);
  try {
    const commands = {
      screenshot: 'set_screenshot_enabled',
      'discord-rpc': 'set_discord_rpc_enabled',
    };
    const command = commands[id];
    if (!command) throw new Error(`Unknown plugin ${id}`);
    syncExtensionInventory(await invoke(command, { enabled }));
  } catch (error) {
    await hydrateExtensions(token);
    reportFailure(token, action, error, { context:`set_plugin_enabled:${id}` });
  }
}

async function setAddonEnabled(id, enabled) {
  extensionState.selectedKey = `addons:${id}`;
  const token = feedbackToken('#extension-groups');
  const action = extensionActionName('addons', id, enabled ? 'enable' : 'disable');
  clearFeedback(token);
  try {
    syncExtensionInventory(await invoke('set_addon_enabled', { id, enabled }));
  } catch (error) {
    await hydrateExtensions(token);
    reportFailure(token, action, error, { context:`set_addon_enabled:${id}` });
  }
}

async function setDatPackageEnabled(id, enabled) {
  extensionState.selectedKey = 'overlays:dats-overlay';
  const token = feedbackToken('#extension-detail');
  const action = extensionActionName('overlays', id, enabled ? 'enable' : 'disable');
  clearFeedback(token);
  try {
    syncExtensionInventory(await invoke('set_dat_package_enabled', { id, enabled }));
  } catch (error) {
    await hydrateExtensions(token);
    reportFailure(token, action, error, { context:`set_dat_package_enabled:${id}` });
  }
}

async function setDatPackageOrder(id, direction) {
  extensionState.selectedKey = 'overlays:dats-overlay';
  const packages = installedItems('overlays');
  const index = packages.findIndex(packageItem => packageItem.id === id);
  const position = direction === 'earlier' ? index - 1 : direction === 'later' ? index + 1 : -1;
  if (index < 0 || position < 0 || position >= packages.length) return;
  const token = feedbackToken('#extension-detail');
  const packageItem = packages.find(candidate => candidate.id === id);
  const action = `move ${packageItem?.name || id} ${direction}`;
  clearFeedback(token);
  try {
    syncExtensionInventory(await invoke('reorder_dat_package', { id, position }));
  } catch (error) {
    await hydrateExtensions(token);
    reportFailure(token, action, error, { context:`reorder_dat_package:${id}:${direction}` });
  }
}

async function hydrateExtensions(lifetimeToken) {
  if (!tauriInvoke) return;
  const token = feedbackToken('#extension-groups');
  const revision = ++extensionHydrationRevision;
  const canShow = candidate => feedbackCurrent(candidate) && (!lifetimeToken || feedbackCurrent(lifetimeToken));
  if (canShow(token)) clearFeedback(token);
  try {
    const inventory = await invoke('get_extension_inventory');
    if (revision !== extensionHydrationRevision) return;
    extensionState.inventory = inventory;
    if (!canShow(token)) return;
    renderExtensionLibrary();
  } catch (error) {
    if (revision !== extensionHydrationRevision || !canShow(token)) {
      void recordFailure(token.scope, 'load extension inventory', error, EXTENSION_INVENTORY_FAILURE, 'get_extension_inventory');
      return;
    }
    extensionState.inventory = { overlays:[], plugins:[], addons:[] };
    renderExtensionLibrary();
    reportFailure(token, 'load extension inventory', error, {
      message:EXTENSION_INVENTORY_FAILURE,
      context:'get_extension_inventory',
    });
    return;
  }
}

export { hydrateExtensions, renderExtensionLibrary };
