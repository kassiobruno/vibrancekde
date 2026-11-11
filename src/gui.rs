//! Janela de configurações (egui). É um processo separado do daemon, então a
//! memória usada pela interface gráfica só existe enquanto a janela está aberta.

use crate::config::{AppProfile, Config, Resolution, LEVEL_MAX, LEVEL_NEUTRAL};
use crate::engine::Status;
use crate::ipc::{DaemonProxy, GUI_SERVICE};
use crate::kscreen::{self, OutputInfo};
use crate::procinfo::{self, WindowInfo};
use crate::{assets, autostart};
use anyhow::Result;
use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc as tmpsc;

// ------------------------------------------------------------- worker ----

enum Cmd {
    Reload,
    Preview(i32),
    ListWindows,
}

enum Msg {
    /// `None` = daemon fora do ar.
    Status(Option<Status>),
    Windows(Vec<WindowInfo>),
}

/// Thread de fundo que conversa com o daemon por D-Bus sem travar a interface.
fn spawn_worker(ctx: egui::Context, mut cmd_rx: tmpsc::UnboundedReceiver<Cmd>, msg_tx: mpsc::Sender<Msg>) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };
        rt.block_on(async move {
            let mut proxy: Option<DaemonProxy<'static>> = None;
            let mut tick = tokio::time::interval(Duration::from_millis(1000));
            loop {
                if proxy.is_none() {
                    if let Ok(conn) = zbus::Connection::session().await {
                        proxy = DaemonProxy::new(&conn).await.ok();
                    }
                }
                tokio::select! {
                    _ = tick.tick() => {
                        let st = match &proxy {
                            Some(p) => p.status().await.ok().and_then(|j| serde_json::from_str::<Status>(&j).ok()),
                            None => None,
                        };
                        if st.is_none() { proxy = None; }
                        let _ = msg_tx.send(Msg::Status(st));
                    }
                    cmd = cmd_rx.recv() => {
                        let Some(cmd) = cmd else { return };
                        if let Some(p) = &proxy {
                            match cmd {
                                Cmd::Reload => { let _ = p.reload().await; }
                                Cmd::Preview(l) => { let _ = p.preview(l).await; }
                                Cmd::ListWindows => {
                                    if let Ok(j) = p.list_windows().await {
                                        let list = serde_json::from_str(&j).unwrap_or_default();
                                        let _ = msg_tx.send(Msg::Windows(list));
                                    }
                                }
                            }
                        }
                    }
                }
                ctx.request_repaint();
            }
        });
    });
}

// ---------------------------------------------------------------- app ----

struct Editor {
    /// Índice em `cfg.apps`; `None` ao adicionar.
    index: Option<usize>,
    profile: AppProfile,
    last_preview: u8,
}

struct Explorer {
    windows: Vec<WindowInfo>,
    loaded: bool,
}

struct App {
    cfg: Config,
    status: Option<Status>,
    autostart: bool,
    outputs: Vec<OutputInfo>,
    selected: Option<usize>,
    editor: Option<Editor>,
    explorer: Option<Explorer>,
    manual_new: bool,
    toast: Option<(String, Instant, bool)>,
    cmd_tx: tmpsc::UnboundedSender<Cmd>,
    msg_rx: mpsc::Receiver<Msg>,
    rt: tokio::runtime::Runtime,
    /// Mantém o nome D-Bus da instância única enquanto a janela existir.
    _single_instance: Option<zbus::Connection>,
}

impl App {
    fn new(
        ctx: &egui::Context,
        single: Option<zbus::Connection>,
        rt: tokio::runtime::Runtime,
    ) -> Self {
        let (cmd_tx, cmd_rx) = tmpsc::unbounded_channel();
        let (msg_tx, msg_rx) = mpsc::channel();
        spawn_worker(ctx.clone(), cmd_rx, msg_tx);
        let mut app = Self {
            cfg: Config::load(),
            status: None,
            autostart: autostart::is_enabled(),
            outputs: Vec::new(),
            selected: None,
            editor: None,
            explorer: None,
            manual_new: false,
            toast: None,
            cmd_tx,
            msg_rx,
            rt,
            _single_instance: single,
        };
        app.refresh_outputs();
        app
    }

    fn refresh_outputs(&mut self) {
        self.outputs = self.rt.block_on(kscreen::list_outputs()).unwrap_or_default();
    }

    fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some((text.into(), Instant::now(), error));
    }

    fn save(&mut self) {
        self.cfg.sanitize();
        match self.cfg.save() {
            Ok(()) => {
                let _ = self.cmd_tx.send(Cmd::Reload);
            }
            Err(e) => self.notify(format!("Erro ao salvar: {e:#}"), true),
        }
    }

    fn poll_messages(&mut self) {
        while let Ok(m) = self.msg_rx.try_recv() {
            match m {
                Msg::Status(s) => self.status = s,
                Msg::Windows(w) => {
                    if let Some(ex) = &mut self.explorer {
                        ex.windows = w;
                        ex.loaded = true;
                    }
                }
            }
        }
    }

    /// Resoluções disponíveis em qualquer monitor, da maior para a menor.
    fn resolutions(&self) -> Vec<Resolution> {
        let mut v: Vec<Resolution> = self
            .outputs
            .iter()
            .flat_map(|o| o.modes.iter().map(|m| m.resolution()))
            .collect();
        v.sort_by(|a, b| {
            (b.width * b.height, b.refresh).cmp(&(a.width * a.height, a.refresh))
        });
        v.dedup();
        v
    }

    fn open_editor(&mut self, index: Option<usize>, profile: AppProfile) {
        self.refresh_outputs();
        let last_preview = profile.level;
        self.editor = Some(Editor { index, profile, last_preview });
    }

    fn close_editor(&mut self) {
        self.editor = None;
        let _ = self.cmd_tx.send(Cmd::Preview(-1));
    }

    // ----------------------------------------------------------- seções ----

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("vibrance-kde");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (text, color) = match &self.status {
                    None => ("Daemon desligado", Color32::from_rgb(220, 90, 90)),
                    Some(s) if !s.enabled => ("Observador desativado", Color32::from_rgb(200, 160, 60)),
                    Some(s) if !s.kwin_connected => ("KWin sem script de foco", Color32::from_rgb(200, 160, 60)),
                    Some(_) => ("Observador em execução", Color32::from_rgb(70, 190, 100)),
                };
                ui.label(RichText::new(text).color(color));
                let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.5, color);
            });
        });
        if self.status.is_none() {
            ui.label(
                RichText::new("Inicie o programa pelo menu ou execute `vibrance-kde --minimized`. As alterações ficam salvas e valem quando o daemon iniciar.")
                    .small()
                    .weak(),
            );
        }
        if let Some(Status { error: Some(e), .. }) = &self.status {
            ui.colored_label(Color32::from_rgb(220, 90, 90), format!("Erro: {e}"));
        }
    }

    fn general(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            ui.label(RichText::new("Vibrance").strong());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Nível no desktop (fora dos jogos):");
                let r = ui.add(
                    egui::Slider::new(&mut self.cfg.windows_level, LEVEL_NEUTRAL..=LEVEL_MAX).suffix("%"),
                );
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    self.save();
                }
            });
            ui.label(RichText::new("50% = cores normais; 100% = saturação 2x.").small().weak());
            ui.add_space(4.0);

            let mut changed = false;
            let mut on = self.cfg.enabled;
            if ui.checkbox(&mut on, "Ativar vibrance automático por programa").changed() {
                self.cfg.enabled = on;
                changed = true;
            }
            changed |= ui
                .checkbox(&mut self.cfg.affect_primary_only, "Afetar somente o monitor principal")
                .on_hover_text("Os outros monitores nunca são alterados.")
                .changed();
            changed |= ui
                .checkbox(&mut self.cfg.never_switch_resolution, "Nunca trocar a resolução")
                .on_hover_text("Ignora a troca de resolução configurada nos programas.")
                .changed();
            let mut auto = self.autostart;
            if ui
                .checkbox(&mut auto, "Iniciar com o sistema (minimizado na bandeja)")
                .changed()
            {
                match autostart::set(auto) {
                    Ok(()) => self.autostart = autostart::is_enabled(),
                    Err(e) => self.notify(format!("Autostart: {e:#}"), true),
                }
            }
            if changed {
                self.save();
            }
        });
    }

    fn programs(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            ui.label(RichText::new("Programas").strong());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("+ Das janelas abertas…").clicked() {
                    self.explorer = Some(Explorer { windows: Vec::new(), loaded: false });
                    let _ = self.cmd_tx.send(Cmd::ListWindows);
                }
                if ui.button("Adicionar manualmente…").clicked() {
                    self.manual_new = true;
                    let p = AppProfile {
                        name: String::new(),
                        process: String::new(),
                        level: 75,
                        change_resolution: false,
                        resolution: None,
                    };
                    self.open_editor(None, p);
                }
                let sel = self.selected.filter(|&i| i < self.cfg.apps.len());
                if ui.add_enabled(sel.is_some(), egui::Button::new("Editar")).clicked() {
                    if let Some(i) = sel {
                        let p = self.cfg.apps[i].clone();
                        self.open_editor(Some(i), p);
                    }
                }
                if ui.add_enabled(sel.is_some(), egui::Button::new("Remover")).clicked() {
                    if let Some(i) = sel {
                        self.cfg.apps.remove(i);
                        self.selected = None;
                        self.save();
                    }
                }
            });
            ui.add_space(6.0);

            if self.cfg.apps.is_empty() {
                ui.label(
                    RichText::new("Nenhum programa cadastrado. Abra o jogo e use “Das janelas abertas…”.")
                        .weak(),
                );
                return;
            }
            let mut edit: Option<usize> = None;
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                egui::Grid::new("apps").striped(true).num_columns(4).spacing([14.0, 6.0]).show(ui, |ui| {
                    for h in ["Nome", "Processo", "Nível", "Resolução"] {
                        ui.label(RichText::new(h).weak());
                    }
                    ui.end_row();
                    for (i, a) in self.cfg.apps.iter().enumerate() {
                        let r = ui.selectable_label(self.selected == Some(i), &a.name);
                        if r.clicked() {
                            self.selected = Some(i);
                        }
                        if r.double_clicked() {
                            edit = Some(i);
                        }
                        ui.label(&a.process);
                        ui.label(format!("{}%", a.level));
                        ui.label(match (&a.resolution, a.change_resolution) {
                            (Some(r), true) => r.to_string(),
                            _ => "—".to_string(),
                        });
                        ui.end_row();
                    }
                });
            });
            if let Some(i) = edit {
                let p = self.cfg.apps[i].clone();
                self.open_editor(Some(i), p);
            }
        });
    }

    fn monitors(&mut self, ui: &mut egui::Ui) {
        let Some(st) = &self.status else { return };
        if st.outputs.is_empty() {
            return;
        }
        ui.group(|ui| {
            ui.label(RichText::new("Monitores").strong());
            for o in &st.outputs {
                ui.label(format!(
                    "{}{} — {}%",
                    o.name,
                    if o.primary { " (principal)" } else { "" },
                    o.level
                ));
            }
            if let Some(app) = &st.active_app {
                ui.label(RichText::new(format!("Em foco: {app}")).color(Color32::from_rgb(70, 190, 100)));
            }
        });
    }

    // --------------------------------------------------------- diálogos ----

    fn explorer_window(&mut self, ctx: &egui::Context) {
        let Some(ex) = &mut self.explorer else { return };
        let mut open = true;
        let mut chosen: Option<WindowInfo> = None;
        let mut refresh = false;
        let existing: Vec<String> = self.cfg.apps.iter().map(|a| a.process.clone()).collect();

        egui::Window::new("Janelas abertas")
            .open(&mut open)
            .collapsible(false)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Escolha o jogo (deixe-o aberto para aparecer aqui).");
                    if ui.button("Atualizar").clicked() {
                        refresh = true;
                    }
                });
                ui.separator();
                if !ex.loaded {
                    ui.label(RichText::new("Aguardando o daemon…").weak());
                } else if ex.windows.is_empty() {
                    ui.label("Nenhuma janela encontrada. O daemon está rodando e o script do KWin carregado?");
                }
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for w in &ex.windows {
                        let proc = procinfo::suggested_process(w);
                        let already = existing.iter().any(|e| crate::config::names_match(e, &proc));
                        ui.horizontal(|ui| {
                            let title = if w.caption.is_empty() { &w.class } else { &w.caption };
                            ui.vertical(|ui| {
                                ui.label(RichText::new(title).strong());
                                ui.label(RichText::new(format!("processo: {proc}   classe: {}", w.class)).small().weak());
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui
                                    .add_enabled(!already, egui::Button::new(if already { "Já cadastrado" } else { "Adicionar" }))
                                    .clicked()
                                {
                                    chosen = Some(w.clone());
                                }
                            });
                        });
                        ui.separator();
                    }
                });
            });
        if refresh {
            let _ = self.cmd_tx.send(Cmd::ListWindows);
        }
        if let Some(w) = chosen {
            let name = if w.caption.is_empty() { w.class.clone() } else { w.caption.clone() };
            let p = AppProfile {
                name,
                process: procinfo::suggested_process(&w),
                level: 75,
                change_resolution: false,
                resolution: None,
            };
            self.explorer = None;
            self.open_editor(None, p);
        } else if !open {
            self.explorer = None;
        }
    }

    fn editor_window(&mut self, ctx: &egui::Context) {
        let resolutions = self.resolutions();
        let never = self.cfg.never_switch_resolution;
        let Some(ed) = &mut self.editor else { return };
        let mut open = true;
        let mut save = false;
        let mut cancel = false;
        let title = if ed.index.is_some() { "Editar programa" } else { "Novo programa" };

        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                egui::Grid::new("editor").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                    ui.label("Nome:");
                    ui.text_edit_singleline(&mut ed.profile.name);
                    ui.end_row();
                    ui.label("Processo:");
                    ui.text_edit_singleline(&mut ed.profile.process)
                        .on_hover_text("Nome do executável sem .exe (ou a classe da janela, ex.: steam_app_730).");
                    ui.end_row();
                    ui.label("Vibrance no jogo:");
                    ui.add(egui::Slider::new(&mut ed.profile.level, LEVEL_NEUTRAL..=LEVEL_MAX).suffix("%"));
                    ui.end_row();
                });
                ui.label(RichText::new("Mover o controle mostra o efeito ao vivo no monitor.").small().weak());
                ui.separator();
                ui.checkbox(&mut ed.profile.change_resolution, "Trocar a resolução ao entrar no jogo")
                    .on_hover_text("Para jogos em janela ou sem bordas. Jogos em tela cheia já escolhem a própria resolução.");
                if ed.profile.change_resolution {
                    if never {
                        ui.colored_label(
                            Color32::from_rgb(200, 160, 60),
                            "“Nunca trocar a resolução” está ligado e vence esta opção.",
                        );
                    }
                    if resolutions.is_empty() {
                        ui.label(RichText::new("Nenhum modo de vídeo encontrado (kscreen-doctor disponível?).").weak());
                    } else {
                        let cur = ed.profile.resolution.clone();
                        let label = cur.as_ref().map_or("Escolha…".to_string(), |r| r.to_string());
                        egui::ComboBox::from_id_salt("res").selected_text(label).width(240.0).show_ui(ui, |ui| {
                            for r in &resolutions {
                                let selected = cur.as_ref() == Some(r);
                                if ui.selectable_label(selected, r.to_string()).clicked() {
                                    ed.profile.resolution = Some(r.clone());
                                }
                            }
                        });
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let valid = !ed.profile.process.trim().is_empty() && !ed.profile.name.trim().is_empty();
                    if ui.add_enabled(valid, egui::Button::new("Salvar")).clicked() {
                        save = true;
                    }
                    if ui.button("Cancelar").clicked() {
                        cancel = true;
                    }
                });
            });

        // pré-visualização ao vivo
        if ed.profile.level != ed.last_preview {
            ed.last_preview = ed.profile.level;
            let _ = self.cmd_tx.send(Cmd::Preview(i32::from(ed.profile.level)));
        }

        if save {
            if let Some(ed) = self.editor.take() {
                let mut p = ed.profile;
                p.name = p.name.trim().to_string();
                p.process = p.process.trim().to_string();
                match ed.index {
                    Some(i) if i < self.cfg.apps.len() => self.cfg.apps[i] = p,
                    _ => {
                        self.cfg.apps.push(p);
                        self.selected = Some(self.cfg.apps.len() - 1);
                    }
                }
                let _ = self.cmd_tx.send(Cmd::Preview(-1));
                self.save();
            }
        } else if cancel || !open {
            self.close_editor();
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_messages();
        if let Some((_, t, _)) = &self.toast {
            if t.elapsed() > Duration::from_secs(6) {
                self.toast = None;
            }
        }

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.header(ui);
                ui.add_space(8.0);
                self.general(ui);
                ui.add_space(8.0);
                self.programs(ui);
                ui.add_space(8.0);
                self.monitors(ui);
                if let Some((text, _, err)) = &self.toast {
                    ui.add_space(8.0);
                    let color = if *err { Color32::from_rgb(220, 90, 90) } else { Color32::from_rgb(70, 190, 100) };
                    ui.colored_label(color, text);
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Fechar esta janela mantém o vibrance-kde ativo na bandeja do sistema.")
                        .small()
                        .weak(),
                );
            });
        });

        self.explorer_window(&ctx);
        self.editor_window(&ctx);
        ctx.request_repaint_after(Duration::from_millis(1000));
    }
}

/// Abre a janela. Se já houver uma aberta, apenas pede ao daemon para trazê-la à frente.
pub fn run() -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;

    let single = rt.block_on(async {
        let builder = zbus::connection::Builder::session().and_then(|b| b.name(GUI_SERVICE));
        match builder {
            Ok(b) => match b.build().await {
                Ok(c) => Ok(Some(c)),
                Err(zbus::Error::NameTaken) => Err(()),
                Err(_) => Ok(None), // sem barramento: segue sem instância única
            },
            Err(_) => Ok(None),
        }
    });
    let single = match single {
        Ok(s) => s,
        Err(()) => {
            // já existe uma janela: traz ela para a frente (sem acionar o daemon, o que
            // poderia gerar um laço de processos abrindo janelas)
            log::info!("a janela já está aberta");
            rt.block_on(async {
                if let Ok(c) = zbus::Connection::session().await {
                    let _ = crate::kwin::raise_gui_window(&c).await;
                }
            });
            return Ok(());
        }
    };

    let _ = assets::ensure_installed();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("vibrance-kde")
            .with_app_id(assets::APP_ID)
            .with_inner_size([640.0, 720.0])
            .with_min_inner_size([520.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "vibrance-kde",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(&cc.egui_ctx, single, rt)))),
    )
    .map_err(|e| anyhow::anyhow!("falha na interface gráfica: {e}"))
}
