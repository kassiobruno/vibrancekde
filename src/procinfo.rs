//! Descoberta do "nome do processo" de uma janela.
//!
//! O vibranceGUI usa `Process.ProcessName`. No Linux a janela só informa o PID e
//! a classe; jogos Windows rodando em Wine/Proton aparecem como `wine64-preloader`,
//! então também olhamos `comm`, a linha de comando e a classe da janela.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowInfo {
    pub pid: i64,
    /// `resourceClass` (ex.: `steam_app_730`).
    pub class: String,
    /// `resourceName`.
    pub name: String,
    pub caption: String,
    /// Nome do monitor onde a janela está (ex.: `DP-1`).
    pub output: String,
    pub fullscreen: bool,
}

/// Executáveis que só servem de "casca" para outro programa.
fn is_wrapper(name: &str) -> bool {
    let n = name.to_lowercase();
    n.starts_with("wine")
        || n.contains("preloader")
        || n.contains("proton")
        || n.contains("pressure-vessel")
        || n == "reaper"
        || n == "steam"
        || n == "python3"
        || n == "python"
}

fn basename(s: &str) -> &str {
    s.rsplit(['/', '\\']).next().unwrap_or(s)
}

fn read_trim(p: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Nomes de processo plausíveis para a janela, do mais ao menos específico.
pub fn candidates(w: &WindowInfo) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        let s = s.trim();
        if !s.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(s)) {
            out.push(s.to_string());
        }
    };
    if w.pid > 0 {
        let base = format!("/proc/{}", w.pid);
        if let Ok(exe) = std::fs::read_link(format!("{base}/exe")) {
            if let Some(n) = exe.file_name().and_then(|n| n.to_str()) {
                push(n);
            }
        }
        if let Some(comm) = read_trim(format!("{base}/comm")) {
            push(&comm);
        }
        if let Ok(raw) = std::fs::read(format!("{base}/cmdline")) {
            for arg in raw.split(|&b| b == 0).take(8) {
                if let Ok(a) = std::str::from_utf8(arg) {
                    if !a.starts_with('-') {
                        push(basename(a));
                    }
                }
            }
        }
    }
    push(&w.class);
    push(&w.name);
    out
}

/// Melhor nome para sugerir ao usuário ao adicionar uma janela.
pub fn suggested_process(w: &WindowInfo) -> String {
    let c = candidates(w);
    // 1) um `.exe` explícito (Wine/Proton)
    if let Some(exe) = c.iter().find(|n| n.to_lowercase().ends_with(".exe")) {
        return exe.trim_end_matches(".exe").trim_end_matches(".EXE").to_string();
    }
    // 2) o primeiro nome que não é um wrapper
    c.iter()
        .find(|n| !is_wrapper(n))
        .cloned()
        .or_else(|| c.first().cloned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basename_handles_windows_paths() {
        assert_eq!(basename("Z:\\games\\cs2.exe"), "cs2.exe");
        assert_eq!(basename("/usr/bin/foo"), "foo");
    }

    #[test]
    fn wrappers_are_detected() {
        assert!(is_wrapper("wine64-preloader"));
        assert!(is_wrapper("pressure-vessel-adverb"));
        assert!(!is_wrapper("cs2"));
    }

    #[test]
    fn suggestion_falls_back_to_class() {
        let w = WindowInfo { pid: 0, class: "steam_app_730".into(), ..Default::default() };
        assert_eq!(suggested_process(&w), "steam_app_730");
    }
}
