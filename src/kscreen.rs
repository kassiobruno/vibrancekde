//! Interface com o `kscreen-doctor` (libkscreen do KDE).
//!
//! No Plasma 6 o KWin é dono do pipeline de cores. O jeito suportado de mudar
//! o perfil ICC e o modo de vídeo de um monitor é pelo `kscreen-doctor`.

use crate::config::{level_to_factor, Resolution, LEVEL_NEUTRAL};
use crate::icc;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

/// Prefixo dos perfis gerados; usado para reconhecer os nossos.
pub const PROFILE_PREFIX: &str = "vibrance-kde-";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Power {
    Efficiency,
    Accuracy,
}

/// Estado de cor de um monitor, para poder restaurá-lo depois.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorState {
    /// `sRGB`, `ICC` ou `EDID`; `None` se o monitor não suporta perfis.
    pub source: Option<String>,
    pub icc_path: String,
    pub power: Power,
}

impl ColorState {
    /// Estado "sem nada nosso": sRGB, sem perfil, modo eficiência.
    pub fn neutral() -> Self {
        Self { source: Some("sRGB".into()), icc_path: String::new(), power: Power::Efficiency }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModeInfo {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub refresh: f64,
}

impl ModeInfo {
    pub fn resolution(&self) -> Resolution {
        Resolution { width: self.width, height: self.height, refresh: self.refresh.round() as u32 }
    }
}

#[derive(Debug, Clone)]
pub struct OutputInfo {
    pub name: String,
    pub primary: bool,
    pub current_mode_id: String,
    pub modes: Vec<ModeInfo>,
    pub color: ColorState,
    /// O monitor aceita perfil ICC (a saturação só funciona nesses).
    pub color_capable: bool,
    /// Nosso perfil ICC ainda está ativo (sessão anterior interrompida).
    pub stale_profile: bool,
}

impl OutputInfo {
    pub fn current_mode(&self) -> Option<&ModeInfo> {
        self.modes.iter().find(|m| m.id == self.current_mode_id)
    }
}

// ---------------------------------------------------------------- JSON ----

#[derive(Deserialize)]
struct JsonRoot {
    #[serde(default)]
    outputs: Vec<JsonOutput>,
}

#[derive(Deserialize)]
struct JsonSize {
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
}

#[derive(Deserialize)]
struct JsonMode {
    id: serde_json::Value,
    #[serde(default)]
    size: Option<JsonSize>,
    #[serde(default, rename = "refreshRate")]
    refresh_rate: f64,
}

#[derive(Deserialize)]
struct JsonOutput {
    name: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    connected: bool,
    #[serde(default)]
    priority: i64,
    #[serde(default, rename = "currentModeId")]
    current_mode_id: serde_json::Value,
    #[serde(default)]
    modes: Vec<JsonMode>,
    #[serde(default, rename = "iccProfilePath")]
    icc_profile_path: String,
}

fn id_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Estado de cor lido da saída de texto (`kscreen-doctor -o`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TextColor {
    pub source: Option<String>,
    pub power: Option<Power>,
    pub capable: bool,
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Extrai, por monitor, a fonte do perfil e a preferência de energia.
pub fn parse_text_colors(text: &str) -> Vec<(String, TextColor)> {
    let text = strip_ansi(text);
    let mut result: Vec<(String, TextColor)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Output:") {
            // "Output: 1 DP-1 enabled connected ..."
            let name = rest.split_whitespace().nth(1).unwrap_or("").to_string();
            result.push((name, TextColor::default()));
        } else if let Some(v) = line.strip_prefix("Color profile source:") {
            if let Some((_, c)) = result.last_mut() {
                let v = v.trim();
                if v.eq_ignore_ascii_case("incapable") {
                    c.capable = false;
                } else {
                    c.capable = true;
                    c.source = Some(v.to_string());
                }
            }
        } else if let Some(v) = line.strip_prefix("Color power preference:") {
            if let Some((_, c)) = result.last_mut() {
                let v = v.trim().to_lowercase();
                c.power = Some(if v.contains("accuracy") { Power::Accuracy } else { Power::Efficiency });
            }
        }
    }
    result
}

pub fn parse_outputs(json: &str, text: &str) -> Result<Vec<OutputInfo>> {
    let root: JsonRoot = serde_json::from_str(json).context("JSON do kscreen-doctor inválido")?;
    let colors = parse_text_colors(text);
    let mut out = Vec::new();
    for o in root.outputs {
        if !(o.enabled && o.connected) {
            continue;
        }
        let tc = colors.iter().find(|(n, _)| *n == o.name).map(|(_, c)| c.clone());
        let modes = o
            .modes
            .iter()
            .map(|m| ModeInfo {
                id: id_string(&m.id),
                width: m.size.as_ref().map_or(0, |s| s.width),
                height: m.size.as_ref().map_or(0, |s| s.height),
                refresh: m.refresh_rate,
            })
            .collect();
        let ours = std::path::Path::new(&o.icc_profile_path)
            .file_name()
            .and_then(|f| f.to_str())
            .is_some_and(|f| f.starts_with(PROFILE_PREFIX));
        // Depois de restaurar para sRGB o KWin continua guardando o caminho do nosso
        // perfil antigo (não dá para passar caminho vazio ao kscreen-doctor). Só está
        // "ativo" se a fonte de cor ainda for ICC; sem o texto, assume que está.
        let source_is_icc = tc
            .as_ref()
            .and_then(|c| c.source.as_deref())
            .map_or(true, |s| s.eq_ignore_ascii_case("ICC"));
        let stale = ours && source_is_icc;
        // Se o perfil é nosso (ex.: crash anterior ou resto de uma sessão), o "original" é neutro.
        let color = if ours {
            ColorState::neutral()
        } else {
            let tc = tc.clone().unwrap_or_default();
            ColorState {
                source: tc.source,
                icc_path: o.icc_profile_path.clone(),
                power: tc.power.unwrap_or(Power::Efficiency),
            }
        };
        // Sem texto para comparar, assume que o monitor aceita ICC (o JSON só traz
        // `iccProfilePath` quando a capacidade existe, e o reference faz o mesmo).
        let capable = tc.map_or(true, |c| c.capable);
        out.push(OutputInfo {
            name: o.name,
            primary: o.priority == 1,
            current_mode_id: id_string(&o.current_mode_id),
            modes,
            color,
            color_capable: capable,
            stale_profile: stale,
        });
    }
    Ok(out)
}

// ------------------------------------------------------------ comandos ----

async fn run_doctor(args: &[String]) -> Result<String> {
    let child = Command::new("kscreen-doctor")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(15), child)
        .await
        .map_err(|_| anyhow!("kscreen-doctor demorou demais"))?
        .context("não foi possível executar o kscreen-doctor (instale o pacote libkscreen/kscreen)")?;
    if !out.status.success() {
        bail!(
            "kscreen-doctor falhou ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Aplica uma lista de argumentos `output.X.…` numa única transação.
pub async fn apply(args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        return Ok(());
    }
    log::debug!("kscreen-doctor {}", args.join(" "));
    run_doctor(&args).await.map(|_| ())
}

pub async fn list_outputs() -> Result<Vec<OutputInfo>> {
    let (j, o) = (["-j".to_string()], ["-o".to_string()]);
    let (json, text) = tokio::join!(run_doctor(&j), run_doctor(&o));
    // O texto é opcional: sem ele ainda dá para listar monitores e modos.
    parse_outputs(&json?, &text.unwrap_or_default())
}

// ---------------------------------------------------------- perfis ICC ----

pub fn profile_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("color")
        .join("icc")
}

/// Nome do arquivo de perfil: `vibrance-kde-<monitor>-<nível>-n<sufixo>.icc`.
///
/// O sufixo muda a CADA aplicação, de propósito. O `kscreen-doctor` só reenvia o
/// perfil ao KWin quando o caminho muda, e o KWin só lê o arquivo do disco nesse
/// momento. Com um caminho fixo por nível, depois de reiniciar a sessão o KWin
/// guarda o caminho antigo sem ter o perfil carregado (o arquivo já foi apagado),
/// e a próxima aplicação do mesmo nível não surtiria efeito: cor padrão no jogo.
pub fn profile_path(output: &str, level: u8, nonce: &str) -> PathBuf {
    profile_dir().join(format!("{PROFILE_PREFIX}{output}-{level}-n{nonce}.icc"))
}

/// Sufixo único: relógio em nanossegundos + contador (único mesmo em chamadas seguidas).
fn unique_nonce() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", nanos & 0xffff_ffff_ffff, COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Gera um perfil NOVO para o nível (sempre com caminho inédito) e devolve o caminho.
pub fn create_profile(output: &str, level: u8) -> Result<PathBuf> {
    let path = profile_path(output, level, &unique_nonce());
    std::fs::create_dir_all(profile_dir()).context("criando diretório de perfis ICC")?;
    let pct = u32::from(level) - u32::from(LEVEL_NEUTRAL);
    let desc = format!("vibrance-kde +{}% ({output})", pct * 2);
    let data = icc::build_profile(&desc, level_to_factor(level));
    let tmp = path.with_extension("icc.tmp");
    std::fs::write(&tmp, data).context("escrevendo perfil ICC")?;
    std::fs::rename(&tmp, &path).context("gravando perfil ICC")?;
    Ok(path)
}

/// `true` se `rest` (o que sobra após `vibrance-kde-<monitor>-`) é `<nível>` (formato
/// antigo, sem sufixo) ou `<nível>-n<sufixo hex>`, sem o `.icc`.
fn is_profile_tail(rest: &str) -> bool {
    let (level, nonce) = match rest.split_once('-') {
        Some((l, n)) => (l, Some(n)),
        None => (rest, None),
    };
    !level.is_empty()
        && level.bytes().all(|b| b.is_ascii_digit())
        && nonce.map_or(true, |n| {
            n.strip_prefix('n')
                .is_some_and(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()))
        })
}

/// Remove os perfis do monitor, mantendo só `keep` (o que o KWin está usando agora).
pub fn cleanup_profiles(output: &str, keep: Option<&std::path::Path>) {
    let prefix = format!("{PROFILE_PREFIX}{output}-");
    let Ok(rd) = std::fs::read_dir(profile_dir()) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let rest = name.strip_prefix(&prefix).and_then(|r| r.strip_suffix(".icc"));
        // exige o formato exato: evita apagar arquivos de outro monitor ("DP-1" x "DP-1-2")
        if !rest.is_some_and(is_profile_tail) {
            continue;
        }
        if keep.is_some_and(|k| k == e.path()) {
            continue;
        }
        let _ = std::fs::remove_file(e.path());
    }
}

fn power_arg(p: Power) -> &'static str {
    match p {
        Power::Efficiency => "preferEfficiency",
        Power::Accuracy => "preferAccuracy",
    }
}

/// Argumentos para aplicar `level` num monitor, e o perfil criado (se houver).
/// No nível neutro restaura o estado original e não cria perfil.
pub fn level_args(
    output: &str,
    level: u8,
    original: &ColorState,
) -> Result<(Vec<String>, Option<PathBuf>)> {
    if level <= LEVEL_NEUTRAL {
        return Ok((restore_args(output, original), None));
    }
    let path = create_profile(output, level)?;
    // O KWin só executa o shader ICC (a tabela 3D) em "preferir precisão"; em
    // "eficiência" ele manda só uma curva 1D ao hardware e ignora a tabela.
    let args = vec![
        format!("output.{output}.iccprofile.{}", path.display()),
        format!("output.{output}.colorProfileSource.ICC"),
        format!("output.{output}.colorPowerTradeoff.preferAccuracy"),
    ];
    Ok((args, Some(path)))
}

pub fn restore_args(output: &str, original: &ColorState) -> Vec<String> {
    let mut args = Vec::new();
    if !original.icc_path.is_empty() {
        args.push(format!("output.{output}.iccprofile.{}", original.icc_path));
    }
    let source = original
        .source
        .clone()
        .unwrap_or_else(|| if original.icc_path.is_empty() { "sRGB".into() } else { "ICC".into() });
    args.push(format!("output.{output}.colorProfileSource.{source}"));
    args.push(format!("output.{output}.colorPowerTradeoff.{}", power_arg(original.power)));
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{"outputs":[
      {"id":1,"name":"DP-1","enabled":true,"connected":true,"priority":1,
       "currentModeId":"2","iccProfilePath":"",
       "modes":[{"id":"1","name":"3840x2160@60","size":{"width":3840,"height":2160},"refreshRate":60.0},
                {"id":"2","name":"2560x1440@144","size":{"width":2560,"height":1440},"refreshRate":143.912}]},
      {"id":2,"name":"HDMI-A-1","enabled":true,"connected":true,"priority":2,
       "currentModeId":"7","iccProfilePath":"/home/u/.local/share/color/icc/vibrance-kde-HDMI-A-1-80.icc","modes":[]},
      {"id":3,"name":"DP-2","enabled":false,"connected":true,"priority":0,"modes":[]}
    ]}"#;

    const TEXT: &str = "\u{1b}[01;32mOutput: \u{1b}[0;0m1 DP-1 enabled connected priority 1 DisplayPort\n\
        \t\u{1b}[01;33mICC profile: \u{1b}[0;0mnone\n\
        \t\u{1b}[01;33mColor profile source: \u{1b}[0;0mEDID\n\
        \t\u{1b}[01;33mColor power preference: \u{1b}[0;0mprefer accuracy\n\
        Output: 2 HDMI-A-1 enabled connected priority 2 HDMI\n\
        \tColor profile source: sRGB\n\
        \tColor power preference: prefer efficiency and performance\n";

    #[test]
    fn parses_text_colors_with_ansi() {
        let c = parse_text_colors(TEXT);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].0, "DP-1");
        assert_eq!(c[0].1.source.as_deref(), Some("EDID"));
        assert_eq!(c[0].1.power, Some(Power::Accuracy));
        assert_eq!(c[1].1.power, Some(Power::Efficiency));
    }

    #[test]
    fn parses_outputs_and_restores_neutral_for_our_profile() {
        let outs = parse_outputs(JSON, TEXT).unwrap();
        assert_eq!(outs.len(), 2);
        assert!(outs[0].primary);
        assert_eq!(outs[0].color.source.as_deref(), Some("EDID"));
        assert_eq!(outs[0].color.power, Power::Accuracy);
        assert_eq!(outs[0].current_mode().unwrap().resolution().refresh, 144);
        // perfil atual é nosso => original considerado neutro; fonte já é sRGB => não está ativo
        assert_eq!(outs[1].color, ColorState::neutral());
        assert!(!outs[1].stale_profile);
    }

    #[test]
    fn our_profile_with_icc_source_is_stale() {
        let text = TEXT.replace(
            "Color profile source: sRGB",
            "Color profile source: ICC",
        );
        let outs = parse_outputs(JSON, &text).unwrap();
        assert!(outs[1].stale_profile);
        assert!(!outs[0].stale_profile);
    }

    #[test]
    fn restore_args_keep_original_profile() {
        let st = ColorState {
            source: Some("ICC".into()),
            icc_path: "/home/u/cal.icc".into(),
            power: Power::Accuracy,
        };
        let a = restore_args("DP-1", &st);
        assert_eq!(a[0], "output.DP-1.iccprofile./home/u/cal.icc");
        assert_eq!(a[1], "output.DP-1.colorProfileSource.ICC");
        assert_eq!(a[2], "output.DP-1.colorPowerTradeoff.preferAccuracy");
        let n = restore_args("DP-1", &ColorState::neutral());
        assert_eq!(n.len(), 2);
        assert!(n[0].ends_with("colorProfileSource.sRGB"));
    }

    #[test]
    fn neutral_level_restores_instead_of_profile() {
        let (a, p) = level_args("DP-1", 50, &ColorState::neutral()).unwrap();
        assert!(a.iter().all(|s| !s.contains("iccprofile")));
        assert!(p.is_none());
    }

    #[test]
    fn profile_names_are_never_reused() {
        let a = unique_nonce();
        let b = unique_nonce();
        assert_ne!(a, b);
        assert_ne!(profile_path("DP-1", 80, &a), profile_path("DP-1", 80, &b));
    }

    #[test]
    fn profile_tail_recognition() {
        assert!(is_profile_tail("80")); // formato antigo
        assert!(is_profile_tail("80-n1a2b3c"));
        assert!(!is_profile_tail("2-80")); // "DP-1-2" nível 80 no formato antigo não é de "DP-1"
        assert!(!is_profile_tail("2-80-n1a2b"));
        assert!(!is_profile_tail("80-n"));
        assert!(!is_profile_tail("80-nxyz"));
        assert!(!is_profile_tail(""));
    }
}
