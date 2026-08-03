/* TigridenR web client: mirrors the native chrome and attaches xterm.js to
 * the live desktop terminals over one websocket. */
(function () {
  'use strict';

  // Fallback palette (classic dark) used until the host publishes its theme;
  // src/remote/state.rs sends the real one with every state update.
  var THEME = {
    background: '#1e2227',
    foreground: '#d6dbe1',
    cursor: '#d6dbe1',
    cursorAccent: '#1e2227',
    selectionBackground: '#39414b',
    black: '#1e2227', red: '#e05f65', green: '#8cc265', yellow: '#e2b04c',
    blue: '#5c9ce0', magenta: '#c684dd', cyan: '#51baba', white: '#d6dbe1',
    brightBlack: '#5c6570', brightRed: '#ef8388', brightGreen: '#a5d680',
    brightYellow: '#f0c674', brightBlue: '#85b8ef', brightMagenta: '#d9a4ed',
    brightCyan: '#7bd4d4', brightWhite: '#f0f3f6'
  };

  var ANSI_NAMES = [
    'black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white',
    'brightBlack', 'brightRed', 'brightGreen', 'brightYellow',
    'brightBlue', 'brightMagenta', 'brightCyan', 'brightWhite'
  ];

  var appliedTheme = '';

  // Mirrors the desktop appearance: CSS custom properties for the chrome,
  // an xterm theme for the terminal.
  function applyTheme(t) {
    if (!t || !t.ansi || t.ansi.length < 16) return;
    var key = JSON.stringify(t);
    if (key === appliedTheme) return;
    appliedTheme = key;

    var css = document.documentElement.style;
    css.setProperty('--bg', t.bg);
    css.setProperty('--panel', t.panel);
    css.setProperty('--panel-hover', t.panel_hover);
    css.setProperty('--selection', t.selection);
    css.setProperty('--border', t.border);
    css.setProperty('--text', t.text);
    css.setProperty('--text-dim', t.text_dim);
    css.setProperty('--accent', t.accent);
    if (t.ui_font_size) {
      css.setProperty('--font', t.ui_font_size +
        'px -apple-system, "Segoe UI", system-ui, sans-serif');
    }
    document.querySelector('meta[name="theme-color"]').setAttribute('content', t.bg);

    var xterm = {
      background: t.ansi[0],
      foreground: t.ansi[7],
      cursor: t.ansi[7],
      cursorAccent: t.ansi[0],
      selectionBackground: t.selection
    };
    ANSI_NAMES.forEach(function (name, i) { xterm[name] = t.ansi[i]; });
    term.options.theme = xterm;

    if (t.font_family) {
      term.options.fontFamily = '"' + t.font_family +
        '", Menlo, Consolas, ui-monospace, monospace';
    }
    // The host's size is the default; a manual choice still wins.
    if (!manualFont && t.font_size) {
      term.options.fontSize = t.font_size;
      showFontSize();
    }
  }

  var $ = function (id) { return document.getElementById(id); };

  var term = new Terminal({
    theme: THEME,
    fontFamily: 'Menlo, Consolas, ui-monospace, monospace',
    fontSize: 13,
    scrollback: 10000,
    allowProposedApi: true
  });
  term.open($('term'));

  var ws = null;
  var state = null;          // last "t":"state" payload
  var currentTerm = 0;       // attached term id, 0 = none
  var resizable = false;     // whether the host honors resize (headless)
  var reconnectDelay = 500;
  var utf8 = new TextDecoder();

  function send(obj) {
    if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(obj));
  }

  // ---- connection ----

  function connect() {
    var proto = location.protocol === 'https:' ? 'wss://' : 'ws://';
    ws = new WebSocket(proto + location.host + '/ws');
    ws.binaryType = 'arraybuffer';

    ws.onopen = function () {
      $('conn-dot').className = 'on';
      reconnectDelay = 500;
      // State arrives first; attach happens from renderState.
    };
    ws.onclose = function () {
      $('conn-dot').className = 'off';
      currentTerm = 0;
      setTimeout(connect, reconnectDelay);
      reconnectDelay = Math.min(reconnectDelay * 2, 8000);
    };
    ws.onerror = function () { ws.close(); };

    ws.onmessage = function (ev) {
      if (typeof ev.data === 'string') {
        var msg = JSON.parse(ev.data);
        if (msg.t === 'state') { state = msg; renderState(); }
        else if (msg.t === 'attached') {
          resizable = !!msg.resizable;
          term.reset();
          term.resize(msg.cols, msg.rows);
          hideOverlay();
          if (resizable) fitResize(); else fitFont(msg.cols);
        }
        else if (msg.t === 'exited') {
          if (msg.term === currentTerm) showOverlay('shell exited');
        }
        else if (msg.t === 'file') { showFile(msg); }
      } else {
        var bytes = new Uint8Array(ev.data);
        var id = 0;
        // 8-byte LE id; ids stay well below 2^53.
        for (var i = 7; i >= 0; i--) id = id * 256 + bytes[i];
        if (id === currentTerm) term.write(bytes.subarray(8));
      }
    };
  }

  function attach(id) {
    if (id === currentTerm) return;
    if (currentTerm) send({ t: 'detach', term: currentTerm });
    currentTerm = id;
    if (id) send({ t: 'attach', term: id });
  }

  function resync() {
    var id = currentTerm;
    if (!id) return;
    send({ t: 'detach', term: id });
    currentTerm = 0;
    attach(id);
  }

  // ---- terminal input ----

  term.onData(function (data) {
    if (currentTerm) send({ t: 'input', term: currentTerm, data: data });
  });

  // ---- sizing ----

  function cellSize() {
    var core = term._core;
    var dims = core && core._renderService && core._renderService.dimensions;
    if (dims && dims.css && dims.css.cell.width) {
      return { w: dims.css.cell.width, h: dims.css.cell.height };
    }
    return { w: term.options.fontSize * 0.6, h: term.options.fontSize * 1.35 };
  }

  // Host owns the grid: pick the largest font that fits its columns, but
  // never shrink below MIN_FIT — an unreadable wall of text is worse than
  // scrolling sideways.
  var MIN_FIT = 11, MIN_FONT = 8, MAX_FONT = 32;

  function fitFont(cols) {
    if (manualFont) { term.options.fontSize = manualFont; return; }
    var wrap = $('term-wrap');
    var avail = wrap.clientWidth - 10;
    if (avail <= 0 || !cols) return;
    var cell = cellSize();
    var ideal = Math.floor(term.options.fontSize * avail / (cols * cell.w));
    term.options.fontSize = Math.max(MIN_FIT, Math.min(18, ideal));
    showFontSize();
  }

  // ---- font size control ----

  var FONT_KEY = 'tigridenr.fontSize';
  var manualFont = parseFloat(localStorage.getItem(FONT_KEY)) || 0; // 0 = auto-fit

  function showFontSize() {
    var el = $('font-size');
    el.textContent = Math.round(term.options.fontSize);
    el.className = manualFont ? '' : 'auto';
  }

  function nudgeFont(delta) {
    var next = (manualFont || term.options.fontSize) + delta;
    manualFont = Math.max(MIN_FONT, Math.min(MAX_FONT, next));
    localStorage.setItem(FONT_KEY, manualFont);
    term.options.fontSize = manualFont;
    showFontSize();
    if (resizable && currentTerm) fitResize();
  }

  function autoFont() {
    manualFont = 0;
    localStorage.removeItem(FONT_KEY);
    if (currentTerm) { if (resizable) fitResize(); else fitFont(term.cols); }
    showFontSize();
  }

  $('font-dec').onclick = function () { nudgeFont(-1); };
  $('font-inc').onclick = function () { nudgeFont(1); };
  $('font-size').onclick = autoFont;

  // Headless: the client drives the PTY size from its own viewport, at
  // whatever font size the user chose.
  function fitResize() {
    if (manualFont) term.options.fontSize = manualFont;
    var wrap = $('term-wrap');
    var cell = cellSize();
    var cols = Math.max(2, Math.floor((wrap.clientWidth - 10) / cell.w));
    var rows = Math.max(1, Math.floor(wrap.clientHeight / cell.h));
    if (cols !== term.cols || rows !== term.rows) {
      term.resize(cols, rows);
      send({ t: 'resize', term: currentTerm, cols: cols, rows: rows });
    }
    showFontSize();
  }

  var resizeTimer = null;
  function onViewportChange() {
    // Force the app height only while the soft keyboard shrinks the visual
    // viewport (mobile); otherwise let CSS 100dvh own the layout so the
    // bottom bar always sits at the window edge on desktop.
    var app = document.getElementById('app');
    if (window.visualViewport && window.visualViewport.height < window.innerHeight - 50) {
      app.style.height = window.visualViewport.height + 'px';
      window.scrollTo(0, 0);
    } else {
      app.style.height = '';
    }
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(function () {
      if (!currentTerm) return;
      if (resizable) fitResize(); else fitFont(term.cols);
    }, 120);
  }
  window.addEventListener('resize', onViewportChange);
  if (window.visualViewport) window.visualViewport.addEventListener('resize', onViewportChange);

  // ---- chrome rendering ----

  function renderState() {
    if (!state) return;
    applyTheme(state.theme);
    if (state.title) {
      $('brand').textContent = state.title.split('—')[0].trim() || 'TigridenR';
      document.title = state.title;
    }
    var session = state.sessions[state.active_session];
    $('session-name').textContent = session ? '— ' + session.name : '';
    renderTabs(session);
    renderPresets();
    renderTree();
    // Follow the host's active terminal.
    if (session && session.terms.length) {
      attach(session.terms[session.active_term] || session.terms[0]);
      var exited = session.exited[session.active_term];
      if (exited) showOverlay('shell exited'); else if (currentTerm) hideOverlay();
    } else {
      attach(0);
      showOverlay(state.sessions.length ? 'no terminal' : 'no session — add a folder on the desktop');
    }
  }

  // Tabs switch only — closing has its own button next to "+", so a mis-tap
  // can't kill a shell.
  function renderTabs(session) {
    var tabs = $('tabs');
    tabs.textContent = '';
    $('close-term').className = session && session.terms.length > 1 ? '' : 'hidden';
    if (!session) return;
    session.terms.forEach(function (id, i) {
      var tab = document.createElement('button');
      tab.className = 'tab' + (i === session.active_term ? ' active' : '');
      tab.textContent = String(i + 1);
      tab.onclick = function () {
        send({ t: 'select_term', session: state.active_session, tab: i });
      };
      tabs.appendChild(tab);
    });
  }

  function renderPresets() {
    var presets = $('presets');
    presets.textContent = '';
    (state.presets || []).forEach(function (label, i) {
      var btn = document.createElement('button');
      btn.className = 'preset';
      btn.textContent = label;
      btn.onclick = function () { send({ t: 'preset', i: i, idx: i }); };
      presets.appendChild(btn);
    });
  }

  function renderTree() {
    var tree = $('tree');
    tree.textContent = '';
    (state.tree || []).forEach(function (row) {
      var el = document.createElement('div');
      var name = document.createElement('span');
      name.className = 'name';
      name.textContent = row.name;
      el.style.paddingLeft = (8 + row.indent * 14) + 'px';
      if (row.kind === 0) {
        el.className = 'row header' + (row.session === state.active_session ? ' active' : '');
        el.onclick = function () { send({ t: 'select_session', idx: row.session }); closeSidebar(); };
      } else if (row.kind === 1) {
        el.className = 'row dir';
        var arrow = document.createElement('span');
        arrow.className = 'arrow';
        arrow.textContent = row.expanded ? '▾' : '▸';
        el.appendChild(arrow);
        el.onclick = function () { send({ t: 'row_toggle', row: row.row_id }); };
      } else if (row.kind === 2) {
        el.className = 'row file';
        if (row.path) {
          el.onclick = function () {
            send({ t: 'open_file', path: row.path });
            closeSidebar();
          };
        }
      } else if (row.kind === 3) {
        el.className = 'row changes-header';
      } else {
        el.className = 'row change';
        var status = document.createElement('span');
        status.className = 'status';
        status.textContent = row.name.charAt(0);
        name.textContent = row.name.slice(1).trim();
        el.appendChild(status);
      }
      el.appendChild(name);
      tree.appendChild(el);
    });
  }

  // ---- overlay ----

  function showOverlay(text) {
    var el = $('overlay');
    el.textContent = text;
    el.className = '';
  }
  function hideOverlay() { $('overlay').className = 'hidden'; }

  // ---- file viewer ----

  function showFile(msg) {
    var name = (msg.path || '').split('/').pop();
    $('file-name').textContent = name + (msg.truncated ? ' (truncated)' : '');
    $('file-content').textContent =
      msg.error ? '— ' + msg.error + ' —' : msg.content;
    $('file-view').className = '';
  }
  $('file-close').onclick = function () {
    $('file-view').className = 'hidden';
    term.focus();
  };

  // ---- top bar / sidebar ----

  function closeSidebar() {
    $('sidebar').classList.remove('open');
    $('scrim').classList.remove('open');
  }
  $('menu-btn').onclick = function () {
    $('sidebar').classList.toggle('open');
    $('scrim').classList.toggle('open');
  };
  $('scrim').onclick = closeSidebar;
  $('changes-btn').onclick = function () { send({ t: 'toggle_changes' }); };
  $('kbd-btn').onclick = function () { term.focus(); };
  $('resync-btn').onclick = resync;
  $('new-term').onclick = function () {
    if (state) send({ t: 'new_term', session: state.active_session });
  };

  // ---- confirmation bar ----

  var confirmAction = null;

  function askConfirm(text, okLabel, action) {
    $('confirm-text').textContent = text;
    $('confirm-ok').textContent = okLabel;
    confirmAction = action;
    $('confirm').className = '';
  }

  function hideConfirm() {
    confirmAction = null;
    $('confirm').className = 'hidden';
  }

  $('confirm-cancel').onclick = hideConfirm;
  $('confirm-ok').onclick = function () {
    var action = confirmAction;
    hideConfirm();
    if (action) action();
  };

  $('close-term').onclick = function () {
    if (!state) return;
    var session = state.sessions[state.active_session];
    if (!session || session.terms.length <= 1) return;
    var tab = session.active_term;
    var exited = session.exited[tab];
    askConfirm(
      'Close terminal ' + (tab + 1) + '? ' +
        (exited ? 'Its shell has already exited.'
                : 'Its shell (and anything running in it) will be stopped.'),
      'Close',
      function () {
        send({ t: 'close_term', session: state.active_session, tab: tab });
      }
    );
  };

  if (manualFont) term.options.fontSize = manualFont;
  showFontSize();
  onViewportChange();
  connect();
})();
