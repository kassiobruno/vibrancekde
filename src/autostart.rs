//! Início automático com a sessão (XDG autostart, lido pelo KDE).
//!
//! Equivale ao `HKCU\...\Run` + argumento `-minimized` do vibranceGUI.

use crate::assets::{desktop_exec, APP_ID};
use anyhow::{Context, Result};
use std::path::PathBuf;

pub fn path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("autostart")
        .join(format!("{APP_ID}.desktop"))
}

fn entry(exe: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=vibrance-kde\n\
         Comment=Vibrance automático por jogo\n\
         Exec={} --minimized\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         X-KDE-autostart-after=panel\n\
         X-KDE-StartupNotify=false\n\
         X-GNOME-Autostart-enabled=true\n",
        desktop_exec(exe)
    )
}

pub fn is_enabled() -> bool {
    std::fs::read_to_string(path()).is_ok_and(|c| {
        // respeita a chave Hidden/X-KDE-autostart-enabled=false
        !c.lines().any(|l| {
            let l = l.trim();
            l.eq_ignore_ascii_case("Hidden=true")
                || l.eq_ignore_ascii_case("X-KDE-autostart-enabled=false")
        })
    })
}

pub fn set(enabled: bool) -> Result<()> {
    let p = path();
    if enabled {
        let exe = crate::assets::launch_path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("criando {}", dir.display()))?;
        }
        std::fs::write(&p, entry(&exe)).with_context(|| format!("escrevendo {}", p.display()))?;
    } else if p.exists() {
        std::fs::remove_file(&p).with_context(|| format!("removendo {}", p.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_starts_minimized() {
        let e = entry("/opt/vk/vibrance-kde");
        assert!(e.contains("Exec=/opt/vk/vibrance-kde --minimized"));
        assert!(e.contains("X-KDE-autostart-after=panel"));
    }
}
