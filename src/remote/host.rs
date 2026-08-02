//! The seam between server threads and whatever owns the sessions: the Slint
//! app (GUI mode) or the HeadlessManager (--headless). Terminal *input* skips
//! this entirely — it goes straight through the registry.

use crate::app::with_app_id;

#[derive(Debug, Clone)]
pub enum Cmd {
    Resize { term: u64, cols: u16, rows: u16 },
    SelectSession(usize),
    SelectTerm { session: usize, tab: usize },
    NewTerm { session: usize },
    CloseTerm { session: usize, tab: usize },
    Preset(usize),
    RowToggle(i32),
}

pub trait RemoteHost: Send + Sync {
    fn command(&self, cmd: Cmd);
    /// GUI mode: false — the desktop pane owns the grid size and remote
    /// clients mirror it. Headless: true — remote clients drive the PTY size.
    fn allow_remote_resize(&self) -> bool;
}

/// Routes commands onto the UI thread of the primary window, using the same
/// invoke_from_event_loop + with_app_id pattern as the PTY and git threads.
pub struct GuiHost {
    pub app_id: u64,
}

impl RemoteHost for GuiHost {
    fn command(&self, cmd: Cmd) {
        let app_id = self.app_id;
        let _ = slint::invoke_from_event_loop(move || {
            with_app_id(app_id, |app| match cmd {
                // Desktop owns the grid in GUI mode; resize is ignored.
                Cmd::Resize { .. } => {}
                Cmd::SelectSession(idx) => app.set_active(idx),
                Cmd::SelectTerm { session, tab } => {
                    app.set_active(session);
                    app.term_tab_clicked(tab);
                }
                Cmd::NewTerm { session } => {
                    app.set_active(session);
                    app.new_terminal_active();
                }
                Cmd::CloseTerm { session, tab } => {
                    app.set_active(session);
                    app.close_terminal(tab);
                }
                Cmd::Preset(idx) => app.preset_clicked(idx),
                Cmd::RowToggle(row) => app.row_toggled(row),
            });
        });
    }

    fn allow_remote_resize(&self) -> bool {
        false
    }
}
