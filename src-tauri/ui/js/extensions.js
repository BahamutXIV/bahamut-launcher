import { extensionState, tauriInvoke, invoke, escapeHtml } from './runtime.js';

const extensionGroupDefinitions = Object.freeze([
  { key:'overlays', label:'Overlays', empty:'No installed overlays.' },
  { key:'plugins', label:'Plugins', empty:'No installed plugins.' },
  { key:'addons', label:'Addons', empty:'No installed addons.' },
]);
const datsOverlayItem = Object.freeze({
  id:'dats-overlay',
  name:'Dats-Overlay',
  author:'',
  version:'',
  description:'Rearrange installed overlays to set DAT load priority.',
  commands:[],
  enabled:true,
  group:'overlays',
});
const plannedExtensionCommands = Object.freeze({
  'addons:fps':['/fps'],
});
const OFFICIAL_OVERLAY_ID = 'bahamut-dats-overlay';
let extensionInventoryError = '';
let extensionActionError = '';

function extensionActionErrorMarkup() {
  return extensionActionError
    ? `<p class="extension-action-error" role="alert">Unable to save extension settings: ${escapeHtml(extensionActionError)}</p>`
    : '';
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
    detail.innerHTML = '<article class="extension-detail-header glass-panel"><h2>No extension selected</h2><p class="extension-description">Select an installed plugin or addon.</p></article>';
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
  const description = item.description ? `<p class="extension-description">${escapeHtml(item.description)}</p>` : '';
  detail.innerHTML = `
        <article class="extension-detail-header glass-panel">
          <header class="extension-detail-title"><h2>${escapeHtml(item.name)}</h2>${meta}</header>
          ${description}
          ${extensionActionErrorMarkup()}
          ${commands}
        </article>`;
}

function overlayConflictSummary() {
  const conflicts = extensionState.inventory && extensionState.inventory.overlay_conflicts;
  if (!Array.isArray(conflicts) || !conflicts.length) return '';
  const rows = conflicts.map(conflict => {
    const path = conflict && conflict.relative_path;
    const packageIds = conflict && conflict.package_ids;
    if (!path || !Array.isArray(packageIds) || !packageIds.length) return '';
    return `<li><code>${escapeHtml(path)}</code><span>${escapeHtml(packageIds.join(', '))}</span></li>`;
  }).filter(Boolean).join('');
  return rows
    ? `<section class="extension-overlay-conflict-summary" data-dat-conflict-summary><h3>File conflicts</h3><p>These files are claimed by more than one enabled package; first-hit order decides which package wins.</p><ul>${rows}</ul></section>`
    : '';
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
  const packageBody = packageRows || (extensionInventoryError
    ? '<li class="extension-empty">Installed package details are unavailable.</li>'
    : '<li class="extension-empty">No installed DAT overlay packages.</li>');
  detail.innerHTML = `
        <article class="extension-detail-header glass-panel extension-overlay-detail" data-extension-detail="dats-overlay">
          <header class="extension-detail-title"><h2>Dats-Overlay</h2></header>
          <p class="extension-description">${escapeHtml(datsOverlayItem.description)}</p>
          <section class="extension-overlay-editor" aria-label="Overlay package controls">
            ${extensionActionErrorMarkup()}
            ${extensionInventoryError ? `<p class="extension-description" role="alert">Extension inventory unavailable: ${escapeHtml(extensionInventoryError)}</p>` : ''}
            <ol class="extension-overlay-packages">${packageBody}</ol>
            ${overlayConflictSummary()}
          </section>
        </article>`;
  detail.querySelectorAll('[data-dat-package-enabled]').forEach(checkbox => {
    checkbox.addEventListener('change', () => setDatPackageEnabled(checkbox.dataset.datPackageEnabled, checkbox.checked, document.activeElement === checkbox));
  });
  detail.querySelectorAll('[data-dat-move]').forEach(button => {
    button.addEventListener('click', () => setDatPackageOrder(button.dataset.datPackageId, button.dataset.datMove, document.activeElement === button));
  });
}
function renderExtensionLibrary() {
  const groups = document.querySelector('#extension-groups');
  const focused = document.activeElement;
  const focusedRow = focused?.closest?.('.extension-row');
  const focusedKey = focusedRow?.querySelector('[data-extension-item]')?.dataset.extensionItem || '';
  const focusedControl = focused?.matches?.('[data-addon-enabled]')
    ? `[data-addon-enabled="${CSS.escape(focused.dataset.addonEnabled)}"]`
    : focused?.matches?.('[data-plugin-enabled]')
      ? `[data-plugin-enabled="${CSS.escape(focused.dataset.pluginEnabled)}"]`
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
      extensionActionError = '';
      groups.querySelectorAll('[data-extension-item]').forEach(candidate => {
        const selected = candidate.dataset.extensionItem === extensionState.selectedKey;
        candidate.closest('.extension-row').dataset.current = String(selected);
        candidate.setAttribute('aria-current', String(selected));
      });
      renderExtensionDetail(extensionItems().find(item => extensionItemKey(item) === extensionState.selectedKey));
    });
  });
  groups.querySelectorAll('[data-addon-enabled]').forEach(checkbox => {
    checkbox.addEventListener('change', () => setAddonEnabled(checkbox.dataset.addonEnabled, checkbox.checked, document.activeElement === checkbox));
  });
  groups.querySelectorAll('[data-plugin-enabled]').forEach(checkbox => {
    checkbox.addEventListener('change', () => setPluginEnabled(checkbox.dataset.pluginEnabled, checkbox.checked, document.activeElement === checkbox));
  });
  renderExtensionDetail(extensionItems().find(item => extensionItemKey(item) === extensionState.selectedKey));
  if (focusedControl) groups.querySelector(focusedControl)?.focus({ preventScroll:true });
}

async function setPluginEnabled(id, enabled, restoreFocus = false) {
  extensionState.selectedKey = `plugins:${id}`;
  try {
    const commands = {
      screenshot: 'set_screenshot_enabled',
      'discord-rpc': 'set_discord_rpc_enabled',
    };
    const command = commands[id];
    if (!command) throw new Error(`Unknown plugin ${id}`);
    extensionState.inventory = await invoke(command, { enabled });
    extensionActionError = '';
    renderExtensionLibrary();
    if (restoreFocus) document.querySelector(`[data-plugin-enabled="${CSS.escape(id)}"]`)?.focus({ preventScroll:true });
  } catch (error) {
    console.error(`Unable to update ${id} state.`, error);
    extensionActionError = error?.message || String(error);
    await hydrateExtensions();
    if (restoreFocus) document.querySelector(`[data-plugin-enabled="${CSS.escape(id)}"]`)?.focus({ preventScroll:true });
  }
}

async function setAddonEnabled(id, enabled, restoreFocus = false) {
  extensionState.selectedKey = `addons:${id}`;
  try {
    extensionState.inventory = await invoke('set_addon_enabled', { id, enabled });
    extensionActionError = '';
    renderExtensionLibrary();
    if (restoreFocus) document.querySelector(`[data-addon-enabled="${CSS.escape(id)}"]`)?.focus({ preventScroll:true });
  } catch (error) {
    console.error('Unable to update addon state.', error);
    extensionActionError = error?.message || String(error);
    await hydrateExtensions();
    if (restoreFocus) document.querySelector(`[data-addon-enabled="${CSS.escape(id)}"]`)?.focus({ preventScroll:true });
  }
}

async function setDatPackageEnabled(id, enabled, restoreFocus = false) {
  extensionState.selectedKey = 'overlays:dats-overlay';
  try {
    extensionState.inventory = await invoke('set_dat_package_enabled', { id, enabled });
    extensionActionError = '';
    renderExtensionLibrary();
    if (restoreFocus) document.querySelector(`[data-dat-package-enabled="${CSS.escape(id)}"]`)?.focus({ preventScroll:true });
  } catch (error) {
    console.error('Unable to update DAT overlay package state.', error);
    extensionActionError = error?.message || String(error);
    await hydrateExtensions();
    if (restoreFocus) document.querySelector(`[data-dat-package-enabled="${CSS.escape(id)}"]`)?.focus({ preventScroll:true });
  }
}

async function setDatPackageOrder(id, direction, restoreFocus = false) {
  extensionState.selectedKey = 'overlays:dats-overlay';
  const packages = installedItems('overlays');
  const index = packages.findIndex(packageItem => packageItem.id === id);
  const position = direction === 'earlier' ? index - 1 : direction === 'later' ? index + 1 : -1;
  if (index < 0 || position < 0 || position >= packages.length) return;
  try {
    extensionState.inventory = await invoke('reorder_dat_package', { id, position });
    extensionActionError = '';
    renderExtensionLibrary();
    if (restoreFocus) focusDatPackage(id, direction);
  } catch (error) {
    console.error('Unable to reorder DAT overlay package.', error);
    extensionActionError = error?.message || String(error);
    await hydrateExtensions();
    if (restoreFocus) focusDatPackage(id, direction);
  }
}

function focusDatPackage(id, direction) {
  const selector = `[data-dat-move="${CSS.escape(direction)}"][data-dat-package-id="${CSS.escape(id)}"]`;
  const control = document.querySelector(selector);
  if (control && !control.disabled) {
    control.focus({ preventScroll:true });
    return;
  }
  document.querySelector(`[data-dat-package-enabled="${CSS.escape(id)}"]`)?.focus({ preventScroll:true });
}

async function hydrateExtensions() {
  if (!tauriInvoke) return;
  try {
    extensionState.inventory = await invoke('get_extension_inventory');
    extensionInventoryError = '';
  } catch (error) {
    extensionInventoryError = error?.message || String(error);
    extensionState.inventory = { overlays:[], plugins:[], addons:[], overlay_conflicts:[] };
  }
  renderExtensionLibrary();
}

export { hydrateExtensions, renderExtensionLibrary };
