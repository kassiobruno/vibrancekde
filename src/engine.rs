//! Motor de decisão: reage à janela em foco e aplica vibrance/resolução.
//!
//! Reproduz o laço do vibranceGUI (`OnWinEventHook`):
//! * janela de um programa cadastrado em foco  -> nível "ingame" no monitor dela
//! * qualquer outra janela                      -> nível "Windows" em todos os monitores
//! * ao encerrar                                -> restaura o estado original

use crate::config::{AppProfile, Config, Resolution, LEVEL_NEUTRAL};
use crate::kscreen::{self, ColorState, OutputInfo};
use crate::procinfo::{self, WindowInfo};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::{mpsc, oneshot, watch};

/// Marca um monitor que já tinha um perfil nosso ao iniciar (sessão anterior interrompida).
const STALE_LEVEL: u8 = 255;

pub enum Event {
    /// Janela em foco mudou.
    Window(WindowInfo),
    /// Lista de janelas abertas (para o "explorador de processos" da GUI).
    WindowList(Vec<WindowInfo>),
    /// Recarregar o arquivo de configuração.
    Reload,
    SetEnabled(bool),
    /// Pré-visualização do nível; `None` encerra.
    Preview(Option<u8>),
    KwinConnected(bool),
    ListWindows(oneshot::Sender<Vec<WindowInfo>>),
    /// Restaura tudo e encerra o motor.
    Shutdown(oneshot::Sender<()>),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OutputStatus {
    pub name: String,
    pub level: u8,
    pub primary: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Status {
    pub enabled: bool,
    pub kwin_connected: bool,
    pub active_app: Option<String>,
    pub active_level: u8,
    pub windows_level: u8,
    pub outputs: Vec<OutputStatus>,
    pub error: Option<String>,
}

struct Touched {
    /// Nível aplicado agora.
    level: u8,
    /// Estado de cor antes de mexermos.
    original: ColorState,
    /// Modo de vídeo a restaurar, se trocamos a resolução.
    saved_mode: Option<String>,
    /// Arquivo de perfil que o KWin está usando agora (o único que não pode ser apagado).
    profile: Option<std::path::PathBuf>,
}

#[derive(PartialEq, Clone)]
struct Key {
    app: Option<String>,
    output: String,
    enabled: bool,
    generation: u64,
    preview: Option<u8>,
}

pub struct Engine {
    cfg: Config,
    generation: u64,
    window: WindowInfo,
    windows: Vec<WindowInfo>,
    active: Option<AppProfile>,
    preview: Option<u8>,
    kwin_connected: bool,
    touched: HashMap<String, Touched>,
    last_key: Option<Key>,
    error: Option<String>,
    status_tx: watch::Sender<Status>,
}

fn state_file() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("vibrance-kde")
        .join("originals.json")
}

fn load_saved_originals() -> HashMap<String, ColorState> {
    std::fs::read_to_string(state_file())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

impl Engine {
    pub fn new(cfg: Config, status_tx: watch::Sender<Status>) -> Self {
        Self {
            cfg,
            generation: 0,
            window: WindowInfo::default(),
            windows: Vec::new(),
            active: None,
            preview: None,
            kwin_connected: false,
            touched: HashMap::new(),
            last_key: None,
            error: None,
            status_tx,
        }
    }

    pub async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Event>) {
        self.reconcile().await;
        while let Some(first) = rx.recv().await {
            let mut shutdown: Option<oneshot::Sender<()>> = None;
            let mut pending = Some(first);
            // junta eventos em rajada (ex.: telas de abertura do jogo) numa só aplicação
            loop {
                match pending.take() {
                    Some(ev) => {
                        if let Some(done) = self.handle(ev) {
                            shutdown = Some(done);
                            break;
                        }
                    }
                    None => break,
                }
                pending = rx.try_recv().ok();
            }
            if let Some(done) = shutdown {
                self.restore_all().await;
                let _ = done.send(());
                return;
            }
            self.reconcile_if_needed().await;
        }
        self.restore_all().await;
    }

    /// Trata um evento; devolve o canal de confirmação se for pedido de encerramento.
    fn handle(&mut self, ev: Event) -> Option<oneshot::Sender<()>> {
        match ev {
            Event::Window(w) => {
                self.window = w;
                self.refresh_active();
            }
            Event::WindowList(list) => self.windows = list,
            Event::Reload => {
                self.cfg = Config::load();
                self.generation += 1;
                self.refresh_active();
            }
            Event::SetEnabled(on) => {
                self.cfg.enabled = on;
                if let Err(e) = self.cfg.save() {
                    log::warn!("não foi possível salvar a config: {e:#}");
                }
                self.generation += 1;
            }
            Event::Preview(level) => self.preview = level,
            Event::KwinConnected(c) => {
                self.kwin_connected = c;
                self.publish(None);
            }
            Event::ListWindows(reply) => {
                let _ = reply.send(self.windows.clone());
            }
            Event::Shutdown(done) => return Some(done),
        }
        None
    }

    fn refresh_active(&mut self) {
        let cands = procinfo::candidates(&self.window);
        self.active = self.cfg.find_profile(&cands).cloned();
        if let Some(a) = &self.active {
            log::info!("janela em foco corresponde a '{}' ({})", a.name, a.process);
        }
    }

    fn key(&self) -> Key {
        Key {
            app: self.active.as_ref().map(|a| a.process.clone()),
            output: self.window.output.clone(),
            enabled: self.cfg.enabled,
            generation: self.generation,
            preview: self.preview,
        }
    }

    async fn reconcile_if_needed(&mut self) {
        let key = self.key();
        if self.last_key.as_ref() == Some(&key) && self.error.is_none() {
            return;
        }
        self.reconcile().await;
    }

    async fn reconcile(&mut self) {
        let key = self.key();
        match self.try_reconcile().await {
            Ok(outs) => {
                self.error = None;
                self.last_key = Some(key);
                self.publish(Some(&outs));
            }
            Err(e) => {
                log::error!("falha ao aplicar: {e:#}");
                self.error = Some(format!("{e:#}"));
                self.last_key = None;
                self.publish(None);
            }
        }
    }

    async fn try_reconcile(&mut self) -> Result<Vec<OutputInfo>> {
        let outs = kscreen::list_outputs().await?;

        // adota monitores que ficaram com perfil nosso de uma sessão interrompida
        let saved = load_saved_originals();
        for o in outs.iter().filter(|o| o.stale_profile) {
            self.touched.entry(o.name.clone()).or_insert_with(|| Touched {
                level: STALE_LEVEL,
                original: saved.get(&o.name).cloned().unwrap_or_else(ColorState::neutral),
                saved_mode: None,
                profile: None,
            });
        }

        let enabled = self.cfg.enabled;
        let app = if enabled { self.active.clone() } else { None };
        let primary_only = self.cfg.affect_primary_only;
        let capable = |o: &&OutputInfo| o.color_capable && (!primary_only || o.primary);
        let managed: Vec<&OutputInfo> = outs.iter().filter(capable).collect();

        // monitor onde o jogo está (ou o principal, se a janela não informou)
        let game_out: Option<String> = outs
            .iter()
            .find(|o| !self.window.output.is_empty() && o.name == self.window.output)
            .or_else(|| outs.iter().find(|o| o.primary))
            .or_else(|| outs.first())
            .map(|o| o.name.clone());

        let mut want: HashMap<String, u8> = HashMap::new();
        if enabled {
            for o in &managed {
                want.insert(o.name.clone(), self.cfg.windows_level);
            }
            if let (Some(a), Some(g)) = (&app, &game_out) {
                if want.contains_key(g) {
                    want.insert(g.clone(), a.level);
                }
            }
            if let Some(p) = self.preview {
                let target = game_out
                    .clone()
                    .filter(|g| want.contains_key(g))
                    .or_else(|| managed.first().map(|o| o.name.clone()));
                if let Some(t) = target {
                    want.insert(t, p.clamp(LEVEL_NEUTRAL, 100));
                }
            }
        }

        let want_res: Option<(String, Resolution)> = app
            .as_ref()
            .filter(|a| a.change_resolution && !self.cfg.never_switch_resolution)
            .and_then(|a| a.resolution.clone())
            .zip(game_out.clone())
            .map(|(r, g)| (g, r));

        struct Pending {
            level: Option<u8>,
            profile: Option<std::path::PathBuf>,
            original: ColorState,
            saved_mode: Option<Option<String>>,
        }
        let mut pending: HashMap<String, Pending> = HashMap::new();
        let mut args: Vec<String> = Vec::new();

        for o in &outs {
            let cur = self.touched.get(&o.name).map_or(LEVEL_NEUTRAL, |t| t.level);
            let target = want.get(&o.name).copied().unwrap_or(LEVEL_NEUTRAL);
            let original = self
                .touched
                .get(&o.name)
                .map(|t| t.original.clone())
                .unwrap_or_else(|| o.color.clone());
            let mut p = Pending {
                level: None,
                profile: None,
                original: original.clone(),
                saved_mode: None,
            };

            if target != cur && o.color_capable {
                match kscreen::level_args(&o.name, target, &original) {
                    Ok((a, profile)) => {
                        args.extend(a);
                        p.level = Some(target);
                        p.profile = profile;
                    }
                    Err(e) => log::error!("perfil ICC de {}: {e:#}", o.name),
                }
            }

            // resolução
            let saved = self.touched.get(&o.name).and_then(|t| t.saved_mode.clone());
            let want_mode = want_res
                .as_ref()
                .filter(|(n, _)| *n == o.name)
                .and_then(|(_, r)| {
                    o.modes.iter().find(|m| {
                        m.width == r.width && m.height == r.height && m.refresh.round() as u32 == r.refresh
                    })
                });
            match (want_mode, saved) {
                (Some(m), saved) => {
                    if o.current_mode_id != m.id {
                        if saved.is_none() {
                            p.saved_mode = Some(Some(o.current_mode_id.clone()));
                        }
                        args.push(format!("output.{}.mode.{}", o.name, m.id));
                    }
                }
                (None, Some(id)) => {
                    if o.modes.iter().any(|m| m.id == id) && o.current_mode_id != id {
                        args.push(format!("output.{}.mode.{id}", o.name));
                    }
                    p.saved_mode = Some(None);
                }
                (None, None) => {}
            }

            if p.level.is_some() || p.saved_mode.is_some() {
                pending.insert(o.name.clone(), p);
            }
        }

        if !args.is_empty() {
            kscreen::apply(args).await?;
        }

        for (name, p) in pending {
            let entry = self.touched.entry(name.clone()).or_insert_with(|| Touched {
                level: LEVEL_NEUTRAL,
                original: p.original.clone(),
                saved_mode: None,
                profile: None,
            });
            if let Some(l) = p.level {
                entry.level = l;
                entry.profile = p.profile.clone();
            }
            if let Some(s) = p.saved_mode {
                entry.saved_mode = s;
            }
            let (level, has_mode) = (entry.level, entry.saved_mode.is_some());
            // apaga os perfis antigos, preservando o que o KWin usa agora
            kscreen::cleanup_profiles(
                &name,
                if level <= LEVEL_NEUTRAL { None } else { entry.profile.as_deref() },
            );
            if level <= LEVEL_NEUTRAL && !has_mode {
                self.touched.remove(&name);
            }
        }
        self.persist_originals();
        Ok(outs)
    }

    fn persist_originals(&self) {
        let file = state_file();
        let active: HashMap<&String, &ColorState> = self
            .touched
            .iter()
            .filter(|(_, t)| t.level != LEVEL_NEUTRAL)
            .map(|(n, t)| (n, &t.original))
            .collect();
        if active.is_empty() {
            let _ = std::fs::remove_file(&file);
            return;
        }
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string(&active) {
            let _ = std::fs::write(&file, text);
        }
    }

    /// Restaura cor e resolução de todos os monitores que alteramos.
    async fn restore_all(&mut self) {
        if self.touched.is_empty() {
            return;
        }
        let mut args = Vec::new();
        for (name, t) in &self.touched {
            if t.level != LEVEL_NEUTRAL {
                args.extend(kscreen::restore_args(name, &t.original));
            }
            if let Some(id) = &t.saved_mode {
                args.push(format!("output.{name}.mode.{id}"));
            }
        }
        if let Err(e) = kscreen::apply(args).await {
            log::error!("falha ao restaurar monitores: {e:#}");
        }
        for name in self.touched.keys() {
            kscreen::cleanup_profiles(name, None);
        }
        self.touched.clear();
        self.persist_originals();
        self.last_key = None;
    }

    fn publish(&self, outs: Option<&[OutputInfo]>) {
        let mut st = self.status_tx.borrow().clone();
        st.enabled = self.cfg.enabled;
        st.kwin_connected = self.kwin_connected;
        st.windows_level = self.cfg.windows_level;
        st.error = self.error.clone();
        st.active_app = if self.cfg.enabled { self.active.as_ref().map(|a| a.name.clone()) } else { None };
        if let Some(outs) = outs {
            st.outputs = outs
                .iter()
                .map(|o| OutputStatus {
                    name: o.name.clone(),
                    level: self.touched.get(&o.name).map_or(LEVEL_NEUTRAL, |t| {
                        if t.level == STALE_LEVEL { LEVEL_NEUTRAL } else { t.level }
                    }),
                    primary: o.primary,
                })
                .collect();
        }
        st.active_level = st
            .outputs
            .iter()
            .map(|o| o.level)
            .max()
            .unwrap_or(self.cfg.windows_level);
        self.status_tx.send_if_modified(|cur| {
            if *cur == st {
                false
            } else {
                *cur = st;
                true
            }
        });
    }
}
