//! Ícone da bandeja do sistema (StatusNotifierItem) com menu de contexto.

use crate::assets::APP_ID;
use crate::engine::Status;
use ksni::menu::{CheckmarkItem, StandardItem};
use ksni::{Category, MenuItem, ToolTip, Tray};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Copy)]
pub enum TrayAction {
    OpenGui,
    SetEnabled(bool),
    SetAutostart(bool),
    Quit,
}

pub struct VibranceTray {
    pub status: Status,
    pub autostart: bool,
    /// Os ícones simbólicos foram instalados no tema.
    pub icons_ok: bool,
    pub tx: UnboundedSender<TrayAction>,
}

impl VibranceTray {
    fn summary(&self) -> String {
        let s = &self.status;
        if !s.enabled {
            "Desativado".to_string()
        } else if let Some(app) = &s.active_app {
            format!("{app}: vibrance {}%", s.active_level)
        } else {
            format!("Área de trabalho: {}%", s.windows_level)
        }
    }
}

impl Tray for VibranceTray {
    fn id(&self) -> String {
        APP_ID.into()
    }

    fn category(&self) -> Category {
        Category::ApplicationStatus
    }

    fn title(&self) -> String {
        "vibrance-kde".into()
    }

    fn icon_name(&self) -> String {
        if !self.icons_ok {
            return "preferences-desktop-color".into();
        }
        let s = &self.status;
        let name = if !s.enabled {
            "disabled"
        } else if s.active_app.is_some() {
            "active"
        } else {
            return format!("{APP_ID}-symbolic");
        };
        format!("{APP_ID}-{name}-symbolic")
    }

    fn tool_tip(&self) -> ToolTip {
        let mut description = self.summary();
        if !self.status.kwin_connected {
            description.push_str("\nKWin: script de foco não carregado");
        }
        if let Some(e) = &self.status.error {
            description.push_str(&format!("\nErro: {e}"));
        }
        ToolTip {
            title: "vibrance-kde".into(),
            description,
            icon_name: self.icon_name(),
            icon_pixmap: Vec::new(),
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(TrayAction::OpenGui);
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(TrayAction::SetEnabled(!self.status.enabled));
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: self.summary(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Abrir configurações".into(),
                icon_name: "configure".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(TrayAction::OpenGui);
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Ativar vibrance automático".into(),
                checked: self.status.enabled,
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(TrayAction::SetEnabled(!t.status.enabled));
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Iniciar com o sistema".into(),
                checked: self.autostart,
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(TrayAction::SetAutostart(!t.autostart));
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Sair".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(TrayAction::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}
