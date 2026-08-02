/* TigridenR web client: mirrors the native chrome and attaches xterm.js to
 * the live desktop terminals over one websocket. */
(function () {
  'use strict';

  // Terminal palette — must match src/term/colors.rs DARK.
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

  // Host owns the grid: pick the largest font that fits the host's columns.
  function fitFont(cols) {
    var wrap = $('term-wrap');
    var avail = wrap.clientWidth - 10;
    if (avail <= 0 || !cols) return;
    var cell = cellSize();
    var ideal = Math.floor(term.options.fontSize * avail / (cols * cell.w));
    term.options.fontSize = Math.max(7, Math.min(18, ideal));
  }

  // Headless: the client drives the PTY size from its own viewport.
  function fitResize() {
    var wrap = $('term-wrap');
    var cell = cellSize();
    var cols = Math.max(2, Math.floor((wrap.clientWidth - 10) / cell.w));
    var rows = Math.max(1, Math.floor(wrap.clientHeight / cell.h));
    if (cols !== term.cols || rows !== term.rows) {
      term.resize(cols, rows);
      send({ t: 'resize', term: currentTerm, cols: cols, rows: rows });
    }
  }

  var resizeTimer = null;
  function onViewportChange() {
    // Keep the app above the iOS soft keyboard.
    if (window.visualViewport) {
      document.getElementById('app').style.height = window.visualViewport.height + 'px';
      window.scrollTo(0, 0);
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

  function renderTabs(session) {
    var tabs = $('tabs');
    tabs.textContent = '';
    if (!session) return;
    session.terms.forEach(function (id, i) {
      var tab = document.createElement('button');
      tab.className = 'tab' + (i === session.active_term ? ' active' : '');
      var label = document.createElement('span');
      label.textContent = String(i + 1);
      tab.appendChild(label);
      if (session.terms.length > 1) {
        var close = document.createElement('span');
        close.className = 'close';
        close.textContent = '✕';
        close.onclick = function (ev) {
          ev.stopPropagation();
          send({ t: 'close_term', session: state.active_session, tab: i });
        };
        tab.appendChild(close);
      }
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
  $('kbd-btn').onclick = function () { term.focus(); };
  $('resync-btn').onclick = resync;
  $('new-term').onclick = function () {
    if (state) send({ t: 'new_term', session: state.active_session });
  };

  onViewportChange();
  connect();
})();
