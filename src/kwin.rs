//! Integração com o KWin: carrega o script de foco via D-Bus e o recarrega se o
//! KWin reiniciar.

use crate::assets;
use crate::engine::Event;
use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use zbus::{Connection, Proxy};

const KWIN: &str = "org.kde.KWin";
const SCRIPT_NAME: &str = "vibrance-kde";
const RAISE_NAME: &str = "vibrance-kde-raise";

async fn scripting(conn: &Connection) -> Result<Proxy<'static>> {
    Proxy::new(conn, KWIN, "/Scripting", "org.kde.kwin.Scripting")
        .await
        .context("KWin indisponível no D-Bus")
}

/// Carrega e executa um script do KWin a partir de um arquivo.
async fn load_and_run(conn: &Connection, path: &str, name: &str) -> Result<()> {
    let scripting = scripting(conn).await?;
    // descarrega uma versão anterior com o mesmo nome (ignora o resultado)
    let _: Result<bool, _> = scripting.call("unloadScript", &(name,)).await;
    let id: i32 = scripting
        .call("loadScript", &(path, name))
        .await
        .context("loadScript falhou")?;
    if id < 0 {
        return Err(anyhow!("o KWin recusou o script (id {id})"));
    }
    // Plasma 6 expõe o objeto em /Scripting/Script<id>; versões antigas em /<id>
    let mut last: Option<zbus::Error> = None;
    for obj in [format!("/Scripting/Script{id}"), format!("/{id}")] {
        let script = Proxy::new(conn, KWIN, obj, "org.kde.kwin.Script").await?;
        match script.call::<_, _, ()>("run", &()).await {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
    }
    Err(anyhow!("não foi possível executar o script: {}", last.map(|e| e.to_string()).unwrap_or_default()))
}

pub async fn load_focus_script(conn: &Connection) -> Result<()> {
    let path = assets::kwin_script_path();
    load_and_run(conn, &path.to_string_lossy(), SCRIPT_NAME).await
}

pub async fn unload_focus_script(conn: &Connection) {
    if let Ok(s) = scripting(conn).await {
        let _: Result<bool, _> = s.call("unloadScript", &(SCRIPT_NAME,)).await;
    }
}

/// Traz a janela do aplicativo para a frente (a GUI já pode estar aberta/minimizada).
pub async fn raise_gui_window(conn: &Connection) -> Result<()> {
    let js = format!(
        "var all = workspace.windowList();\n\
         for (var i = 0; i < all.length; i++) {{\n\
           var w = all[i];\n\
           if (w.resourceClass == \"{id}\" || w.resourceName == \"{id}\") {{\n\
             w.minimized = false;\n\
             workspace.activeWindow = w;\n\
           }}\n\
         }}\n",
        id = assets::APP_ID
    );
    let dir = assets::kwin_script_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let path = dir.join("raise.js");
    std::fs::write(&path, js).context("escrevendo script de foco")?;
    load_and_run(conn, &path.to_string_lossy(), RAISE_NAME).await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    if let Ok(s) = scripting(conn).await {
        let _: Result<bool, _> = s.call("unloadScript", &(RAISE_NAME,)).await;
    }
    Ok(())
}

/// Mantém o script carregado: tenta agora e de novo quando o KWin (re)aparece no barramento.
pub async fn keep_loaded(conn: Connection, tx: UnboundedSender<Event>) {
    let attempt = |conn: Connection, tx: UnboundedSender<Event>| async move {
        match load_focus_script(&conn).await {
            Ok(()) => {
                log::info!("script de foco carregado no KWin");
                let _ = tx.send(Event::KwinConnected(true));
            }
            Err(e) => {
                log::warn!("script de foco não carregado: {e:#}");
                let _ = tx.send(Event::KwinConnected(false));
            }
        }
    };
    attempt(conn.clone(), tx.clone()).await;

    let Ok(dbus) = zbus::fdo::DBusProxy::new(&conn).await else { return };
    let Ok(mut stream) = dbus.receive_name_owner_changed().await else { return };
    while let Some(sig) = stream.next().await {
        let Ok(args) = sig.args() else { continue };
        if args.name().as_str() != KWIN {
            continue;
        }
        if args.new_owner().is_some() {
            // dá tempo do KWin terminar de subir antes de carregar o script
            tokio::time::sleep(Duration::from_secs(2)).await;
            attempt(conn.clone(), tx.clone()).await;
        } else {
            let _ = tx.send(Event::KwinConnected(false));
        }
    }
}
