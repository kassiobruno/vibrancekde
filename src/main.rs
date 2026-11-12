//! vibrance-kde: "digital vibrance" por jogo para KDE Plasma 6 (Wayland).
//!
//! Alternativa para Linux ao vibranceGUI do Windows: quando uma janela cadastrada
//! ganha o foco, a saturação do monitor sobe; ao sair, volta ao normal.

mod assets;
mod autostart;
mod config;
mod daemon;
mod engine;
mod gui;
mod icc;
mod ipc;
mod kscreen;
mod kwin;
mod procinfo;
mod tray;

use anyhow::{bail, Context, Result};
use config::{LEVEL_MAX, LEVEL_NEUTRAL};

const HELP: &str = "\
vibrance-kde - vibrance (saturação) automático por jogo para KDE Plasma 6 / Wayland

USO:
  vibrance-kde                 inicia o daemon (bandeja) e abre a janela de configurações
  vibrance-kde --minimized     inicia só o daemon, minimizado na bandeja (usado no autostart)
  vibrance-kde gui             abre a janela de configurações
  vibrance-kde status          mostra o estado do daemon em execução
  vibrance-kde list-outputs    lista monitores e modos de vídeo
  vibrance-kde set <monitor> <nível>   aplica um nível manualmente (50 = normal, 100 = máximo)
  vibrance-kde reset           remove qualquer vibrance aplicado por este programa
  vibrance-kde autostart on|off        liga/desliga o início com o sistema
  vibrance-kde install         instala ícones, atalho de menu e script do KWin
  vibrance-kde uninstall       remove tudo o que o 'install' criou (inclui autostart)
  vibrance-kde --version

Variável RUST_LOG=debug mostra os comandos enviados ao kscreen-doctor.";

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("criando runtime")
}

fn parse_level(s: &str) -> Result<u8> {
    let v: i32 = s.trim_end_matches('%').parse().with_context(|| format!("nível inválido: {s}"))?;
    if !(i32::from(LEVEL_NEUTRAL)..=i32::from(LEVEL_MAX)).contains(&v) {
        bail!("o nível deve estar entre {LEVEL_NEUTRAL} e {LEVEL_MAX}");
    }
    Ok(v as u8)
}

async fn cmd_set(output: &str, level: u8) -> Result<()> {
    let outs = kscreen::list_outputs().await?;
    let out = outs
        .iter()
        .find(|o| o.name == output)
        .with_context(|| format!("monitor '{output}' não encontrado (use list-outputs)"))?;
    let (args, profile) = kscreen::level_args(&out.name, level, &out.color)?;
    kscreen::apply(args).await?;
    kscreen::cleanup_profiles(&out.name, profile.as_deref());
    println!("{output}: nível {level}% aplicado");
    Ok(())
}

async fn cmd_reset() -> Result<()> {
    let outs = kscreen::list_outputs().await?;
    let mut args = Vec::new();
    for o in outs.iter().filter(|o| o.stale_profile) {
        args.extend(kscreen::restore_args(&o.name, &kscreen::ColorState::neutral()));
    }
    kscreen::apply(args).await?;
    for o in &outs {
        kscreen::cleanup_profiles(&o.name, None);
    }
    println!("vibrance removido de todos os monitores");
    Ok(())
}

async fn cmd_list() -> Result<()> {
    for o in kscreen::list_outputs().await? {
        let cur = o.current_mode().map(|m| m.resolution().to_string()).unwrap_or_default();
        println!(
            "{}{}  atual: {}  perfil ICC: {}",
            o.name,
            if o.primary { " (principal)" } else { "" },
            cur,
            if o.color_capable { "suportado" } else { "NÃO suportado" }
        );
        for m in &o.modes {
            println!("    {}", m.resolution());
        }
    }
    Ok(())
}

async fn cmd_status() -> Result<()> {
    let conn = zbus::Connection::session().await.context("sem barramento de sessão")?;
    let proxy = ipc::DaemonProxy::new(&conn).await?;
    match proxy.status().await {
        Ok(json) => {
            let st: engine::Status = serde_json::from_str(&json)?;
            println!("{}", serde_json::to_string_pretty(&st)?);
            Ok(())
        }
        Err(_) => bail!("o daemon não está em execução"),
    }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a: Vec<&str> = args.iter().map(String::as_str).collect();

    match a.as_slice() {
        [] => runtime()?.block_on(daemon::run(true)),
        ["--minimized"] | ["-m"] | ["daemon"] => runtime()?.block_on(daemon::run(false)),
        ["gui"] => gui::run(),
        #[cfg(feature = "screenshots")]
        ["screenshots", dir] => gui::run_screenshots(dir),
        ["status"] => runtime()?.block_on(cmd_status()),
        ["list-outputs"] => runtime()?.block_on(cmd_list()),
        ["set", output, level] => runtime()?.block_on(cmd_set(output, parse_level(level)?)),
        ["reset"] => runtime()?.block_on(cmd_reset()),
        ["autostart", "on"] => autostart::set(true),
        ["autostart", "off"] => autostart::set(false),
        ["install"] => {
            assets::ensure_installed()?;
            println!("ícones, atalho de menu e script do KWin instalados");
            Ok(())
        }
        ["uninstall"] => {
            let _ = autostart::set(false);
            assets::remove_installed();
            println!("removido (a configuração em ~/.config/vibrance-kde foi mantida)");
            Ok(())
        }
        // uso interno: gera um perfil ICC com fator de saturação exato (testes)
        ["gen-icc", desc, factor, path] => {
            let f: f64 = factor.parse().context("fator inválido")?;
            std::fs::write(path, icc::build_profile(desc, f))?;
            Ok(())
        }
        ["--version"] | ["-V"] => {
            println!("vibrance-kde {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        ["--help"] | ["-h"] | ["help"] => {
            println!("{HELP}");
            Ok(())
        }
        other => bail!("argumentos desconhecidos: {}\n\n{HELP}", other.join(" ")),
    }
}
