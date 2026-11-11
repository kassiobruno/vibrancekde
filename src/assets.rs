//! Instala ícones, atalho do menu e script do KWin no perfil do usuário.
//!
//! Tudo é gravado só se o conteúdo mudou, então é seguro chamar a cada início.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub const APP_ID: &str = "vibrance-kde";

pub const ICON_APP: &str = include_str!("../assets/icons/vibrance-kde.svg");
pub const ICON_IDLE: &str = include_str!("../assets/icons/vibrance-kde-symbolic.svg");
pub const ICON_ACTIVE: &str = include_str!("../assets/icons/vibrance-kde-active-symbolic.svg");
pub const ICON_DISABLED: &str = include_str!("../assets/icons/vibrance-kde-disabled-symbolic.svg");
pub const KWIN_SCRIPT: &str = include_str!("../assets/kwin-focus.js");

fn write_if_changed(path: &Path, content: &str) -> Result<bool> {
    if std::fs::read_to_string(path).is_ok_and(|c| c == content) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("criando {}", dir.display()))?;
    }
    std::fs::write(path, content).with_context(|| format!("escrevendo {}", path.display()))?;
    Ok(true)
}

fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn kwin_script_path() -> PathBuf {
    data_dir().join("vibrance-kde").join("kwin-focus.js")
}

fn icons_root() -> PathBuf {
    data_dir().join("icons").join("hicolor").join("scalable")
}

pub fn desktop_file_path() -> PathBuf {
    data_dir().join("applications").join(format!("{APP_ID}.desktop"))
}

/// Escolhe o caminho que deve ser gravado em atalhos e no autostart.
///
/// Dentro de um AppImage o executável real fica num ponto de montagem temporário
/// (`/tmp/.mount_xxx`) que some quando o programa fecha; o arquivo `.AppImage`
/// é indicado pela variável `APPIMAGE`.
fn pick_launch_path(appimage: Option<String>, exe: Option<String>) -> String {
    appimage
        .filter(|p| !p.is_empty())
        .or(exe)
        .unwrap_or_else(|| "vibrance-kde".to_string())
}

/// Caminho para relançar este programa (veja [`pick_launch_path`]).
pub fn launch_path() -> String {
    pick_launch_path(
        std::env::var("APPIMAGE").ok(),
        std::env::current_exe().ok().and_then(|p| p.to_str().map(str::to_owned)),
    )
}

/// Escapa o caminho para a chave `Exec=` de um arquivo .desktop.
pub fn desktop_exec(exe: &str) -> String {
    if exe.chars().any(|c| c.is_whitespace() || "\"'\\><~|&;$*?#()`".contains(c)) {
        let escaped: String = exe
            .chars()
            .flat_map(|c| match c {
                '"' | '`' | '$' | '\\' => vec!['\\', c],
                _ => vec![c],
            })
            .collect();
        format!("\"{escaped}\"")
    } else {
        exe.to_string()
    }
}

fn desktop_entry() -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=vibrance-kde\n\
         GenericName=Digital Vibrance\n\
         Comment=Aumenta a saturação das cores automaticamente quando um jogo está em foco\n\
         Exec={}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=Utility;Settings;\n\
         Keywords=vibrance;saturation;saturação;cores;jogos;\n\
         StartupWMClass={APP_ID}\n",
        desktop_exec(&launch_path())
    )
}

/// Instala ícones, entrada de menu e script do KWin. Devolve `true` se os ícones
/// da bandeja ficaram disponíveis no tema.
pub fn ensure_installed() -> Result<bool> {
    let root = icons_root();
    let mut icons_ok = true;
    for (rel, content) in [
        (format!("apps/{APP_ID}.svg"), ICON_APP),
        (format!("status/{APP_ID}-symbolic.svg"), ICON_IDLE),
        (format!("status/{APP_ID}-active-symbolic.svg"), ICON_ACTIVE),
        (format!("status/{APP_ID}-disabled-symbolic.svg"), ICON_DISABLED),
    ] {
        if let Err(e) = write_if_changed(&root.join(rel), content) {
            log::warn!("não foi possível instalar ícone: {e:#}");
            icons_ok = false;
        }
    }
    if let Err(e) = write_if_changed(&desktop_file_path(), &desktop_entry()) {
        log::warn!("não foi possível criar o atalho do menu: {e:#}");
    }
    write_if_changed(&kwin_script_path(), KWIN_SCRIPT)?;
    Ok(icons_ok)
}

/// Remove o que `ensure_installed` criou (usado pelo `uninstall`).
pub fn remove_installed() {
    let root = icons_root();
    for rel in [
        format!("apps/{APP_ID}.svg"),
        format!("status/{APP_ID}-symbolic.svg"),
        format!("status/{APP_ID}-active-symbolic.svg"),
        format!("status/{APP_ID}-disabled-symbolic.svg"),
    ] {
        let _ = std::fs::remove_file(root.join(rel));
    }
    let _ = std::fs::remove_file(desktop_file_path());
    let _ = std::fs::remove_file(kwin_script_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_quoting() {
        assert_eq!(desktop_exec("/usr/bin/vibrance-kde"), "/usr/bin/vibrance-kde");
        assert_eq!(desktop_exec("/home/a b/vk"), "\"/home/a b/vk\"");
    }

    #[test]
    fn appimage_path_wins_over_mount_point() {
        let mounted = Some("/tmp/.mount_abc/usr/bin/vibrance-kde".to_string());
        assert_eq!(
            pick_launch_path(Some("/home/u/vibrance-kde.AppImage".into()), mounted.clone()),
            "/home/u/vibrance-kde.AppImage"
        );
        assert_eq!(pick_launch_path(None, mounted.clone()), "/tmp/.mount_abc/usr/bin/vibrance-kde");
        assert_eq!(pick_launch_path(Some(String::new()), mounted.clone()), "/tmp/.mount_abc/usr/bin/vibrance-kde");
        assert_eq!(pick_launch_path(None, None), "vibrance-kde");
    }

    #[test]
    fn embedded_assets_are_valid() {
        assert!(ICON_APP.contains("<svg"));
        for s in [ICON_IDLE, ICON_ACTIVE, ICON_DISABLED] {
            assert!(s.contains("ColorScheme-Text") && s.contains("currentColor"));
        }
        assert!(KWIN_SCRIPT.contains("windowActivated"));
    }
}
