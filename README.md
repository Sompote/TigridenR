# TigridenR — the tiny agentic workbench you can run from anywhere

![Version](https://img.shields.io/badge/version-0.1.0-e8912d) ![License](https://img.shields.io/badge/license-MIT-blue) ![Platform](https://img.shields.io/badge/platform-macOS-lightgrey)

**A tiny workbench for supervising AI coding agents — on your desk, and in your pocket.**

Run `claude`, `codex`, `gemini` — any terminal agent — each in its own folder, side by side. Every workspace gets an embedded terminal, a live file tree, a lightweight editor, and change tracking with one-click rollback. Then leave the desk: **TigridenR mirrors the whole workbench to a web page**, so you can watch an agent think, answer its questions, and kick off the next task from your phone on the couch — or from anywhere on your [Tailscale](https://tailscale.com) network.

No run/debug tooling, no chat panel, no LSP. The agents do the heavy lifting; TigridenR gives you eyes and hands — local or remote.

Written in pure Rust. **~13 MB binary, ~40 MB RAM.**

![TigridenR supervising an agent: the viewer shows a chart the agent produced while the agent CLI runs in one of three terminal tabs below](assets/screenshot.png)

*Above: a real session — the agent's workspace file tree on the left, the built-in viewer inspecting a chart the agent just generated, and the agent CLI running in one of three terminal tabs below.*

## Why

Agentic coding means running several agents in several folders and *checking in on them* — and the checking-in doesn't stop when you stand up. A full IDE is overkill; a bare terminal multiplexer has no file browser, no editor, no diff view, and no way to peek from your phone. TigridenR is the minimal middle:

- **One session per folder** — agent, files, editor, and change tracking together.
- **One URL for the whole thing** — the same terminals, sidebar, and agent buttons in any browser, secured by your tailnet.

## Remote: run your agents from the web

The desktop app and every browser client share the **same live shells** — it's a tmux-style attach, not a screen-share. Output mirrors everywhere in real time; anyone attached can type. Start Claude Code at your desk, then approve its plan from your phone in the kitchen; the desktop window shows every keystroke.

**What the web page gives you** (same dark theme, same orange accent):

- The **terminal** (xterm.js) with full TUI support — attaching mid-`vim` or mid-agent-session replays the current screen (plus up to 2,000 lines of recent scrollback) straight from the terminal grid, so you never land on a blank page.
- The **agent sidebar** — your folders and their browseable file trees (tap to expand/collapse), plus the live **Changes (N)** list of what the agent touched. Read-only by design: inspecting and steering happens in the terminal; there is no web editor.
- **Terminal tabs and preset buttons** — switch folders and shells, open or close terminals, and launch `claude`/`codex`/`gemini` with one tap.
- A **phone-friendly layout** — drawer sidebar, soft-keyboard button, a ⟳ resync button, and font auto-fit to the host's grid width.

With several desktop windows open, the web mirrors the primary (first) window; other windows keep working locally but aren't published.

**Turn it on** (one of):

1. **In the app:** **File ▸ Remote Access… ▸ Enable.** The dialog shows the URL to open.
2. **In config** (`~/Library/Application Support/tigridenr/config.toml`):

   ```toml
   [remote]
   enabled = true
   port = 8620
   ```

3. **Headless** — no window, no display needed (a Mac mini in a closet, over SSH):

   ```sh
   tigridenr --headless             # serves the folders from your last session
   tigridenr --headless --port 9000
   ```

   In headless mode the *browser* drives the terminal size — rotate your phone and the PTY follows. When the desktop GUI is running, its pane owns the grid and browsers mirror it.

**Security = Tailscale, by design.** The server never binds anything but `127.0.0.1`. Reachability comes exclusively from `tailscale serve`, which publishes it at `https://<machine>.<tailnet>.ts.net` with a real TLS certificate, admitting only devices logged into your tailnet (install the Tailscale app on your phone, same account, done). Disabling remote access tears the serve config down and stops the server. There is no separate password — access control *is* your tailnet, and anyone who can open the page has full shell access as your user, so treat tailnet membership accordingly. No Tailscale? The server still runs, but only reachable on the machine itself (`http://127.0.0.1:<port>`).

Everything the page needs (HTML/CSS/JS and a vendored [xterm.js](https://github.com/xtermjs/xterm.js)) is embedded in the binary — no CDN, works on a tailnet with no internet at all. Prefer a local-only build? `cargo build --release --no-default-features` compiles the whole feature out.

## Features (the local half)

- **One-click agents** — preset buttons type the agent command into the terminal for you (fully configurable).
- **Multiple terminals per folder** — the `+` tab spawns extra shells in the same workspace, so one agent can run while you use a second terminal for git, tests, or another agent.
- **Real terminal** — VTE-compliant emulation ([alacritty_terminal](https://crates.io/crates/alacritty_terminal) + a real PTY). TUIs like `vim`, `top`, and the Claude Code interface just work, including bracketed paste and truecolor. Select with the mouse and Cmd+C to copy out; Cmd+V pastes text in, and image paste into Claude Code works with Ctrl+V (the agent reads your clipboard directly).
- **Live file tree** — gitignore-aware, refreshes automatically as agents create and delete files. Right-click any entry for New File/Folder, Reveal in Finder, Open in Default App, Copy (Relative) Path, Duplicate, Rename, and Move to Trash.
- **File change tracking & rollback** — **File ▸ Show Changes Panel** lists every file the agent modified/added/deleted since the baseline, updated within ~1 s. Click a row for a syntax-highlighted diff; discard one file or the whole run, always behind a confirmation. Git folders compare against the last commit; folders **without git get invisible shadow snapshots** (stored in the app's data dir — your folder stays untouched, the agent never sees them).
- **Multiple windows & agent teams** — **File ▸ New Window** opens an independent window with its own folders. Define named preset groups (`[[teams]]` in config.toml) to give different windows different agent buttons.
- **Drag & drop files** — drop any file from Finder and its (shell-quoted) path is typed into the terminal — attach files to an agent prompt like in a native terminal.
- **Built-in editor** — syntax highlighting for 40+ languages ([cosmic-text](https://crates.io/crates/cosmic-text) + syntect), Cmd+S save, auto-reload when the agent edits the open file.
- **File viewers** — images, rendered Markdown, CSV/TSV tables, PDF text extraction.
- **Per-folder sessions, recent folders, native menu bar, persistent layout** — and small on purpose: no webview, no Electron, no C regex libraries.

## Quick install (macOS)

No Rust needed — grab the prebuilt app from the [latest release](https://github.com/Sompote/TigridenR/releases/latest):

1. Download **`TigridenR-0.1.0-macos-universal.app.zip`** (one download for both Apple Silicon and Intel).
2. Unzip and drag **TigridenR.app** into **/Applications**.
3. First launch only: the app isn't notarized, so **right-click → Open → Open**, or run:

   ```sh
   xattr -d com.apple.quarantine /Applications/TigridenR.app
   ```

Prefer a bare binary? The release also ships `tigridenr-0.1.0-macos-arm64.tar.gz` (Apple Silicon) and `tigridenr-0.1.0-macos-x86_64.tar.gz` (Intel) — untar and run `./tigridenr`.

<details>
<summary><b>Build from source</b> (stable Rust required)</summary>

```sh
git clone https://github.com/Sompote/TigridenR.git
cd TigridenR
cargo build --release
./target/release/tigridenr        # or ./bundle/make-app.sh for dist/TigridenR.app
```

Local-only build (remote access compiled out): `cargo build --release --no-default-features`.

macOS is the primary target; Linux/Windows are untested but the stack is cross-platform.
</details>

## Usage

1. Click **+ Add folder** and pick a project directory — a login shell opens there.
2. Click a preset button (e.g. **claude**) to launch the agent, or type any command.
3. Watch the file tree update as the agent works; click any file to inspect or tweak it.
4. Click **+** in the terminal tab strip for more shells in the same folder; add more folders for more agents in parallel.
5. **File ▸ Remote Access… ▸ Enable**, open the URL on your phone, and walk away — the session comes with you.

### Track & roll back what the agent changes

1. **File ▸ Show Changes Panel** — a **Changes (N)** section appears under each folder (off by default, zero overhead while off).
2. Git folders diff against the last commit; non-git folders get a shadow snapshot baseline the moment you enable the panel (under `~/Library/Application Support/tigridenr/snapshots/`; no `.git` appears in your project).
3. Changed files appear within ~1 s as `M` / `A` / `D` rows. **Click a row** for the accumulated diff; the **File** chip jumps to the editable file.
4. **Right-click ▸ Discard Changes…** reverts one file; the **↺** button reverts the whole run. Both confirm first.

The Changes list is mirrored to remote clients too — you can see what the agent touched from your phone before telling it to continue. (In GUI mode it appears remotely once the desktop panel is enabled; in `--headless` mode change tracking is always on.) Viewing diffs and discarding changes stay desktop-only for now.

### Keys

| Context  | Keys |
|----------|------|
| Terminal | everything a terminal expects: Ctrl+C/Z/D/R…, arrows, F1–F12, TUIs; drag to select (double-click = word), Cmd+C copies, Cmd+V pastes (bracketed); wheel scrolls history |
| Editor   | typing, arrows / Home / End / PgUp / PgDn (+Shift selects, +Alt jumps words), Cmd+A / C / X / V, Cmd+S saves |

## Configuration

`~/Library/Application Support/tigridenr/config.toml`, created on first run:

```toml
theme = "dark"          # "dark" | "light"
font_family = "Menlo"
font_size = 13.0
scrollback = 10000

[[presets]]
label = "claude"
command = "claude"
send_enter = true

[[presets]]
label = "codex"
command = "codex"

[[presets]]
label = "gemini"
command = "gemini"

# Optional: named preset groups for File ▸ New Window ▸ <team>.
[[teams]]
name = "reviewers"
[[teams.presets]]
label = "claude-review"
command = "claude /review"
send_enter = true

# Remote web access (see "Remote" above).
[remote]
enabled = false
port = 8620
```

Runtime state (restored folders, split position) lives next to it in `state.toml`; shadow snapshots live in `snapshots/`. CLI flags: `--headless`, `--port N`, `--no-remote`. Note: toggling remote access from the app re-writes `config.toml`, so hand-added comments are lost.

## Architecture

Slint provides only the chrome (sidebar, layout, splitter). The two hard parts are custom-rendered pixel panes on the CPU:

| Pane | Engine | Rendering |
|------|--------|-----------|
| Terminal | headless `alacritty_terminal` grid fed by `portable-pty` | glyphs rasterized per cell via cosmic-text's swash cache |
| Editor | cosmic-text `SyntaxEditor` (syntect highlighting) | draws itself into the same pixel-buffer canvas |

Only the PTY reader threads run in the background; rendering and editing happen on the UI thread with coalesced repaints.

The remote layer (`src/remote/`, default-on `remote` feature) is a loopback-only HTTP/WebSocket server with no async runtime — plain threads, matching the rest of the app. The PTY reader thread tees raw output to attached browser clients; a newly attached client gets the current screen serialized back to ANSI straight from the alacritty grid (recent scrollback, colors, cursor, and terminal modes included), which is why attaching mid-TUI just works. Browser keystrokes feed the same PTY writer channel the desktop uses. Sidebar state (sessions, tabs, tree, changes) streams as JSON snapshots; terminal bytes go as binary frames. In `--headless` mode a minimal session manager replaces the GUI entirely — same terminals, tree, and change tracking, no Slint or window server involved.

`vendor/cosmic-text/` is a verbatim copy of cosmic-text 0.19 with one change: its syntect dependency uses the pure-Rust `fancy-regex` engine instead of the oniguruma C library (smaller binary, no C build dependency).

### Debug builds

`cargo build --features framedump`, then run with `TIGRIDENR_DUMP=/tmp/frames` to dump both panes as PNGs. `TIGRIDENR_TEST_INPUT='claude\r'` and `TIGRIDENR_TEST_OPEN=path` script the first session for headless testing.

## Roadmap / known limitations (v1)

- [ ] Editor undo
- [ ] Mouse reporting to TUIs (desktop; the web terminal already forwards what TUIs request)
- [ ] IME / dead-key composition
- [ ] Editor tabs (currently one open file per session)
- [ ] PDF page rendering (currently text extraction)
- [ ] Linux / Windows testing
- [ ] Web: editor pane, diff viewer, and discard actions (currently terminal + read-only sidebar)
- [ ] Web: adding/removing folders remotely (folders come from the desktop or the last saved session)
- [ ] Web: mirror a window other than the primary one

## License

MIT
