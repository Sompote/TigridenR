mod app;
mod config;
mod editor;
mod git;
mod paint;
#[cfg(feature = "remote")]
mod remote;
mod session;
mod term;
mod theme;
mod tree;
mod viewer;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
use slint::ComponentHandle;

use app::{with_app_id, App};
use term::keys::Mods;

slint::include_modules!();

fn mods(ctrl: bool, alt: bool, meta: bool, shift: bool) -> Mods {
    // Slint follows the Qt convention on macOS: its `control` modifier is the
    // ⌘ Command key and `meta` is the physical Ctrl key. Terminal semantics
    // need the physical keys, so swap them back.
    #[cfg(target_os = "macos")]
    let (ctrl, meta) = (meta, ctrl);
    Mods { ctrl, alt, meta, shift }
}

struct WindowOpts {
    /// Team index into `config.teams`; None = the default flat preset list.
    team: Option<usize>,
    initial_folders: Vec<PathBuf>,
    is_primary: bool,
    split_ratio: Option<f32>,
    restore_active: usize,
}

fn open_window(config: config::Config, recents: Vec<PathBuf>, opts: WindowOpts) {
    let ui = MainWindow::new().expect("failed to create window");
    if let Some(ratio) = opts.split_ratio {
        ui.set_split_ratio(ratio.clamp(0.15, 0.85));
    }

    #[cfg(feature = "remote")]
    let auto_remote = opts.is_primary && config.remote.enabled;
    let app = Rc::new(RefCell::new(App::new(
        &ui,
        config.clone(),
        recents,
        opts.team,
        opts.is_primary,
    )));
    let app_id = app.borrow().id;
    app::register(app_id, app, ui.clone_strong());
    with_app_id(app_id, |app| app.update_recents_model());

    wire_callbacks(&ui, app_id);

    // Open initial folders (silently dropping folders that vanished).
    for folder in &opts.initial_folders {
        if folder.is_dir() {
            with_app_id(app_id, |app| app.add_session(folder.clone(), false));
        }
    }
    with_app_id(app_id, |app| app.set_active(opts.restore_active));

    // [remote] enabled = true in config.toml: bring the server up on launch.
    #[cfg(feature = "remote")]
    if auto_remote {
        with_app_id(app_id, |app| app.remote_enable(false));
    }

    ui.show().expect("failed to show window");
}

fn wire_callbacks(ui: &MainWindow, app_id: u64) {
    ui.on_add_folder(move || {
        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
            with_app_id(app_id, |app| app.add_session(folder, true));
        }
    });
    // New Window ▸ team — a plain closure, not inside with_app_id, because
    // open_window registers a new app in the same registry.
    ui.on_new_window(move |team_idx| {
        let Some(folder) = rfd::FileDialog::new().pick_folder() else { return };
        let team = (team_idx >= 0).then_some(team_idx as usize);
        open_window(
            app::config(),
            config::load_state().recent_folders,
            WindowOpts {
                team,
                initial_folders: vec![folder],
                is_primary: false,
                split_ratio: None,
                restore_active: 0,
            },
        );
    });
    ui.on_recent_clicked(move |i| with_app_id(app_id, |app| app.recent_clicked(i as usize)));
    ui.on_recent_forget(move |i| with_app_id(app_id, |app| app.forget_recent(i as usize)));
    ui.on_row_clicked(move |id| with_app_id(app_id, |app| app.row_clicked(id)));
    ui.on_row_toggled(move |id| with_app_id(app_id, |app| app.row_toggled(id)));
    ui.on_close_session(move |idx| with_app_id(app_id, |app| app.close_session(idx as usize)));
    ui.on_preset_clicked(move |idx| with_app_id(app_id, |app| app.preset_clicked(idx as usize)));
    ui.on_term_tab_clicked(move |tab| with_app_id(app_id, |app| app.term_tab_clicked(tab as usize)));
    ui.on_new_terminal(move || with_app_id(app_id, |app| app.new_terminal_active()));
    ui.on_close_terminal(move |tab| with_app_id(app_id, |app| app.close_terminal(tab as usize)));
    ui.on_split_changed(move || with_app_id(app_id, |app| app.split_changed()));
    ui.on_menu_save(move || with_app_id(app_id, |app| app.save_editor()));
    ui.on_menu_copy(move || with_app_id(app_id, |app| app.menu_copy()));
    ui.on_menu_paste(move || with_app_id(app_id, |app| app.menu_paste()));
    ui.on_menu_select_all(move || with_app_id(app_id, |app| app.menu_select_all()));
    ui.on_menu_close_terminal(move || with_app_id(app_id, |app| app.menu_close_terminal()));
    ui.on_menu_close_session(move || with_app_id(app_id, |app| app.menu_close_session()));
    ui.on_tree_context(move |action, id| with_app_id(app_id, |app| app.tree_context(action, id)));
    ui.on_name_dialog_accept(move |name| {
        with_app_id(app_id, |app| app.name_dialog_accept(name.to_string()))
    });
    ui.on_name_dialog_cancel(move || with_app_id(app_id, |app| app.name_dialog_cancel()));
    ui.on_settings_open(move || with_app_id(app_id, |app| app.open_settings()));
    ui.on_settings_close(move || with_app_id(app_id, |app| app.close_settings()));
    // Appearance keys reach every window through app::settings_changed, which
    // borrows each one — so they must NOT run inside with_app_id, which is
    // already holding a mutable borrow of this window. Only the remote keys,
    // which touch this window alone, go through with_app_id.
    ui.on_settings_changed(move |key, value| {
        if key.starts_with("remote") {
            #[cfg(feature = "remote")]
            with_app_id(app_id, |app| app.remote_setting(&key, &value));
        } else {
            app::settings_changed(&key, &value);
        }
    });
    ui.on_settings_reset(app::settings_reset);
    ui.on_settings_reveal_config(app::reveal_config);
    #[cfg(feature = "framedump")]
    ui.on_settings_jumped(|y| eprintln!("SETTINGS jumped to remote section at y={y}"));
    ui.on_toggle_view(move || with_app_id(app_id, |app| app.toggle_view()));
    ui.on_toggle_changes(move || with_app_id(app_id, |app| app.toggle_changes()));
    ui.on_banner_primary(move || with_app_id(app_id, |app| app.banner_primary()));
    ui.on_banner_secondary(move || with_app_id(app_id, |app| app.banner_secondary()));
    #[cfg(feature = "remote")]
    {
        ui.on_menu_remote(move || with_app_id(app_id, |app| app.menu_remote()));
    }

    ui.on_term_key(move |text, ctrl, alt, meta, shift| {
        let mut handled = false;
        with_app_id(app_id, |app| handled = app.term_key(&text, mods(ctrl, alt, meta, shift)));
        handled
    });
    ui.on_term_wheel(move |delta| with_app_id(app_id, |app| app.term_wheel(delta)));
    ui.on_term_mouse(move |kind, x, y| with_app_id(app_id, |app| app.term_mouse(kind, x, y)));
    ui.on_term_size_changed(move |w, h| with_app_id(app_id, |app| app.term_resized(w, h)));

    ui.on_editor_key(move |text, ctrl, alt, meta, shift| {
        let mut handled = false;
        with_app_id(app_id, |app| handled = app.editor_key(&text, mods(ctrl, alt, meta, shift)));
        handled
    });
    ui.on_editor_mouse(move |kind, x, y| with_app_id(app_id, |app| app.editor_mouse(kind, x, y)));
    ui.on_editor_wheel(move |delta| with_app_id(app_id, |app| app.editor_wheel(delta)));
    ui.on_editor_size_changed(move |w, h| with_app_id(app_id, |app| app.editor_resized(w, h)));

    // External file drops arrive as winit events the Slint DropArea never
    // sees; forward them to the active terminal as a typed path.
    ui.window().on_winit_window_event(move |_, event| match event {
        winit::event::WindowEvent::DroppedFile(path) => {
            let path = path.clone();
            with_app_id(app_id, move |app| app.file_dropped(path));
            EventResult::PreventDefault
        }
        winit::event::WindowEvent::HoveredFile(_) => {
            with_app_id(app_id, |app| app.file_drop_hover(true));
            EventResult::PreventDefault
        }
        winit::event::WindowEvent::HoveredFileCancelled => {
            with_app_id(app_id, |app| app.file_drop_hover(false));
            EventResult::PreventDefault
        }
        _ => EventResult::Propagate,
    });

    // Kill this window's PTYs now, then drop its registry entry outside the
    // callback (dropping the window inside its own handler is unsound); quit
    // once the last window is gone.
    ui.window().on_close_requested(move || {
        with_app_id(app_id, |app| app.shutdown());
        let _ = slint::invoke_from_event_loop(move || {
            if app::remove_window(app_id) == 0 {
                let _ = slint::quit_event_loop();
            }
        });
        slint::CloseRequestResponse::HideWindow
    });
}

fn main() {
    config::migrate_legacy_dir();
    let (mut config, malformed_config) = config::load_config();
    let state = config::load_state();

    if malformed_config {
        eprintln!("tigridenr: config.toml is malformed; using defaults (file left untouched)");
    }

    // Hand-rolled flags: --headless [--port N] [FOLDER...] runs the remote
    // server with no window (and no Slint/winit init, so it works over SSH);
    // --no-remote forces the server off for this GUI launch; --port overrides
    // the config. Bare arguments are folders to open.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut headless = false;
    let mut no_remote = false;
    let mut folders: Vec<PathBuf> = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--headless" => headless = true,
            "--no-remote" => no_remote = true,
            "--port" => {
                if let Some(port) = iter.next().and_then(|v| v.parse::<u16>().ok()) {
                    config.remote.port = port;
                }
            }
            "-h" | "--help" => {
                println!(
                    "TigridenR {}\n\n\
                     USAGE:\n    tigridenr [OPTIONS] [FOLDER...]\n\n\
                     OPTIONS:\n\
                     \x20   --headless      Serve the web UI with no window (works over SSH)\n\
                     \x20   --port <PORT>   Port to serve on (default from config.toml)\n\
                     \x20   --no-remote     Start the GUI with remote access off\n\
                     \x20   -h, --help      Show this help\n\n\
                     FOLDER...  Folders to open. Without any, the folders from the\n\
                     \x20          last session are restored.\n",
                    env!("CARGO_PKG_VERSION")
                );
                return;
            }
            other if other.starts_with('-') => {
                eprintln!("tigridenr: unknown option {other} (try --help)");
                std::process::exit(2);
            }
            path => folders.push(PathBuf::from(path)),
        }
    }
    if no_remote {
        config.remote.enabled = false;
    }
    // Explicit folders win over the restored session.
    let state_folders = if folders.is_empty() { state.folders.clone() } else { folders.clone() };

    if headless {
        #[cfg(feature = "remote")]
        {
            let port = config.remote.port;
            remote::headless::run(config, port, state_folders);
        }
        #[cfg(not(feature = "remote"))]
        {
            eprintln!("tigridenr: this build has no remote support (built without the `remote` feature)");
            std::process::exit(2);
        }
    }
    app::set_config(config.clone());

    open_window(
        config,
        state.recent_folders.clone(),
        WindowOpts {
            team: None,
            initial_folders: state_folders.clone(),
            is_primary: true,
            split_ratio: state.split_ratio,
            restore_active: state.active,
        },
    );

    slint::run_event_loop().expect("event loop failed");
    // Covers macOS ⌘Q, which quits the loop without a per-window close_requested.
    app::shutdown_all();
}
