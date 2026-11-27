// The tray popup. Everything comes from the Rust side as a "view"; this file only draws it.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const COLORS = { ok: '#8ce61e', or: '#ff9828', pk: '#ff4888', fa: '#8a8a8a' };
let view = null;
let confirming = null; // 'disconnect' | 'remove'
let busy = false;

// ---- tiny DOM helpers ----

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === undefined || value === null || value === false) continue;
    if (key === 'class') node.className = value;
    else if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value === true ? '' : value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    node.append(typeof child === 'string' ? document.createTextNode(child) : child);
  }
  return node;
}

const span = (cls, text) => el('span', { class: cls }, text);
const clock = (secs) => new Date(secs * 1000).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });

function ago(secs) {
  if (!secs) return null;
  const s = Math.max(0, Date.now() / 1000 - secs);
  if (s < 60) return ['now', ''];
  if (s < 3600) return [`${Math.floor(s / 60)} min`, 'ago'];
  if (s < 86400) return [`${Math.floor(s / 3600)} h`, 'ago'];
  return [`${Math.floor(s / 86400)} d`, 'ago'];
}

function expiry(seconds) {
  if (seconds === null || seconds === undefined) return '';
  if (seconds <= 0) return 'expired';
  if (seconds < 3600) return 'expires within the hour';
  if (seconds < 86400) return `expires in ${Math.floor(seconds / 3600)} h`;
  const days = Math.floor(seconds / 86400);
  return `expires in ${days} day${days === 1 ? '' : 's'}`;
}

// ---- actions ----

async function run(command, args) {
  if (busy) return;
  busy = true;
  draw();
  try {
    view = await invoke(command, args);
  } catch (error) {
    view = { ...view, message: String(error) };
  }
  busy = false;
  confirming = null;
  draw();
}

// ---- pieces shared by several states ----

function facts() {
  return el('div', { class: 'facts' },
    el('div', {}, span('mu', 'Signed in as'), span('mono', view.user || '—')),
    el('div', {}, span('mu', 'This device'), span('mono', view.device || '—')),
    el('div', {}, span('mu', 'Server'), span('mono', view.serverHost || '—')));
}

function tile(label, value, small, color) {
  return el('div', { class: 'tile' },
    el('div', { class: 'lbl' }, label),
    el('div', { class: `big ${color || ''}` }, value),
    el('div', { class: 'small' }, small));
}

function tiles() {
  const t = view.today || {};
  const reachable = view.status !== 'unreachable';
  const requests = reachable && t.requests !== null && t.requests !== undefined ? String(t.requests) : '—';
  const holds = reachable && t.limitHits !== null && t.limitHits !== undefined ? String(t.limitHits) : '—';
  const lastSecs = t.lastRequestUtc ? Date.parse(t.lastRequestUtc) / 1000 : null;
  const last = ago(lastSecs) || ['—', ''];
  return el('div', { class: 'tiles' },
    tile('TODAY', requests, reachable ? (t.requests === null || t.requests === undefined ? 'not recorded yet' : 'requests') : 'unavailable'),
    tile('HOLDING', holds, t.limitHits ? 'plan limit (429)' : '', t.limitHits ? 'or' : ''),
    tile('LAST', last[0], last[1]));
}

function checks() {
  const line = (state, what, where) => {
    const mark = { ok: ['✓', 'ok'], missing: ['✗', 'pk'], unreadable: ['!', 'or'], notInstalled: ['–', 'fa'] }[state] || ['?', 'fa'];
    const label = state === 'notInstalled' ? 'not installed' : state === 'unreadable' ? 'can’t read file' : where;
    return el('div', { class: 'check' }, span(mark[1], mark[0]), span('what', what), span('where', label));
  };
  return el('div', { class: 'settings' },
    el('div', { class: 'lbl' }, 'SETTINGS ON THIS COMPUTER'),
    line(view.checks.vscode, 'VS Code', 'claudeCode.environmentVariables'),
    line(view.checks.claude, 'Claude Code', '~\\.claude\\settings.json'));
}

function warnings() {
  const out = [];
  if (view.sandboxed) out.push(el('div', { class: 'panel warn' }, 'Sandbox mode: settings go to a test folder, not your real profile.'));
  if (view.exposed) out.push(el('div', { class: 'panel warn' }, 'This server uses plain http, so the sign-in token crosses the network unencrypted. Ask your administrator for an https address.'));
  if (view.checks.billingWarning) out.push(el('div', { class: 'panel warn' }, view.checks.billingWarning));
  return out;
}

function message() {
  return view.message ? el('div', { class: 'error' }, view.message) : null;
}

// ---- states ----

function firstRun() {
  const manual = view.mode === 'manual';
  const known = !!view.mode;
  const server = el('input', { id: 'server', value: view.server || '', placeholder: 'https://comptroller.example.internal', spellcheck: 'false' });
  const code = el('input', { id: 'code', placeholder: 'XXXX-XXXX-XXXX', spellcheck: 'false', autocomplete: 'off' });

  const go = () => {
    if (!known || (server.value.trim() && server.value.trim() !== view.server)) {
      run('set_server', { server: server.value });
    } else {
      run('connect', { code: manual ? code.value : null });
    }
  };
  for (const input of [server, code]) input.addEventListener('keydown', (e) => { if (e.key === 'Enter') go(); });

  const intro = manual
    ? 'Connect this computer so Claude Code goes through your team’s comptroller. Enter the one-time code from your administrator.'
    : `Connect this computer so Claude Code goes through your team’s comptroller. You’ll be signed in with your Windows login${view.windowsUser ? `, ${view.windowsUser}` : ''}.`;

  const step = (text, mono) => el('div', {}, span('ok', '✓ '), text, mono ? span('mono', mono) : null);
  return [
    el('div', { class: 'mu' }, known ? intro : 'Enter your team’s claude-comptroller address. Your administrator has it.'),
    el('label', {}, 'Server', server),
    manual ? el('label', {}, 'One-time code', code) : null,
    known ? el('div', { class: 'steps' },
      manual ? step('Redeem the one-time code') : step('Sign in silently with Windows'),
      !manual && view.allowedGroup ? step('Check you’re in ', view.allowedGroup) : null,
      step('Write VS Code and Claude Code settings'),
      el('div', { class: 'fa' }, '· Start with Windows')) : null,
    ...warnings(),
    message(),
    el('button', { class: 'big plain', onclick: go, disabled: busy }, busy ? 'Connecting…' : known ? 'Connect' : 'Continue'),
    el('div', { class: 'note' }, 'Restart VS Code once afterwards so it picks up the new settings.'),
  ];
}

function connected() {
  return [
    el('div', { class: 'mu' }, 'Claude Code on this computer goes through claude-comptroller.'),
    ...warnings(),
    facts(),
    tiles(),
    el('div', { class: 'row' },
      el('span', {}, span('mu', 'Token '), span(`mono ${view.expiringSoon ? 'or' : ''}`, expiry(view.secondsLeft))),
      el('button', { class: 'link', onclick: () => run('refresh'), disabled: busy }, busy ? 'Refreshing…' : 'Refresh now')),
    checks(),
    message(),
    el('button', { class: 'big danger', onclick: () => { confirming = 'disconnect'; draw(); }, disabled: busy }, 'Disconnect'),
    el('div', { class: 'note' }, 'Disconnect blocks Claude on this computer until you reconnect.'),
  ];
}

function disconnected() {
  return [
    el('div', { class: 'panel alert' },
      'Claude is blocked on this computer. VS Code, the CLI and the Desktop Code tab get ', span('mono', '403 · disconnected'), '.',
      view.disconnectedAt ? el('div', { class: 'sub' }, `You disconnected at ${clock(view.disconnectedAt)}.`) : null),
    ...warnings(),
    facts(),
    tiles(),
    el('div', { class: 'row' }, el('span', {}, span('mu', 'Token '), span('mono', 'suspended')),
      span('mu', view.mode === 'windows' ? 'reconnect rechecks your group' : '')),
    checks(),
    message(),
    el('button', { class: 'big primary', onclick: () => run('reconnect'), disabled: busy }, busy ? 'Reconnecting…' : 'Reconnect'),
    el('div', { class: 'note' }, view.mode === 'windows' ? 'Reconnect signs in with your Windows login. No password prompt.' : ''),
  ];
}

function blocked() {
  const canRefresh = view.mode === 'windows' && (view.secondsLeft ?? 1) <= 0;
  return [
    el('div', { class: 'panel alert' }, view.message || 'Claude is blocked on this computer.',
      el('div', { class: 'sub' }, 'Claude Code requests are refused until this is resolved.')),
    ...warnings(),
    facts(),
    checks(),
    canRefresh
      ? el('button', { class: 'big primary', onclick: () => run('refresh'), disabled: busy }, busy ? 'Signing in…' : 'Sign in again')
      : el('button', { class: 'big plain', onclick: () => run('retry'), disabled: busy }, busy ? 'Checking…' : 'Check again'),
    el('div', { class: 'note' }, 'Your administrator can change this on the Users screen.'),
  ];
}

function unreachable() {
  const host = view.serverHost || 'the server';
  return [
    el('div', { class: 'panel' },
      'Claude won’t work until ', span('mono', host), ' is back. This usually isn’t anything on your computer.',
      el('div', { class: 'sub' }, `${view.lastContact ? `Last contact ${clock(view.lastContact)} · ` : ''}retrying every 30 s`)),
    ...warnings(),
    facts(),
    tiles(),
    el('div', { class: 'row' }, el('span', {}, span('mu', 'Token '), span('mono', expiry(view.secondsLeft))), span('mu', 'can’t refresh')),
    checks(),
    el('button', { class: 'big plain', onclick: () => run('retry'), disabled: busy }, busy ? 'Retrying…' : 'Retry now'),
    el('div', { class: 'note' }, 'Ask your admin to check Status in the claude-comptroller TUI.'),
  ];
}

function confirm() {
  const remove = confirming === 'remove';
  const text = remove
    ? 'Remove the comptroller settings? Claude Code on this computer will talk to Anthropic directly again, and your earlier settings are put back. Restart VS Code afterwards.'
    : 'Disconnect this computer? Claude Code stops working here until you reconnect.';
  return [
    el('div', { class: `panel ${remove ? 'warn' : 'alert'}` }, text),
    el('div', { class: 'buttons' },
      el('button', { class: 'big plain', onclick: () => { confirming = null; draw(); } }, 'Cancel'),
      el('button', { class: 'big danger', onclick: () => run(remove ? 'remove_settings' : 'disconnect'), disabled: busy },
        remove ? 'Remove' : 'Disconnect')),
  ];
}

// ---- drawing ----

const HEADER = {
  connected: ['Connected', 'ok'],
  disconnected: ['Disconnected', 'pk'],
  blocked: ['Blocked', 'pk'],
  unreachable: ['Server unreachable', 'pk'],
  notConnected: ['Not connected yet', 'fa'],
};

function draw() {
  if (!view) return;
  let [word, color] = HEADER[view.status];
  if (view.status === 'connected' && view.expiringSoon) [word, color] = ['Sign-in expires soon', 'or'];
  document.getElementById('name').textContent = view.organization ? `claude-comptroller · ${view.organization}` : 'claude-comptroller';
  const state = document.getElementById('state');
  state.textContent = word;
  state.className = color;
  const dot = document.getElementById('dot');
  dot.style.background = view.status === 'notConnected' ? 'none' : COLORS[color];
  dot.classList.toggle('hollow', view.status === 'notConnected');
  document.getElementById('blip').setAttribute('fill', COLORS[color]);
  document.getElementById('slash').toggleAttribute('hidden', !['disconnected', 'blocked'].includes(view.status));

  const body = document.getElementById('body');
  const content = confirming ? confirm() : {
    notConnected: firstRun, connected, disconnected, blocked, unreachable,
  }[view.status]();
  // Keep what the user is typing across redraws.
  const typed = Object.fromEntries([...body.querySelectorAll('input')].map((i) => [i.id, i.value]));
  body.replaceChildren(...content.filter(Boolean));
  for (const [id, value] of Object.entries(typed)) {
    const input = document.getElementById(id);
    if (input && value) input.value = value;
  }

  requestAnimationFrame(() => invoke('fit', { height: document.getElementById('popup').scrollHeight }));
}

document.getElementById('close').addEventListener('click', () => invoke('hide'));
document.addEventListener('keydown', (e) => { if (e.key === 'Escape') invoke('hide'); });

listen('view', (event) => { if (!busy) { view = event.payload; draw(); } });
listen('confirm', (event) => { confirming = event.payload; draw(); });
invoke('view').then((v) => { view = v; draw(); });
