//! Configuração persistente (TOML em `~/.config/vibrance-kde/config.toml`).
//!
//! Equivale ao par `vibranceGUI.ini` + `applicationData.xml` do original.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Nível neutro (sem alteração), como o "50%" do vibranceGUI.
pub const LEVEL_NEUTRAL: u8 = 50;
/// Nível máximo (2x de saturação), como o "100%" do vibranceGUI.
pub const LEVEL_MAX: u8 = 100;

/// Converte o nível do slider (50..=100) em fator de saturação (1.0..=2.0).
pub fn level_to_factor(level: u8) -> f64 {
    f64::from(level.clamp(LEVEL_NEUTRAL, LEVEL_MAX)) / f64::from(LEVEL_NEUTRAL)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
}

impl std::fmt::Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{} @ {} Hz", self.width, self.height, self.refresh)
    }
}

/// Perfil de um programa (equivale a `ApplicationSetting`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppProfile {
    /// Nome exibido.
    pub name: String,
    /// Nome do processo (sem `.exe`, sem diferenciar maiúsculas) ou classe da janela.
    pub process: String,
    /// Nível de vibrance dentro do jogo (50..=100).
    #[serde(default = "default_ingame_level")]
    pub level: u8,
    /// Trocar a resolução do monitor enquanto o jogo estiver em foco.
    #[serde(default)]
    pub change_resolution: bool,
    #[serde(default)]
    pub resolution: Option<Resolution>,
}

fn default_ingame_level() -> u8 {
    75
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Liga/desliga a observação de janelas.
    pub enabled: bool,
    /// Nível usado fora dos jogos ("Windows Vibrance Level").
    pub windows_level: u8,
    /// Só mexer no monitor principal.
    pub affect_primary_only: bool,
    /// Nunca trocar a resolução de nenhum monitor.
    pub never_switch_resolution: bool,
    pub apps: Vec<AppProfile>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            windows_level: LEVEL_NEUTRAL,
            affect_primary_only: false,
            never_switch_resolution: false,
            apps: Vec::new(),
        }
    }
}

impl Config {
    pub fn dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("vibrance-kde")
    }

    pub fn path() -> PathBuf {
        Self::dir().join("config.toml")
    }

    /// Lê a configuração; arquivo ausente ou inválido gera os valores padrão.
    pub fn load() -> Self {
        match std::fs::read_to_string(Self::path()) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(mut cfg) => {
                    cfg.sanitize();
                    cfg
                }
                Err(e) => {
                    log::warn!("config inválida ({e}); usando padrão");
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        }
    }

    /// Mantém todos os valores em faixas válidas.
    pub fn sanitize(&mut self) {
        self.windows_level = self.windows_level.clamp(LEVEL_NEUTRAL, LEVEL_MAX);
        for app in &mut self.apps {
            app.level = app.level.clamp(LEVEL_NEUTRAL, LEVEL_MAX);
        }
    }

    /// Grava de forma atômica (arquivo temporário + rename).
    pub fn save(&self) -> Result<()> {
        let dir = Self::dir();
        std::fs::create_dir_all(&dir).with_context(|| format!("criando {}", dir.display()))?;
        let text = toml::to_string_pretty(self).context("serializando config")?;
        let tmp = dir.join("config.toml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("escrevendo {}", tmp.display()))?;
        std::fs::rename(&tmp, Self::path()).context("substituindo config.toml")?;
        Ok(())
    }

    /// Procura o perfil que corresponde a algum dos nomes candidatos do processo.
    pub fn find_profile(&self, candidates: &[String]) -> Option<&AppProfile> {
        self.apps
            .iter()
            .find(|a| candidates.iter().any(|c| names_match(&a.process, c)))
    }
}

/// Compara nomes de processo sem diferenciar maiúsculas e ignorando `.exe`.
pub fn names_match(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let s = s.trim().to_lowercase();
        s.strip_suffix(".exe").map(str::to_owned).unwrap_or(s)
    };
    let (a, b) = (norm(a), norm(b));
    !a.is_empty() && a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factor_mapping() {
        assert!((level_to_factor(50) - 1.0).abs() < 1e-9);
        assert!((level_to_factor(75) - 1.5).abs() < 1e-9);
        assert!((level_to_factor(100) - 2.0).abs() < 1e-9);
        assert!((level_to_factor(0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn matching_ignores_case_and_exe() {
        assert!(names_match("CS2", "cs2"));
        assert!(names_match("game.exe", "Game"));
        assert!(!names_match("", ""));
        assert!(!names_match("cs2", "csgo"));
    }

    #[test]
    fn roundtrip_toml() {
        let mut cfg = Config::default();
        cfg.apps.push(AppProfile {
            name: "CS2".into(),
            process: "cs2".into(),
            level: 80,
            change_resolution: true,
            resolution: Some(Resolution { width: 1920, height: 1080, refresh: 144 }),
        });
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg, back);
        assert_eq!(back.find_profile(&["CS2".to_string()]).unwrap().level, 80);
    }

    #[test]
    fn partial_toml_uses_defaults() {
        let cfg: Config = toml::from_str("windows_level = 60").unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.windows_level, 60);
    }
}
