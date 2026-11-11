//! KWin falso para testes de integração: expõe `org.kde.kwin.Scripting` e
//! `org.kde.kwin.Script` no barramento de sessão e registra o que recebe.
//!
//! Uso: `cargo run --example fake_kwin` dentro de um `dbus-run-session`.

use std::sync::{Arc, Mutex};
use zbus::interface;

struct Scripting {
    log: Arc<Mutex<Vec<String>>>,
    next_id: Mutex<i32>,
}

#[interface(name = "org.kde.kwin.Scripting")]
impl Scripting {
    #[zbus(name = "loadScript")]
    fn load_script(&self, path: &str, name: &str) -> i32 {
        let mut id = self.next_id.lock().unwrap();
        *id += 1;
        let line = format!("loadScript path={path} name={name} -> {}", *id);
        println!("{line}");
        self.log.lock().unwrap().push(line);
        *id
    }

    #[zbus(name = "unloadScript")]
    fn unload_script(&self, name: &str) -> bool {
        println!("unloadScript name={name}");
        true
    }
}

struct Script;

#[interface(name = "org.kde.kwin.Script")]
impl Script {
    #[zbus(name = "run")]
    fn run(&self) {
        println!("run()");
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> zbus::Result<()> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let _conn = zbus::connection::Builder::session()?
        .name("org.kde.KWin")?
        .serve_at("/Scripting", Scripting { log, next_id: Mutex::new(0) })?
        // Plasma 6 expõe cada script em /Scripting/Script<id>; o id 1 é o primeiro
        .serve_at("/Scripting/Script1", Script)?
        .serve_at("/Scripting/Script2", Script)?
        .build()
        .await?;
    println!("fake KWin pronto");
    std::future::pending::<()>().await;
    Ok(())
}
