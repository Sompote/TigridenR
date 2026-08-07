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
    // The host's size seeds auto-fit (a manual choice still wins), then the
    // fit re-runs so a theme change doesn't undo the window fitting.
    if (!manualFont && t.font_size) {
      term.options.fontSize = t.font_size;
      if (currentTerm && !resizable) fitFont(term.cols, term.rows);
      else showFontSize();
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
          if (resizable) fitResize(); else fitFont(msg.cols, msg.rows);
        }
        else if (msg.t === 'exited') {
          if (msg.term === currentTerm) showOverlay('shell exited');
        }
        else if (msg.t === 'file') { showFile(msg); }
        else if (msg.t === 'uploaded') { flash('attached ' + msg.name); }
        else if (msg.t === 'upload_error') { flash('attach failed: ' + msg.msg); }
      } else {
        var bytes = new Uint8Array(ev.data);
        var id = 0;
        // 8-byte LE id; ids stay well below 2^53.
        for (var i = 7; i >= 0; i--) id = id * 256 + bytes[i];
        // The callback keeps the "jump to latest" button in sync when output
        // arrives while the view is scrolled up (no scroll event fires then).
        if (id === currentTerm) term.write(bytes.subarray(8), updateScrollButton);
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

  // ---- selecting and copying ----
  //
  // xterm.js draws its own selection rather than the browser's, so nothing
  // copies it for us: without this, text picked out in the web terminal could
  // not leave the page at all. The desktop gates copy/paste on Cmd alone and
  // lets plain Ctrl+C through as SIGINT — never steal that, or there is no way
  // to interrupt an agent. Non-Mac keyboards get Ctrl+Shift+C/V/A instead.
  var isMac = /Mac|iP(hone|ad|od)/.test(navigator.platform || navigator.userAgent);

  // navigator.clipboard exists only in a secure context. Reached over plain
  // http on a tailnet IP it is undefined, so fall back to execCommand.
  function copyText(text) {
    if (!text) return;
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).catch(function () { legacyCopy(text); });
    } else {
      legacyCopy(text);
    }
  }

  function legacyCopy(text) {
    var ta = document.createElement('textarea');
    ta.value = text;
    // Off-screen but still focusable — execCommand ignores hidden nodes.
    ta.style.cssText = 'position:fixed;top:-1000px;opacity:0';
    document.body.appendChild(ta);
    ta.select();
    try { document.execCommand('copy'); } catch (e) { /* nothing else to try */ }
    document.body.removeChild(ta);
    term.focus();
  }

  function pasteInto(text) {
    if (text && currentTerm) send({ t: 'input', term: currentTerm, data: text });
  }

  function copySelection() {
    if (term.hasSelection()) copyText(term.getSelection());
  }

  // Runs before xterm encodes the key, so returning false keeps it off the PTY.
  term.attachCustomKeyEventHandler(function (ev) {
    if (ev.type !== 'keydown') return true;
    var accel = isMac ? (ev.metaKey && !ev.ctrlKey) : (ev.ctrlKey && ev.shiftKey);
    if (!accel || ev.altKey) return true;
    var key = ev.key.toLowerCase();
    if (key === 'c' && term.hasSelection()) { copySelection(); return false; }
    if (key === 'a') { term.selectAll(); return false; }
    // Cmd+V is left to the browser: xterm's textarea gets a real paste event,
    // which carries the clipboard without needing read permission.
    return true;
  });

  // ---- right-click menu ----

  var ctxmenu = $('ctxmenu');
  var ctxTarget = null;   // 'term' or 'file'

  function closeCtxMenu() { ctxmenu.classList.add('hidden'); ctxTarget = null; }

  function fileSelection() {
    var sel = window.getSelection();
    return sel && !sel.isCollapsed ? sel.toString() : '';
  }

  function openCtxMenu(ev, target) {
    // No preventDefault on the selection itself: the click must not collapse
    // what the menu is about to act on.
    ev.preventDefault();
    ctxTarget = target;
    var hasSel = target === 'term' ? term.hasSelection() : !!fileSelection();
    ctxmenu.querySelector('[data-act="copy"]').disabled = !hasSel;
    // The file viewer is read-only, so Paste does not belong there.
    ctxmenu.querySelector('[data-act="paste"]').classList.toggle('hidden', target !== 'term');
    ctxmenu.classList.remove('hidden');
    // Place it inside the viewport, flipping at the right/bottom edges.
    var r = ctxmenu.getBoundingClientRect();
    var x = Math.min(ev.clientX, window.innerWidth - r.width - 6);
    var y = Math.min(ev.clientY, window.innerHeight - r.height - 6);
    ctxmenu.style.left = Math.max(6, x) + 'px';
    ctxmenu.style.top = Math.max(6, y) + 'px';
  }

  ctxmenu.addEventListener('mousedown', function (ev) { ev.preventDefault(); });
  ctxmenu.addEventListener('click', function (ev) {
    var btn = ev.target.closest('button');
    if (!btn || btn.disabled) return;
    var act = btn.dataset.act, target = ctxTarget;
    closeCtxMenu();
    if (act === 'copy') {
      copyText(target === 'term' ? term.getSelection() : fileSelection());
    } else if (act === 'all') {
      if (target === 'term') { term.selectAll(); term.focus(); }
      else {
        var sel = window.getSelection(), range = document.createRange();
        range.selectNodeContents($('file-content'));
        sel.removeAllRanges();
        sel.addRange(range);
      }
    } else if (act === 'paste') {
      if (navigator.clipboard && navigator.clipboard.readText) {
        navigator.clipboard.readText().then(pasteInto).catch(function () {});
      }
      term.focus();
    }
  });

  $('term-wrap').addEventListener('contextmenu', function (ev) { openCtxMenu(ev, 'term'); });
  $('file-content').addEventListener('contextmenu', function (ev) { openCtxMenu(ev, 'file'); });
  document.addEventListener('mousedown', function (ev) {
    if (!ctxmenu.classList.contains('hidden') && !ctxmenu.contains(ev.target)) closeCtxMenu();
  });
  document.addEventListener('keydown', function (ev) {
    if (ev.key === 'Escape') closeCtxMenu();
  });
  window.addEventListener('blur', closeCtxMenu);

  // ---- scrollback on touch screens ----
  //
  // xterm.js has no touch handling of its own, so on a phone there is no way
  // to reach earlier output. A one-finger vertical swipe scrolls the
  // scrollback; full-screen apps (alternate buffer) own their screen, so
  // swipes are left alone there.
  (function () {
    var wrap = $('term-wrap');
    var start = null;   // {x, y} while a one-finger gesture is undecided
    var lastY = null;   // set once the gesture is committed to vertical
    var acc = 0;
    wrap.addEventListener('touchstart', function (ev) {
      start = ev.touches.length === 1
        ? { x: ev.touches[0].clientX, y: ev.touches[0].clientY }
        : null;
      lastY = null;
      acc = 0;
    }, { passive: true });
    wrap.addEventListener('touchmove', function (ev) {
      if (ev.touches.length !== 1) return;
      if (term.buffer.active.type !== 'normal') return;
      var t = ev.touches[0];
      if (lastY === null) {
        if (!start) return;
        var dx = t.clientX - start.x;
        var dy = t.clientY - start.y;
        if (Math.abs(dx) + Math.abs(dy) < 8) return;
        // A mostly-horizontal swipe stays native: it pans a terminal wider
        // than the screen. Only vertical swipes become scrollback.
        if (Math.abs(dx) > Math.abs(dy)) { start = null; return; }
        lastY = start.y;
      }
      acc += lastY - t.clientY;
      lastY = t.clientY;
      var cell = cellSize().h || 16;
      var lines = Math.trunc(acc / cell);
      if (lines !== 0) {
        acc -= lines * cell;
        term.scrollLines(lines);
      }
      ev.preventDefault(); // keep the page itself from rubber-banding
    }, { passive: false });
    wrap.addEventListener('touchend', function () { start = null; lastY = null; });
  })();

  // A "jump to latest" button appears whenever the view sits above the live
  // edge, on phones and desktops alike.
  function updateScrollButton() {
    var buffer = term.buffer.active;
    var above = buffer.type === 'normal' && buffer.viewportY < buffer.baseY;
    $('scroll-bottom').className = above ? '' : 'hidden';
  }
  term.onScroll(updateScrollButton);
  $('scroll-bottom').onclick = function () { term.scrollToBottom(); };

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

  // Scales the font so the host's grid fills the window. Both axes matter:
  // sizing on width alone would overflow a short window vertically, so the
  // smaller of the two fits wins and the whole grid stays visible.
  function fitFont(cols, rows) {
    if (manualFont) { term.options.fontSize = manualFont; return; }
    var wrap = $('term-wrap');
    var availW = wrap.clientWidth - 12;
    var availH = wrap.clientHeight - 6;
    if (availW <= 0 || !cols) return;
    var cell = cellSize();
    var size = term.options.fontSize;
    var byWidth = size * availW / (cols * cell.w);
    var byHeight = rows && cell.h ? size * availH / (rows * cell.h) : byWidth;
    var ideal = Math.floor(Math.min(byWidth, byHeight));
    term.options.fontSize = Math.max(MIN_FIT, Math.min(MAX_FONT, ideal));
    showFontSize();
  }

  // ---- font size control ----

  var FONT_KEY = 'tigridenr.fontSize';
  var manualFont = parseFloat(localStorage.getItem(FONT_KEY)) || 0; // 0 = auto-fit

  function showFontSize() {
    var el = $('font-size');
    el.textContent = Math.round(term.options.fontSize);
    el.className = manualFont ? '' : 'auto';
    el.title = manualFont
      ? 'Fixed size — tap to auto-fit the window'
      : 'Auto-fitting the window — A− / A+ to fix a size';
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
    if (currentTerm) { if (resizable) fitResize(); else fitFont(term.cols, term.rows); }
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
      if (resizable) fitResize(); else fitFont(term.cols, term.rows);
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

  // Transient note that restores whatever the overlay was showing before.
  var flashTimer = null;
  function flash(text) {
    var el = $('overlay');
    var wasHidden = el.className === 'hidden';
    var previous = el.textContent;
    showOverlay(text);
    clearTimeout(flashTimer);
    flashTimer = setTimeout(function () {
      if (wasHidden) hideOverlay(); else showOverlay(previous);
    }, 2500);
  }

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

  // ---- attaching files to the agent ----
  //
  // A browser never exposes the real path of a dropped file, and it is usually
  // running on a different machine anyway — so the bytes go to the host, which
  // saves them and types *its* path into the terminal, the same thing a
  // desktop drag-and-drop does.

  var UPLOAD_CAP = 25 * 1024 * 1024;

  function sendFile(file) {
    if (!currentTerm) { flash('no terminal to attach to'); return; }
    if (file.size > UPLOAD_CAP) { flash(file.name + ' is over the 25 MB limit'); return; }
    file.arrayBuffer().then(function (buf) {
      var name = new TextEncoder().encode(file.name || 'upload');
      var frame = new Uint8Array(12 + name.length + buf.byteLength);
      var view = new DataView(frame.buffer);
      view.setBigUint64(0, BigInt(currentTerm), true);
      view.setUint32(8, name.length, true);
      frame.set(name, 12);
      frame.set(new Uint8Array(buf), 12 + name.length);
      if (ws && ws.readyState === WebSocket.OPEN) ws.send(frame);
    }).catch(function () { flash('could not read ' + file.name); });
  }

  function sendFiles(files) {
    for (var i = 0; i < files.length; i++) sendFile(files[i]);
  }

  // Drag and drop over the terminal (desktop browsers).
  var dragDepth = 0;
  function hasFiles(ev) {
    var dt = ev.dataTransfer;
    return dt && Array.prototype.indexOf.call(dt.types || [], 'Files') !== -1;
  }
  window.addEventListener('dragenter', function (ev) {
    if (!hasFiles(ev)) return;
    ev.preventDefault();
    dragDepth++;
    $('dropzone').className = '';
  });
  window.addEventListener('dragover', function (ev) {
    if (hasFiles(ev)) { ev.preventDefault(); ev.dataTransfer.dropEffect = 'copy'; }
  });
  window.addEventListener('dragleave', function () {
    // dragleave also fires moving between children, so count enters/leaves.
    if (--dragDepth <= 0) { dragDepth = 0; $('dropzone').className = 'hidden'; }
  });
  window.addEventListener('drop', function (ev) {
    if (!hasFiles(ev)) return;
    ev.preventDefault();
    dragDepth = 0;
    $('dropzone').className = 'hidden';
    sendFiles(ev.dataTransfer.files);
    term.focus();
  });

  // Phones can't drag: the 📎 button opens the photo library / camera / Files.
  $('attach-btn').onclick = function () { $('file-input').click(); };
  $('file-input').onchange = function (ev) {
    sendFiles(ev.target.files);
    ev.target.value = '';   // let the same file be picked twice
    term.focus();
  };

  // Pasting an image (screenshot in the clipboard) attaches it too.
  window.addEventListener('paste', function (ev) {
    var items = (ev.clipboardData && ev.clipboardData.files) || [];
    if (items.length) { ev.preventDefault(); sendFiles(items); }
  });

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
