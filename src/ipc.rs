//! Serviço D-Bus do daemon.
//!
//! * O script do KWin chama `WindowActivated` / `WindowList`.
//! * A GUI usa `Status`, `ListWindows`, `Reload`, `Preview` etc.

use crate::engine::{Event, Status};
use crate::procinfo::WindowInfo;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, watch, Notify};
use zbus::interface;

pub const SERVICE: &str = "io.github.vibrance_kde";
pub const PATH: &str = "/io/github/vibrance_kde";
pub const GUI_SERVICE: &str = "io.github.vibrance_kde.Gui";

pub struct Service {
    pub tx: mpsc::UnboundedSender<Event>,
    pub status: watch::Receiver<Status>,
    pub quit: Arc<Notify>,
    pub open_gui: Arc<Notify>,
}

#[interface(name = "io.github.vibrance_kde.Daemon")]
impl Service {
    /// Chamado pelo script do KWin quando a janela em foco muda.
    fn window_activated(&self, json: &str) {
        let w: WindowInfo = serde_json::from_str(json).unwrap_or_default();
        let _ = self.tx.send(Event::Window(w));
    }

    /// Chamado pelo script do KWin com a lista de janelas abertas.
    fn window_list(&self, json: &str) {
        let list: Vec<WindowInfo> = serde_json::from_str(json).unwrap_or_default();
        let _ = self.tx.send(Event::WindowList(list));
    }

    async fn list_windows(&self) -> String {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Event::ListWindows(tx)).is_err() {
            return "[]".into();
        }
        let list = rx.await.unwrap_or_default();
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
    }

    fn status(&self) -> String {
        serde_json::to_string(&*self.status.borrow()).unwrap_or_else(|_| "{}".into())
    }

    fn reload(&self) {
        let _ = self.tx.send(Event::Reload);
    }

    /// Pré-visualiza um nível (50..=100); valor negativo encerra a pré-visualização.
    fn preview(&self, level: i32) {
        let ev = if level < 0 { None } else { Some(level.clamp(0, 100) as u8) };
        let _ = self.tx.send(Event::Preview(ev));
    }

    fn set_enabled(&self, on: bool) {
        let _ = self.tx.send(Event::SetEnabled(on));
    }

    fn open_gui(&self) {
        self.open_gui.notify_one();
    }

    fn quit(&self) {
        self.quit.notify_one();
    }
}

/// Proxy usado pela GUI.
#[zbus::proxy(
    interface = "io.github.vibrance_kde.Daemon",
    default_service = "io.github.vibrance_kde",
    default_path = "/io/github/vibrance_kde"
)]
pub trait Daemon {
    fn list_windows(&self) -> zbus::Result<String>;
    fn status(&self) -> zbus::Result<String>;
    fn reload(&self) -> zbus::Result<()>;
    fn preview(&self, level: i32) -> zbus::Result<()>;
    fn set_enabled(&self, on: bool) -> zbus::Result<()>;
    fn open_gui(&self) -> zbus::Result<()>;
    fn quit(&self) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_json_matches_kwin_script_shape() {
        // formato produzido por assets/kwin-focus.js
        let json = r#"{"pid":4242,"class":"steam_app_730","name":"cs2","caption":"Counter-Strike 2","output":"DP-1","fullscreen":true}"#;
        let w: WindowInfo = serde_json::from_str(json).unwrap();
        assert_eq!(w.pid, 4242);
        assert_eq!(w.class, "steam_app_730");
        assert_eq!(w.output, "DP-1");
        assert!(w.fullscreen);
        // janela ausente -> objeto vazio -> valores padrão
        let empty: WindowInfo = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, WindowInfo::default());
    }
}
