//! Processo residente: bandeja + observador de janelas + motor.

use crate::config::Config;
use crate::engine::{Engine, Event, Status};
use crate::ipc::{Service, PATH, SERVICE};
use crate::tray::{TrayAction, VibranceTray};
use crate::{assets, autostart, kwin};
use anyhow::{Context, Result};
use ksni::TrayMethods;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, watch, Notify};

fn spawn_gui() -> Option<Child> {
    let exe = std::env::current_exe().ok()?;
    Command::new(exe)
        .arg("gui")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(false)
        .spawn()
        .map_err(|e| log::error!("não foi possível abrir a janela de configurações: {e}"))
        .ok()
}

/// Abre a GUI, ou traz para a frente a que já está aberta.
async fn open_gui(child: &mut Option<Child>, conn: &zbus::Connection) {
    if let Some(c) = child {
        if matches!(c.try_wait(), Ok(None)) {
            if let Err(e) = kwin::raise_gui_window(conn).await {
                log::debug!("não foi possível trazer a janela para a frente: {e:#}");
            }
            return;
        }
    }
    *child = spawn_gui();
}

type TrayHandle = Arc<tokio::sync::Mutex<Option<ksni::Handle<VibranceTray>>>>;

/// Cria o ícone da bandeja em segundo plano, tentando de novo enquanto o painel
/// ainda não subiu (comum no autostart). Nunca bloqueia o laço principal.
fn spawn_tray_task(
    slot: TrayHandle,
    status: watch::Receiver<Status>,
    icons_ok: bool,
    tx: mpsc::UnboundedSender<TrayAction>,
) {
    tokio::spawn(async move {
        for attempt in 1..=60 {
            let tray = VibranceTray {
                status: status.borrow().clone(),
                autostart: autostart::is_enabled(),
                icons_ok,
                tx: tx.clone(),
            };
            match tray.spawn().await {
                Ok(handle) => {
                    // o estado pode ter mudado durante a espera
                    let st = status.borrow().clone();
                    handle.update(|t| t.status = st).await;
                    *slot.lock().await = Some(handle);
                    log::info!("ícone da bandeja criado");
                    return;
                }
                Err(e) => {
                    log::warn!("bandeja indisponível (tentativa {attempt}): {e}");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
        }
        log::error!("sem bandeja do sistema; o daemon segue funcionando sem ícone");
    });
}

/// Executa o daemon. `open_gui_on_start` abre a janela de configurações logo de início.
pub async fn run(open_gui_on_start: bool) -> Result<()> {
    let icons_ok = assets::ensure_installed()
        .map_err(|e| log::warn!("instalando assets: {e:#}"))
        .unwrap_or(false);

    let cfg = Config::load();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel::<Event>();
    let (status_tx, status_rx) = watch::channel(Status {
        enabled: cfg.enabled,
        windows_level: cfg.windows_level,
        active_level: cfg.windows_level,
        ..Default::default()
    });
    let quit = Arc::new(Notify::new());
    let gui_req = Arc::new(Notify::new());

    let service = Service {
        tx: ev_tx.clone(),
        status: status_rx.clone(),
        quit: quit.clone(),
        open_gui: gui_req.clone(),
    };
    let builder = zbus::connection::Builder::session()
        .context("conectando ao barramento de sessão")?
        .name(SERVICE)
        .context("nome D-Bus inválido")?
        .serve_at(PATH, service)
        .context("registrando o serviço D-Bus")?;
    let conn = match builder.build().await {
        Ok(c) => c,
        Err(zbus::Error::NameTaken) => {
            // já existe um daemon: só pede para abrir a janela
            log::info!("o daemon já está em execução");
            if open_gui_on_start {
                if let Ok(c) = zbus::Connection::session().await {
                    if let Ok(p) = crate::ipc::DaemonProxy::new(&c).await {
                        let _ = p.open_gui().await;
                    }
                }
            }
            return Ok(());
        }
        Err(e) => return Err(e).context("iniciando o serviço D-Bus"),
    };

    let engine = Engine::new(cfg, status_tx);
    let engine_task = tokio::spawn(engine.run(ev_rx));
    let kwin_task = tokio::spawn(kwin::keep_loaded(conn.clone(), ev_tx.clone()));

    let (act_tx, mut act_rx) = mpsc::unbounded_channel::<TrayAction>();
    let tray_slot: TrayHandle = Arc::new(tokio::sync::Mutex::new(None));
    spawn_tray_task(tray_slot.clone(), status_rx.clone(), icons_ok, act_tx);

    let mut gui_child: Option<Child> = None;
    if open_gui_on_start {
        open_gui(&mut gui_child, &conn).await;
    }

    let mut status_rx = status_rx;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("instalando handler de SIGTERM")?;

    loop {
        tokio::select! {
            _ = quit.notified() => break,
            _ = tokio::signal::ctrl_c() => break,
            _ = sigterm.recv() => break,
            _ = gui_req.notified() => open_gui(&mut gui_child, &conn).await,
            changed = status_rx.changed() => {
                if changed.is_err() { break; }
                let st = status_rx.borrow_and_update().clone();
                if let Some(h) = tray_slot.lock().await.as_ref() {
                    h.update(|t| t.status = st).await;
                }
            }
            Some(action) = act_rx.recv() => match action {
                TrayAction::OpenGui => open_gui(&mut gui_child, &conn).await,
                TrayAction::SetEnabled(on) => { let _ = ev_tx.send(Event::SetEnabled(on)); }
                TrayAction::SetAutostart(on) => {
                    if let Err(e) = autostart::set(on) {
                        log::error!("autostart: {e:#}");
                    }
                    if let Some(h) = tray_slot.lock().await.as_ref() {
                        h.update(|t| t.autostart = autostart::is_enabled()).await;
                    }
                }
                TrayAction::Quit => break,
            },
        }
    }

    log::info!("encerrando: restaurando monitores");
    kwin::unload_focus_script(&conn).await;
    let (done_tx, done_rx) = oneshot::channel();
    if ev_tx.send(Event::Shutdown(done_tx)).is_ok() {
        let _ = tokio::time::timeout(Duration::from_secs(20), done_rx).await;
    }
    kwin_task.abort();
    let _ = tokio::time::timeout(Duration::from_secs(2), engine_task).await;
    if let Some(h) = tray_slot.lock().await.take() {
        let _ = h.shutdown().await;
    }
    if let Some(mut c) = gui_child {
        let _ = c.start_kill();
    }
    Ok(())
}
