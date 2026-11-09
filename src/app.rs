//! The main window.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Key as K, Layout, Rect, RichText, Sense, Stroke,
    StrokeKind, TextEdit, Ui, Vec2, pos2, vec2,
};
use rusqlite::Connection;

use crate::model::{
    FileEntry, Kind, Snapshot, Source, TAG_PALETTE, Tag, color_to_u32, format_time, human_size,
};
use crate::scanner::{self, Job, ScanMsg};
use crate::theme::{self, ACCENT, BORDER, CARD, CARD_HOVER, DANGER, TEXT, TEXT_DIM};
use crate::thumbs::{self, Lookup, Thumbs};
use crate::i18n::{self, LangChoice, tr, trf};
use crate::{db, smb};

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Grid,
    List,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Name,
    Modified,
    Size,
    Type,
}

impl Sort {
    fn label(self) -> &'static str {
        match self {
            Sort::Name => tr("Name", "Nome"),
            Sort::Modified => tr("Modified", "Modificado"),
            Sort::Size => tr("Size", "Tamanho"),
            Sort::Type => tr("Type", "Tipo"),
        }
    }
    fn key(self) -> &'static str {
        match self {
            Sort::Name => "name",
            Sort::Modified => "modified",
            Sort::Size => "size",
            Sort::Type => "type",
        }
    }
    fn from_key(s: &str) -> Sort {
        match s {
            "modified" => Sort::Modified,
            "size" => Sort::Size,
            "type" => Sort::Type,
            _ => Sort::Name,
        }
    }
}

enum SourceStatus {
    Scanning(u64),
    Ready(u64),
    Error(String),
}

/// Payload carried while dragging files onto a tag.
struct DragFiles(Vec<i64>);

enum Action {
    Click { pos: usize, ctrl: bool, shift: bool },
    Context(usize),
    Open(i64),
    Reveal(i64),
    CopyPaths(Vec<i64>),
    Rename(i64),
    Delete(Vec<i64>),
    TagFiles { tag: i64, files: Vec<i64>, add: bool },
    NewTagFor { name: String, files: Vec<i64> },
    ToggleInclude(i64),
    ToggleExclude(i64),
    RenameTag(i64),
    SetTagColor(i64, Color32),
    DeleteTag(i64),
    Rescan(i64),
    RemoveSource(i64),
    ConnectSource(i64),
}

enum Dialog {
    Smb(SmbDialog),
    RenameFile { id: i64, name: String, error: Option<String> },
    RenameTag { id: i64, name: String },
    NewTag { name: String, color: Color32 },
    ConfirmDelete { ids: Vec<i64> },
    ConfirmDeleteTag { id: i64 },
}

#[derive(Default)]
struct SmbDialog {
    server: String,
    share: String,
    user: String,
    password: String,
    shares: Vec<String>,
    busy: bool,
    error: Option<String>,
    rx: Option<Receiver<SmbResult>>,
    /// Reconnecting an existing source: skip share selection.
    reconnect: Option<i64>,
}

enum SmbResult {
    Shares(Result<Vec<String>, String>),
    Connected(Result<String, String>),
}

pub struct App {
    conn: Connection,
    db_path: PathBuf,

    files: Vec<FileEntry>,
    index_of: HashMap<i64, usize>,
    tags: Vec<Tag>,
    tag_counts: HashMap<i64, usize>,
    untagged_count: usize,
    kind_counts: HashMap<Kind, usize>,
    sources: Vec<Source>,
    source_status: HashMap<i64, SourceStatus>,

    query: String,
    include: BTreeSet<i64>,
    exclude: BTreeSet<i64>,
    untagged_only: bool,
    kind: Option<Kind>,
    sort: Sort,
    sort_desc: bool,
    view: View,
    thumb_px: f32,
    tag_filter: String,

    visible: Vec<usize>,
    dirty: bool,
    selection: HashSet<i64>,
    anchor: Option<usize>,
    scroll_to: Option<usize>,

    scan_tx: Sender<Job>,
    scan_rx: Receiver<ScanMsg>,
    load_tx: Sender<Snapshot>,
    load_rx: Receiver<Snapshot>,
    loading: bool,
    reload_again: bool,
    mutation_gen: u64,
    last_reload: Instant,

    thumbs: Thumbs,
    tag_input: String,
    dialog: Option<Dialog>,
    toast: Option<(String, Instant)>,
    focus_search: bool,
    /// Clipboard text goes through egui's context, which `run` doesn't have.
    pending_copy: Option<String>,
    lang_choice: LangChoice,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme::install(&ctx);

        // TAGEX_DATA lets you run a separate library (handy for testing).
        let data_dir = std::env::var_os("TAGEX_DATA").map(PathBuf::from).unwrap_or_else(|| {
            directories::ProjectDirs::from("", "", "Tagex")
                .map(|d| d.data_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."))
        });
        let _ = std::fs::create_dir_all(&data_dir);
        let db_path = data_dir.join("library.db");
        let conn = db::open(&db_path).expect("open database");

        let (scan_out_tx, scan_rx) = unbounded();
        let scan_tx = scanner::spawn(db_path.clone(), ctx.clone(), scan_out_tx);
        let (load_tx, load_rx) = unbounded();

        let get = |k: &str| db::get_setting(&conn, k);
        let view = if get("view").as_deref() == Some("list") { View::List } else { View::Grid };
        let thumb_px = get("thumb_px").and_then(|s| s.parse().ok()).unwrap_or(150.0f32).clamp(80.0, 320.0);
        let sort = Sort::from_key(&get("sort").unwrap_or_default());
        let sort_desc = get("sort_desc").as_deref() == Some("1");
        let lang_choice = LangChoice::from_key(&get("language").unwrap_or_default());
        i18n::set(lang_choice.resolve());

        let mut app = Self {
            conn,
            db_path,
            files: Vec::new(),
            index_of: HashMap::new(),
            tags: Vec::new(),
            tag_counts: HashMap::new(),
            untagged_count: 0,
            kind_counts: HashMap::new(),
            sources: Vec::new(),
            source_status: HashMap::new(),
            query: String::new(),
            include: BTreeSet::new(),
            exclude: BTreeSet::new(),
            untagged_only: false,
            kind: None,
            sort,
            sort_desc,
            view,
            thumb_px,
            tag_filter: String::new(),
            visible: Vec::new(),
            dirty: true,
            selection: HashSet::new(),
            anchor: None,
            scroll_to: None,
            scan_tx,
            scan_rx,
            load_tx,
            load_rx,
            loading: false,
            reload_again: false,
            mutation_gen: 0,
            last_reload: Instant::now(),
            thumbs: Thumbs::new(ctx),
            tag_input: String::new(),
            dialog: None,
            toast: None,
            focus_search: false,
            pending_copy: None,
            lang_choice,
        };

        // First load is synchronous so the window opens with content already there.
        if let Ok(snap) = db::load_snapshot(&app.conn, 0) {
            app.apply_snapshot(snap);
        }
        // Refresh every location in the background; local disks first, shares after.
        let mut srcs = app.sources.clone();
        srcs.sort_by_key(|s| s.is_network());
        for s in srcs {
            app.queue_scan(s.id, s.path);
        }
        app
    }

    // ---------------------------------------------------------------- data

    fn apply_snapshot(&mut self, snap: Snapshot) {
        self.files = snap.files;
        self.tags = snap.tags;
        self.sources = snap.sources;
        self.index_of = self.files.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
        let index_of = &self.index_of;
        self.selection.retain(|id| index_of.contains_key(id));
        let tag_ids: HashSet<i64> = self.tags.iter().map(|t| t.id).collect();
        self.include.retain(|t| tag_ids.contains(t));
        self.exclude.retain(|t| tag_ids.contains(t));
        self.recount();
        self.dirty = true;
    }

    fn recount(&mut self) {
        self.tag_counts.clear();
        self.untagged_count = 0;
        self.kind_counts.clear();
        for f in &self.files {
            *self.kind_counts.entry(f.kind).or_default() += 1;
            if f.tags.is_empty() {
                self.untagged_count += 1;
            }
            for t in &f.tags {
                *self.tag_counts.entry(*t).or_default() += 1;
            }
        }
    }

    fn request_reload(&mut self) {
        if self.loading {
            self.reload_again = true;
            return;
        }
        self.loading = true;
        self.last_reload = Instant::now();
        let path = self.db_path.clone();
        let tx = self.load_tx.clone();
        let generation = self.mutation_gen;
        std::thread::spawn(move || {
            if let Ok(conn) = db::open(&path) {
                if let Ok(snap) = db::load_snapshot(&conn, generation) {
                    let _ = tx.send(snap);
                }
            }
        });
    }

    fn mutated(&mut self) {
        self.mutation_gen += 1;
    }

    fn queue_scan(&mut self, source: i64, path: String) {
        self.source_status.insert(source, SourceStatus::Scanning(0));
        let _ = self.scan_tx.send(Job { source, path });
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        let mut need_reload = false;
        for msg in self.scan_rx.try_iter().collect::<Vec<_>>() {
            match msg {
                ScanMsg::Progress { source, files } => {
                    self.source_status.insert(source, SourceStatus::Scanning(files));
                    if self.last_reload.elapsed() > Duration::from_secs(3) {
                        need_reload = true;
                    }
                }
                ScanMsg::Done { source, files, error } => {
                    let st = match error {
                        Some(e) => SourceStatus::Error(e),
                        None => SourceStatus::Ready(files),
                    };
                    self.source_status.insert(source, st);
                    need_reload = true;
                }
            }
        }
        if need_reload {
            self.request_reload();
        }

        if let Ok(snap) = self.load_rx.try_recv() {
            self.loading = false;
            if snap.generation == self.mutation_gen {
                self.apply_snapshot(snap);
            } else {
                // A tag edit happened while loading: this snapshot may predate it.
                self.reload_again = true;
            }
            if self.reload_again {
                self.reload_again = false;
                self.request_reload();
            }
        }

        if self.source_status.values().any(|s| matches!(s, SourceStatus::Scanning(_))) || self.loading {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn rebuild_visible(&mut self) {
        let mut name_terms = Vec::new();
        let mut tag_terms = Vec::new();
        for w in self.query.split_whitespace() {
            let w = w.to_lowercase();
            if let Some(t) = w.strip_prefix('#') {
                if !t.is_empty() {
                    tag_terms.push(t.to_string());
                }
            } else {
                name_terms.push(w);
            }
        }
        // Tag-name search terms resolve to sets of tag ids.
        let tag_term_ids: Vec<HashSet<i64>> = tag_terms
            .iter()
            .map(|t| {
                self.tags
                    .iter()
                    .filter(|tag| tag.name.to_lowercase().contains(t.as_str()))
                    .map(|tag| tag.id)
                    .collect()
            })
            .collect();

        let mut vis: Vec<usize> = self
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                if self.untagged_only && !f.tags.is_empty() {
                    return false;
                }
                if let Some(k) = self.kind {
                    if f.kind != k {
                        return false;
                    }
                }
                if !self.include.iter().all(|t| f.has_tag(*t)) {
                    return false;
                }
                if self.exclude.iter().any(|t| f.has_tag(*t)) {
                    return false;
                }
                if !name_terms.iter().all(|w| f.name_lc.contains(w.as_str())) {
                    return false;
                }
                tag_term_ids.iter().all(|ids| f.tags.iter().any(|t| ids.contains(t)))
            })
            .map(|(i, _)| i)
            .collect();

        let files = &self.files;
        match self.sort {
            Sort::Name => vis.sort_by(|a, b| files[*a].name_lc.cmp(&files[*b].name_lc)),
            Sort::Modified => vis.sort_by_key(|i| std::cmp::Reverse(files[*i].mtime)),
            Sort::Size => vis.sort_by_key(|i| std::cmp::Reverse(files[*i].size)),
            Sort::Type => vis.sort_by(|a, b| {
                (&files[*a].ext, &files[*a].name_lc).cmp(&(&files[*b].ext, &files[*b].name_lc))
            }),
        }
        if self.sort_desc {
            vis.reverse();
        }
        self.visible = vis;
        self.anchor = None;
        self.dirty = false;
    }

    fn save_view_settings(&self) {
        db::set_setting(&self.conn, "view", if self.view == View::List { "list" } else { "grid" });
        db::set_setting(&self.conn, "thumb_px", &format!("{}", self.thumb_px.round()));
        db::set_setting(&self.conn, "sort", self.sort.key());
        db::set_setting(&self.conn, "sort_desc", if self.sort_desc { "1" } else { "0" });
    }

    fn file(&self, id: i64) -> Option<&FileEntry> {
        self.index_of.get(&id).map(|i| &self.files[*i])
    }

    fn selected_ids(&self) -> Vec<i64> {
        // Keep the on-screen order for predictable bulk operations.
        let mut v: Vec<i64> = self.selection.iter().copied().collect();
        v.sort_by_key(|id| self.index_of.get(id).copied().unwrap_or(usize::MAX));
        v
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn tag_files(&mut self, tag: i64, ids: &[i64], add: bool) {
        if let Err(e) = db::tag_files(&mut self.conn, tag, ids, add) {
            self.toast(trf(format!("Could not save tags: {e}"), format!("Não foi possível salvar as tags: {e}")));
            return;
        }
        for id in ids {
            if let Some(i) = self.index_of.get(id) {
                let f = &mut self.files[*i];
                if add { f.add_tag(tag) } else { f.remove_tag(tag) };
            }
        }
        self.mutated();
        self.recount();
        if !self.include.is_empty() || !self.exclude.is_empty() || self.untagged_only {
            self.dirty = true;
        }
    }

    fn create_tag(&mut self, name: &str, color: Color32) -> Option<i64> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        if let Some(t) = self.tags.iter().find(|t| t.name.eq_ignore_ascii_case(name)) {
            return Some(t.id);
        }
        match db::create_tag(&self.conn, name, color_to_u32(color)) {
            Ok(id) => {
                self.tags.push(Tag { id, name: name.to_string(), color });
                self.tags.sort_by_key(|t| t.name.to_lowercase());
                self.mutated();
                Some(id)
            }
            Err(e) => {
                self.toast(trf(format!("Could not create tag: {e}"), format!("Não foi possível criar a tag: {e}")));
                None
            }
        }
    }

    fn next_color(&self) -> Color32 {
        TAG_PALETTE[self.tags.len() % TAG_PALETTE.len()]
    }

    fn add_folder_source(&mut self, path: String) {
        match db::add_source(&self.conn, &path) {
            Ok(id) => {
                if !self.sources.iter().any(|s| s.id == id) {
                    self.sources.push(Source { id, path: path.clone() });
                }
                self.queue_scan(id, path);
                self.toast(tr("Indexing location…", "Indexando local…"));
            }
            Err(e) => self.toast(trf(format!("Could not add location: {e}"), format!("Não foi possível adicionar o local: {e}"))),
        }
    }

    // ------------------------------------------------------------- actions

    fn run(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Click { pos, ctrl, shift } => {
                    let Some(&idx) = self.visible.get(pos) else { continue };
                    let id = self.files[idx].id;
                    if shift {
                        let from = self.anchor.unwrap_or(pos);
                        let (a, b) = if from <= pos { (from, pos) } else { (pos, from) };
                        if !ctrl {
                            self.selection.clear();
                        }
                        for p in a..=b {
                            self.selection.insert(self.files[self.visible[p]].id);
                        }
                    } else if ctrl {
                        if !self.selection.remove(&id) {
                            self.selection.insert(id);
                        }
                        self.anchor = Some(pos);
                    } else {
                        self.selection.clear();
                        self.selection.insert(id);
                        self.anchor = Some(pos);
                    }
                }
                Action::Context(pos) => {
                    let Some(&idx) = self.visible.get(pos) else { continue };
                    let id = self.files[idx].id;
                    if !self.selection.contains(&id) {
                        self.selection.clear();
                        self.selection.insert(id);
                        self.anchor = Some(pos);
                    }
                }
                Action::Open(id) => {
                    if let Some(f) = self.file(id) {
                        if let Err(e) = open::that_detached(&f.path) {
                            self.toast(trf(format!("Could not open: {e}"), format!("Não foi possível abrir: {e}")));
                        }
                    }
                }
                Action::Reveal(id) => {
                    if let Some(f) = self.file(id) {
                        use std::os::windows::process::CommandExt;
                        let _ = std::process::Command::new("explorer.exe")
                            .raw_arg(format!("/select,\"{}\"", f.path))
                            .spawn();
                    }
                }
                Action::CopyPaths(ids) => {
                    let text = ids
                        .iter()
                        .filter_map(|id| self.file(*id).map(|f| f.path.clone()))
                        .collect::<Vec<_>>()
                        .join("\r\n");
                    // Copied through the window's clipboard on the next frame.
                    self.pending_copy = Some(text);
                    self.toast(trf(format!("Copied {} path(s)", ids.len()), format!("{} caminho(s) copiado(s)", ids.len())));
                }
                Action::Rename(id) => {
                    if let Some(f) = self.file(id) {
                        self.dialog = Some(Dialog::RenameFile { id, name: f.name.clone(), error: None });
                    }
                }
                Action::Delete(ids) => {
                    if !ids.is_empty() {
                        self.dialog = Some(Dialog::ConfirmDelete { ids });
                    }
                }
                Action::TagFiles { tag, files, add } => self.tag_files(tag, &files, add),
                Action::NewTagFor { name, files } => {
                    let color = self.next_color();
                    if let Some(tag) = self.create_tag(&name, color) {
                        if !files.is_empty() {
                            self.tag_files(tag, &files, true);
                        }
                    }
                }
                Action::ToggleInclude(t) => {
                    self.exclude.remove(&t);
                    if !self.include.remove(&t) {
                        self.include.insert(t);
                    }
                    self.untagged_only = false;
                    self.dirty = true;
                }
                Action::ToggleExclude(t) => {
                    self.include.remove(&t);
                    if !self.exclude.remove(&t) {
                        self.exclude.insert(t);
                    }
                    self.dirty = true;
                }
                Action::RenameTag(id) => {
                    if let Some(t) = self.tags.iter().find(|t| t.id == id) {
                        self.dialog = Some(Dialog::RenameTag { id, name: t.name.clone() });
                    }
                }
                Action::SetTagColor(id, c) => {
                    if let Some(t) = self.tags.iter_mut().find(|t| t.id == id) {
                        t.color = c;
                    }
                    let _ = db::set_tag_color(&self.conn, id, color_to_u32(c));
                    self.mutated();
                }
                Action::DeleteTag(id) => self.dialog = Some(Dialog::ConfirmDeleteTag { id }),
                Action::Rescan(id) => {
                    if let Some(s) = self.sources.iter().find(|s| s.id == id).cloned() {
                        self.queue_scan(s.id, s.path);
                    }
                }
                Action::RemoveSource(id) => {
                    let _ = db::remove_source(&self.conn, id);
                    self.source_status.remove(&id);
                    self.mutated();
                    self.request_reload();
                }
                Action::ConnectSource(id) => {
                    if let Some(s) = self.sources.iter().find(|s| s.id == id) {
                        let mut parts = s.path.trim_start_matches('\\').splitn(2, '\\');
                        let server = parts.next().unwrap_or_default().to_string();
                        let share = parts.next().unwrap_or_default().to_string();
                        self.dialog = Some(Dialog::Smb(SmbDialog {
                            server,
                            share,
                            reconnect: Some(id),
                            ..Default::default()
                        }));
                    }
                }
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_background(&ctx);
        self.thumbs.begin_frame(&ctx);
        if let Some(text) = self.pending_copy.take() {
            ctx.copy_text(text);
        }
        self.handle_dropped_files(&ctx);
        let mut actions = Vec::new();
        self.handle_keys(&ctx, &mut actions);
        if self.dirty {
            self.rebuild_visible();
        }

        egui::Panel::top("top")
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(14, 10)))
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(14, 4)))
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(240.0)
            .min_size(190.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(12)))
            .show(ui, |ui| {
                egui::Panel::bottom("locations")
                    .frame(egui::Frame::new().inner_margin(egui::Margin { top: 8, ..Default::default() }))
                    .show(ui, |ui| self.locations(ui, &mut actions));
                self.sidebar(ui, &mut actions);
            });
        if !self.selection.is_empty() {
            egui::Panel::right("inspector")
                .resizable(true)
                .default_size(290.0)
                .min_size(240.0)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::same(14)))
                .show(ui, |ui| self.inspector(ui, &mut actions));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(10, 6)))
            .show(ui, |ui| {
                if self.visible.is_empty() {
                    self.empty_state(ui);
                } else {
                    match self.view {
                        View::Grid => self.grid(ui, &mut actions),
                        View::List => self.list(ui, &mut actions),
                    }
                }
            });

        self.drag_overlay(&ctx);
        self.dialogs(&ctx, &mut actions);
        self.toast_overlay(&ctx);
        self.run(actions);
    }
}

// ------------------------------------------------------------------ UI parts

impl App {
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        for p in dropped {
            if p.is_dir() {
                self.add_folder_source(p.to_string_lossy().into_owned());
            } else {
                self.toast(tr("Drop folders here to add them as locations", "Solte pastas aqui para adicioná-las como locais"));
            }
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if self.dialog.is_some() {
            return;
        }
        let typing = ctx.text_edit_focused();
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::COMMAND, K::F) {
                self.focus_search = true;
            }
            if typing {
                return;
            }
            if i.consume_key(egui::Modifiers::COMMAND, K::A) {
                self.selection = self.visible.iter().map(|ix| self.files[*ix].id).collect();
            }
            if i.consume_key(egui::Modifiers::NONE, K::Escape) {
                self.selection.clear();
            }
            let sel = self.selected_ids();
            if i.consume_key(egui::Modifiers::NONE, K::Enter) && sel.len() == 1 {
                actions.push(Action::Open(sel[0]));
            }
            if i.consume_key(egui::Modifiers::NONE, K::F2) && sel.len() == 1 {
                actions.push(Action::Rename(sel[0]));
            }
            if i.consume_key(egui::Modifiers::NONE, K::Delete) && !sel.is_empty() {
                actions.push(Action::Delete(sel.clone()));
            }
            // 1-9: toggle the n-th tag on the selection — the fastest way to tag a pile of files.
            // Read from text events: they arrive for real and for injected key presses alike.
            let digits: Vec<usize> = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t) if t.len() == 1 => t.chars().next()?.to_digit(10),
                    _ => None,
                })
                .filter(|d| (1..=9).contains(d))
                .map(|d| d as usize - 1)
                .collect();
            for n in digits {
                if sel.is_empty() {
                    break;
                }
                if let Some(tag) = self.tags.get(n) {
                    let all = sel.iter().all(|id| self.file(*id).is_some_and(|f| f.has_tag(tag.id)));
                    actions.push(Action::TagFiles { tag: tag.id, files: sel.clone(), add: !all });
                }
            }
        });
    }

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("◆").color(ACCENT).size(20.0));
            ui.label(RichText::new("Tagex").strong().size(17.0).color(TEXT));
            ui.add_space(12.0);

            let search = ui.add(
                TextEdit::singleline(&mut self.query)
                    .hint_text(tr("Search files…   #tag to match tag names", "Buscar arquivos…   #tag para buscar por tag"))
                    .desired_width((ui.available_width() - 330.0).clamp(160.0, 520.0))
                    .margin(vec2(10.0, 6.0)),
            );
            if self.focus_search {
                search.request_focus();
                self.focus_search = false;
            }
            if search.changed() {
                self.dirty = true;
            }
            if !self.query.is_empty() && ui.small_button("✕").on_hover_text(tr("Clear search", "Limpar busca")).clicked() {
                self.query.clear();
                self.dirty = true;
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut changed = false;
                ui.menu_button("⚙", |ui| self.settings_menu(ui)).response.on_hover_text(tr("Settings", "Configurações"));
                let grid = ui.selectable_label(self.view == View::Grid, "⊞").on_hover_text(tr("Grid", "Grade"));
                let list = ui.selectable_label(self.view == View::List, "☰").on_hover_text(tr("List", "Lista"));
                if grid.clicked() {
                    self.view = View::Grid;
                    changed = true;
                }
                if list.clicked() {
                    self.view = View::List;
                    changed = true;
                }
                if self.view == View::Grid {
                    let r = ui.add(egui::Slider::new(&mut self.thumb_px, 80.0..=320.0).show_value(false));
                    changed |= r.drag_stopped() || r.changed() && !r.dragged();
                }
                let arrow = if self.sort_desc { "↓" } else { "↑" };
                if ui.button(arrow).on_hover_text(tr("Reverse order", "Inverter ordem")).clicked() {
                    self.sort_desc = !self.sort_desc;
                    self.dirty = true;
                    changed = true;
                }
                egui::ComboBox::from_id_salt("sort")
                    .selected_text(self.sort.label())
                    .width(100.0)
                    .show_ui(ui, |ui| {
                        for s in [Sort::Name, Sort::Modified, Sort::Size, Sort::Type] {
                            if ui.selectable_value(&mut self.sort, s, s.label()).clicked() {
                                self.dirty = true;
                                changed = true;
                            }
                        }
                    });
                if changed {
                    self.save_view_settings();
                }
            });
        });
    }

    fn settings_menu(&mut self, ui: &mut Ui) {
        ui.set_min_width(220.0);
        ui.label(RichText::new(tr("LANGUAGE", "IDIOMA")).small().strong().color(TEXT_DIM));
        let system = match i18n::system_lang() {
            i18n::Lang::Pt => "Português",
            i18n::Lang::En => "English",
        };
        let auto_label = trf(format!("Automatic (Windows: {system})"), format!("Automático (Windows: {system})"));
        let options = [
            (LangChoice::Auto, auto_label),
            (LangChoice::Fixed(i18n::Lang::En), "English".to_string()),
            (LangChoice::Fixed(i18n::Lang::Pt), "Português".to_string()),
        ];
        for (choice, label) in options {
            if ui.radio(self.lang_choice == choice, label).clicked() {
                self.lang_choice = choice;
                i18n::set(choice.resolve());
                db::set_setting(&self.conn, "language", choice.key());
                ui.close();
            }
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let total = self.files.len();
            let shown = self.visible.len();
            let txt = if shown == total {
                trf(format!("{total} files"), format!("{total} arquivos"))
            } else {
                trf(format!("{shown} of {total} files"), format!("{shown} de {total} arquivos"))
            };
            ui.label(RichText::new(txt).color(TEXT_DIM).small());
            if !self.selection.is_empty() {
                let bytes: u64 = self.selection.iter().filter_map(|id| self.file(*id)).map(|f| f.size).sum();
                ui.label(
                    RichText::new(trf(format!("·  {} selected  ·  {}", self.selection.len(), human_size(bytes)), format!("·  {} selecionado(s)  ·  {}", self.selection.len(), human_size(bytes))))
                        .color(TEXT_DIM)
                        .small(),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let scanning: u64 = self
                    .source_status
                    .values()
                    .filter_map(|s| if let SourceStatus::Scanning(n) = s { Some(*n) } else { None })
                    .sum();
                if self.source_status.values().any(|s| matches!(s, SourceStatus::Scanning(_))) {
                    ui.label(RichText::new(trf(format!("Indexing… {scanning} files"), format!("Indexando… {scanning} arquivos"))).color(ACCENT).small());
                    ui.add(egui::Spinner::new().size(12.0));
                } else {
                    ui.label(RichText::new(tr("1–9 toggles tags on the selection", "1–9 aplica/remove tags na seleção")).color(TEXT_DIM).small());
                }
            });
        });
    }

    fn sidebar(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let dragging = egui::DragAndDrop::has_payload_of_type::<DragFiles>(ui.ctx());

        section_title(ui, tr("LIBRARY", "BIBLIOTECA"));
        let all_active = self.include.is_empty() && self.exclude.is_empty() && !self.untagged_only;
        if nav_row(ui, "▤", tr("All files", "Todos os arquivos"), self.files.len(), all_active, ACCENT).clicked() {
            self.include.clear();
            self.exclude.clear();
            self.untagged_only = false;
            self.dirty = true;
        }
        if nav_row(ui, "◌", tr("Untagged", "Sem tag"), self.untagged_count, self.untagged_only, TEXT_DIM).clicked() {
            self.untagged_only = !self.untagged_only;
            if self.untagged_only {
                self.include.clear();
            }
            self.dirty = true;
        }

        ui.add_space(14.0);
        section_title(ui, tr("TYPES", "TIPOS"));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for k in Kind::ALL {
                let n = self.kind_counts.get(&k).copied().unwrap_or(0);
                if n == 0 {
                    continue;
                }
                if chip(ui, &format!("{} {n}", k.label()), self.kind == Some(k), k.color()) {
                    self.kind = if self.kind == Some(k) { None } else { Some(k) };
                    self.dirty = true;
                }
            }
        });

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            section_title(ui, "TAGS");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("+").on_hover_text(tr("New tag", "Nova tag")).clicked() {
                    self.dialog = Some(Dialog::NewTag { name: String::new(), color: self.next_color() });
                }
            });
        });
        if self.tags.len() > 8 {
            ui.add(TextEdit::singleline(&mut self.tag_filter).hint_text(tr("Filter tags…", "Filtrar tags…")).desired_width(f32::INFINITY));
        }
        if dragging {
            ui.label(RichText::new(tr("Drop on a tag to apply it", "Solte numa tag para aplicá-la")).color(ACCENT).small());
        }

        egui::ScrollArea::vertical().id_salt("tags").auto_shrink([false, true]).show(ui, |ui| {
            let filter = self.tag_filter.to_lowercase();
            for (n, tag) in self.tags.iter().enumerate() {
                if !filter.is_empty() && !tag.name.to_lowercase().contains(&filter) {
                    continue;
                }
                let count = self.tag_counts.get(&tag.id).copied().unwrap_or(0);
                let included = self.include.contains(&tag.id);
                let excluded = self.exclude.contains(&tag.id);
                let resp = tag_row(ui, tag, count, included, excluded, if n < 9 { Some(n + 1) } else { None });

                if let Some(payload) = resp.dnd_release_payload::<DragFiles>() {
                    actions.push(Action::TagFiles { tag: tag.id, files: payload.0.clone(), add: true });
                } else if resp.clicked() {
                    if ui.input(|i| i.modifiers.alt) {
                        actions.push(Action::ToggleExclude(tag.id));
                    } else {
                        actions.push(Action::ToggleInclude(tag.id));
                    }
                }
                resp.context_menu(|ui| {
                    if ui.button(if included { tr("Stop filtering", "Parar de filtrar") } else { tr("Show files with this tag", "Mostrar arquivos com esta tag") }).clicked() {
                        actions.push(Action::ToggleInclude(tag.id));
                        ui.close();
                    }
                    if ui.button(if excluded { tr("Stop hiding", "Parar de esconder") } else { tr("Hide files with this tag  (Alt+click)", "Esconder arquivos com esta tag  (Alt+clique)") }).clicked() {
                        actions.push(Action::ToggleExclude(tag.id));
                        ui.close();
                    }
                    if !self.selection.is_empty() {
                        ui.separator();
                        if ui.button(trf(format!("Add to {} selected", self.selection.len()), format!("Adicionar aos {} selecionados", self.selection.len()))).clicked() {
                            actions.push(Action::TagFiles { tag: tag.id, files: self.selected_ids(), add: true });
                            ui.close();
                        }
                        if ui.button(trf(format!("Remove from {} selected", self.selection.len()), format!("Remover dos {} selecionados", self.selection.len()))).clicked() {
                            actions.push(Action::TagFiles { tag: tag.id, files: self.selected_ids(), add: false });
                            ui.close();
                        }
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(tr("Color", "Cor"));
                        let mut c = tag.color;
                        if ui.color_edit_button_srgba(&mut c).changed() {
                            actions.push(Action::SetTagColor(tag.id, c));
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        for c in TAG_PALETTE {
                            let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
                            ui.painter().circle_filled(r.center(), 7.0, c);
                            if resp.clicked() {
                                actions.push(Action::SetTagColor(tag.id, c));
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button(tr("Rename…", "Renomear…")).clicked() {
                        actions.push(Action::RenameTag(tag.id));
                        ui.close();
                    }
                    if ui.button(RichText::new(tr("Delete tag", "Excluir tag")).color(DANGER)).clicked() {
                        actions.push(Action::DeleteTag(tag.id));
                        ui.close();
                    }
                });
            }
            if self.tags.is_empty() {
                ui.label(
                    RichText::new(tr("No tags yet. Select files and type a tag name on the right.", "Nenhuma tag ainda. Selecione arquivos e digite o nome de uma tag à direita."))
                        .color(TEXT_DIM)
                        .small(),
                );
            }
        });

    }

    fn locations(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        section_title(ui, tr("LOCATIONS", "LOCAIS"));
        egui::ScrollArea::vertical().id_salt("sources").max_height(28.0 * 5.0).show(ui, |ui| {
            for s in &self.sources {
                let status = self.source_status.get(&s.id);
                source_row(ui, s, status, actions);
            }
            if self.sources.is_empty() {
                ui.label(RichText::new(tr("Add a folder or a network share to start.", "Adicione uma pasta ou compartilhamento de rede para começar.")).color(TEXT_DIM).small());
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(tr("+ Folder", "+ Pasta")).clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.add_folder_source(dir.to_string_lossy().into_owned());
                }
            }
            if ui.button(tr("+ Network", "+ Rede")).on_hover_text(tr("Add an SMB share", "Adicionar compartilhamento SMB")).clicked() {
                self.dialog = Some(Dialog::Smb(SmbDialog::default()));
            }
        });
    }

    fn empty_state(&mut self, ui: &mut Ui) {
        ui.centered_and_justified(|ui| {
            let msg = if self.sources.is_empty() {
                tr("Add a folder or network share on the left — or drop a folder here.", "Adicione uma pasta ou compartilhamento de rede à esquerda — ou solte uma pasta aqui.")
            } else if self.files.is_empty() {
                tr("Indexing…", "Indexando…")
            } else {
                tr("Nothing matches these filters.", "Nada corresponde a esses filtros.")
            };
            ui.label(RichText::new(msg).color(TEXT_DIM).size(16.0));
        });
    }

    fn grid(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let cell_w = self.thumb_px + 16.0;
        let name_h = 36.0;
        let cell_h = self.thumb_px + name_h + 14.0;
        let gap = 8.0;
        let width = ui.available_width();
        let cols = (((width + gap) / (cell_w + gap)).floor() as usize).max(1);
        // Spread leftover space so the grid fills the row edge to edge.
        let cell_w = ((width - gap * (cols as f32 - 1.0)) / cols as f32).max(cell_w);
        let rows = self.visible.len().div_ceil(cols);

        let mut area = egui::ScrollArea::vertical().auto_shrink(false);
        if let Some(pos) = self.scroll_to.take() {
            let y = (pos / cols) as f32 * (cell_h + gap);
            area = area.vertical_scroll_offset(y);
        }
        ui.spacing_mut().item_spacing = vec2(gap, gap);
        let ctrl_shift = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
        area.show_rows(ui, cell_h, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    for col in 0..cols {
                        let pos = row * cols + col;
                        let Some(&idx) = self.visible.get(pos) else { break };
                        let (rect, resp) = ui.allocate_exact_size(vec2(cell_w, cell_h), Sense::click_and_drag());
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let f = &self.files[idx];
                        let selected = self.selection.contains(&f.id);
                        let p = ui.painter();
                        let bg = if selected {
                            theme::selected_fill()
                        } else if resp.hovered() {
                            CARD_HOVER
                        } else {
                            CARD
                        };
                        p.rect_filled(rect, CornerRadius::same(10), bg);
                        if selected {
                            p.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.5, ACCENT), StrokeKind::Inside);
                        }
                        let thumb_rect = Rect::from_min_size(
                            rect.min + vec2(8.0, 8.0),
                            vec2(cell_w - 16.0, self.thumb_px),
                        );
                        draw_thumb(ui, &mut self.thumbs, f, thumb_rect, true);

                        // Tag dots overlay at the bottom of the thumbnail.
                        let mut x = thumb_rect.left() + 8.0;
                        for t in f.tags.iter().take(8) {
                            if let Some(tag) = self.tags.iter().find(|tg| tg.id == *t) {
                                let c = pos2(x, thumb_rect.bottom() - 8.0);
                                ui.painter().circle(c, 5.0, tag.color, Stroke::new(1.5, Color32::from_black_alpha(160)));
                                x += 12.0;
                            }
                        }

                        let name_rect = Rect::from_min_max(
                            pos2(rect.left() + 8.0, thumb_rect.bottom() + 6.0),
                            pos2(rect.right() - 8.0, rect.bottom() - 4.0),
                        );
                        let mut job = egui::text::LayoutJob::simple(
                            f.name.clone(),
                            FontId::proportional(12.5),
                            TEXT,
                            name_rect.width(),
                        );
                        job.wrap.max_rows = 2;
                        job.wrap.break_anywhere = true;
                        job.halign = Align::Center;
                        let galley = ui.painter().layout_job(job);
                        ui.painter().galley(pos2(name_rect.center().x, name_rect.top()), galley, TEXT);

                        let resp = resp.on_hover_text_at_pointer(hover_text(f, &self.tags));
                        item_interactions(&resp, pos, f.id, ctrl_shift, &self.selection, actions);
                        let sel = if selected { self.selected_ids() } else { vec![f.id] };
                        file_context_menu(&resp, f, &sel, &self.tags, &self.files, &self.index_of, actions);
                    }
                });
            }
        });
    }

    fn list(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let row_h = 30.0;
        let width = ui.available_width();
        let col_size = 90.0;
        let col_date = 130.0;
        let col_tags = (width * 0.25).clamp(120.0, 320.0);
        let col_folder = (width * 0.22).clamp(100.0, 360.0);

        // Header
        ui.horizontal(|ui| {
            ui.add_space(36.0);
            let name_w = width - col_size - col_date - col_tags - col_folder - 60.0;
            for (label, w, sort) in [
                (tr("Name", "Nome"), name_w, Some(Sort::Name)),
                ("Tags", col_tags, None),
                (tr("Location", "Local"), col_folder, None),
                (tr("Size", "Tamanho"), col_size, Some(Sort::Size)),
                (tr("Modified", "Modificado"), col_date, Some(Sort::Modified)),
            ] {
                let active = sort == Some(self.sort);
                let txt = if active {
                    format!("{label} {}", if self.sort_desc { "↓" } else { "↑" })
                } else {
                    label.to_string()
                };
                let r = ui.add_sized(
                    vec2(w, 20.0),
                    egui::Label::new(RichText::new(txt).small().color(if active { ACCENT } else { TEXT_DIM }))
                        .sense(Sense::click()),
                );
                if let (true, Some(s)) = (r.clicked(), sort) {
                    if self.sort == s {
                        self.sort_desc = !self.sort_desc;
                    } else {
                        self.sort = s;
                        self.sort_desc = false;
                    }
                    self.dirty = true;
                    self.save_view_settings();
                }
            }
        });
        ui.separator();

        let mut area = egui::ScrollArea::vertical().auto_shrink(false);
        if let Some(pos) = self.scroll_to.take() {
            area = area.vertical_scroll_offset(pos as f32 * (row_h + 2.0));
        }
        ui.spacing_mut().item_spacing.y = 2.0;
        let ctrl_shift = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
        area.show_rows(ui, row_h, self.visible.len(), |ui, range| {
            for pos in range {
                let idx = self.visible[pos];
                let f = &self.files[idx];
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
                let selected = self.selection.contains(&f.id);
                let bg = if selected {
                    theme::selected_fill()
                } else if resp.hovered() {
                    CARD_HOVER
                } else if pos % 2 == 0 {
                    Color32::TRANSPARENT
                } else {
                    Color32::from_white_alpha(4)
                };
                ui.painter().rect_filled(rect, CornerRadius::same(6), bg);

                let icon_rect = Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), vec2(22.0, 22.0));
                draw_thumb(ui, &mut self.thumbs, f, icon_rect, false);

                let p = ui.painter();
                let mut x = rect.left() + 40.0;
                let name_w = rect.width() - col_size - col_date - col_tags - col_folder - 60.0;
                clip_text(p, pos2(x, rect.center().y), name_w - 8.0, &f.name, 13.5, TEXT);
                x += name_w;

                // Tag chips
                let mut cx = x;
                for t in &f.tags {
                    if let Some(tag) = self.tags.iter().find(|tg| tg.id == *t) {
                        let galley = p.layout_no_wrap(tag.name.clone(), FontId::proportional(11.5), TEXT);
                        let w = galley.size().x + 18.0;
                        if cx + w > x + col_tags - 6.0 {
                            p.text(pos2(cx, rect.center().y), Align2::LEFT_CENTER, "…", FontId::proportional(12.0), TEXT_DIM);
                            break;
                        }
                        let chip = Rect::from_min_size(pos2(cx, rect.center().y - 9.0), vec2(w, 18.0));
                        p.rect_filled(chip, CornerRadius::same(9), tag.color.gamma_multiply(0.25));
                        p.circle_filled(pos2(chip.left() + 7.0, chip.center().y), 3.0, tag.color);
                        p.galley(pos2(chip.left() + 13.0, chip.center().y - galley.size().y / 2.0), galley, TEXT);
                        cx += w + 4.0;
                    }
                }
                x += col_tags + 8.0;
                clip_text(p, pos2(x, rect.center().y), col_folder - 8.0, f.folder(), 12.5, TEXT_DIM);
                x += col_folder + 8.0;
                p.text(pos2(x + col_size - 12.0, rect.center().y), Align2::RIGHT_CENTER, human_size(f.size), FontId::proportional(12.5), TEXT_DIM);
                x += col_size + 8.0;
                p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, format_time(f.mtime), FontId::proportional(12.5), TEXT_DIM);

                item_interactions(&resp, pos, f.id, ctrl_shift, &self.selection, actions);
                let sel = if selected { self.selected_ids() } else { vec![f.id] };
                file_context_menu(&resp, f, &sel, &self.tags, &self.files, &self.index_of, actions);
            }
        });
    }

    fn inspector(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let sel = self.selected_ids();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if sel.len() == 1 {
                let Some(f) = self.file(sel[0]).cloned() else { return };
                let w = ui.available_width();
                let (rect, resp) = ui.allocate_exact_size(vec2(w, w.min(260.0)), Sense::click());
                ui.painter().rect_filled(rect, CornerRadius::same(10), CARD);
                draw_thumb(ui, &mut self.thumbs, &f, rect.shrink(8.0), true);
                if resp.double_clicked() {
                    actions.push(Action::Open(f.id));
                }
                ui.add_space(8.0);
                ui.add(egui::Label::new(RichText::new(&f.name).strong().size(15.0).color(TEXT)).wrap());
                ui.add(egui::Label::new(RichText::new(f.folder()).small().color(TEXT_DIM)).wrap());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(human_size(f.size)).small().color(TEXT_DIM));
                    ui.label(RichText::new("·").small().color(TEXT_DIM));
                    ui.label(RichText::new(format_time(f.mtime)).small().color(TEXT_DIM));
                });
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button(tr("Open", "Abrir")).clicked() {
                        actions.push(Action::Open(f.id));
                    }
                    if ui.button(tr("Show in Explorer", "Mostrar no Explorer")).clicked() {
                        actions.push(Action::Reveal(f.id));
                    }
                    if ui.button(tr("Rename", "Renomear")).clicked() {
                        actions.push(Action::Rename(f.id));
                    }
                });
            } else {
                ui.label(RichText::new(trf(format!("{} files selected", sel.len()), format!("{} arquivos selecionados", sel.len()))).strong().size(15.0).color(TEXT));
                let bytes: u64 = sel.iter().filter_map(|id| self.file(*id)).map(|f| f.size).sum();
                ui.label(RichText::new(human_size(bytes)).small().color(TEXT_DIM));
            }

            ui.add_space(14.0);
            section_title(ui, "TAGS");

            // Tags present on the selection (partial ones drawn faded).
            let mut present: Vec<(Tag, usize)> = Vec::new();
            for tag in &self.tags {
                let n = sel.iter().filter(|id| self.file(**id).is_some_and(|f| f.has_tag(tag.id))).count();
                if n > 0 {
                    present.push((tag.clone(), n));
                }
            }
            ui.horizontal_wrapped(|ui| {
                for (tag, n) in &present {
                    let partial = *n < sel.len();
                    let label = if partial { format!("{}  {}/{}", tag.name, n, sel.len()) } else { tag.name.clone() };
                    let r = tag_chip(ui, &label, tag.color, partial, true);
                    if r.clicked() {
                        actions.push(Action::TagFiles { tag: tag.id, files: sel.clone(), add: false });
                    }
                    let r = r.on_hover_text(if partial { tr("Click to remove · right-click to apply to all", "Clique para remover · botão direito para aplicar a todos") } else { tr("Click to remove", "Clique para remover") });
                    if partial && r.secondary_clicked() {
                        actions.push(Action::TagFiles { tag: tag.id, files: sel.clone(), add: true });
                    }
                }
                if present.is_empty() {
                    ui.label(RichText::new(tr("No tags", "Sem tags")).small().color(TEXT_DIM));
                }
            });

            ui.add_space(10.0);
            let input = ui.add(
                TextEdit::singleline(&mut self.tag_input)
                    .hint_text(tr("Add a tag and press Enter…", "Digite uma tag e aperte Enter…"))
                    .desired_width(f32::INFINITY)
                    .margin(vec2(8.0, 6.0)),
            );
            let entered = input.lost_focus() && ui.input(|i| i.key_pressed(K::Enter));
            let q = self.tag_input.trim().to_lowercase();
            let present_ids: HashSet<i64> =
                present.iter().filter(|(_, n)| *n == sel.len()).map(|(t, _)| t.id).collect();
            let suggestions: Vec<&Tag> = self
                .tags
                .iter()
                .filter(|t| !present_ids.contains(&t.id) && (q.is_empty() || t.name.to_lowercase().contains(&q)))
                .take(24)
                .collect();
            if entered && !q.is_empty() {
                let exact = self.tags.iter().find(|t| t.name.to_lowercase() == q);
                match exact.or(if suggestions.len() == 1 { suggestions.first().copied() } else { None }) {
                    Some(t) => actions.push(Action::TagFiles { tag: t.id, files: sel.clone(), add: true }),
                    None => actions.push(Action::NewTagFor { name: self.tag_input.trim().to_string(), files: sel.clone() }),
                }
                self.tag_input.clear();
                input.request_focus();
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                for t in &suggestions {
                    if tag_chip(ui, &t.name, t.color, true, false).clicked() {
                        actions.push(Action::TagFiles { tag: t.id, files: sel.clone(), add: true });
                    }
                }
                if !q.is_empty() && !self.tags.iter().any(|t| t.name.to_lowercase() == q) {
                    if ui.small_button(trf(format!("+ Create “{}”", self.tag_input.trim()), format!("+ Criar “{}”", self.tag_input.trim()))).clicked() {
                        actions.push(Action::NewTagFor { name: self.tag_input.trim().to_string(), files: sel.clone() });
                        self.tag_input.clear();
                    }
                }
            });

            ui.add_space(18.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button(tr("Copy path", "Copiar caminho")).clicked() {
                    actions.push(Action::CopyPaths(sel.clone()));
                }
                if ui.button(RichText::new(tr("Move to Recycle Bin", "Mover para a Lixeira")).color(DANGER)).clicked() {
                    actions.push(Action::Delete(sel.clone()));
                }
            });
        });
    }

    fn drag_overlay(&self, ctx: &egui::Context) {
        if let Some(p) = egui::DragAndDrop::payload::<DragFiles>(ctx) {
            if let Some(pos) = ctx.pointer_latest_pos() {
                let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("drag")));
                let text = if p.0.len() == 1 { tr("1 file", "1 arquivo").to_string() } else { trf(format!("{} files", p.0.len()), format!("{} arquivos", p.0.len())) };
                let galley = painter.layout_no_wrap(text, FontId::proportional(13.0), Color32::WHITE);
                let rect = Rect::from_min_size(pos + vec2(14.0, 10.0), galley.size() + vec2(20.0, 10.0));
                painter.rect_filled(rect, CornerRadius::same(8), ACCENT);
                painter.galley(rect.min + vec2(10.0, 5.0), galley, Color32::WHITE);
            }
        }
    }

    fn toast_overlay(&mut self, ctx: &egui::Context) {
        let Some((msg, at)) = &self.toast else { return };
        if at.elapsed() > Duration::from_millis(2600) {
            self.toast = None;
            return;
        }
        egui::Area::new(egui::Id::unique("toast"))
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -40.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x2a, 0x2e, 0x3a))
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .show(ui, |ui| ui.label(RichText::new(msg.as_str()).color(TEXT)));
            });
        ctx.request_repaint_after(Duration::from_millis(200));
    }

    fn dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let mut keep = true;
        let modal = egui::Modal::new(egui::Id::unique("dialog")).show(ctx, |ui| {
            ui.set_width(380.0);
            match &mut dialog {
                Dialog::Smb(d) => keep = self.smb_dialog(ui, d),
                Dialog::RenameFile { id, name, error } => {
                    ui.heading(tr("Rename file", "Renomear arquivo"));
                    ui.add_space(6.0);
                    let r = ui.add(TextEdit::singleline(name).desired_width(f32::INFINITY));
                    r.request_focus();
                    if let Some(e) = error {
                        ui.label(RichText::new(e.as_str()).color(DANGER).small());
                    }
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(K::Enter));
                    let (ok, cancel) = buttons(ui, tr("Rename", "Renomear"));
                    if ok || enter {
                        match self.rename_file(*id, name.trim()) {
                            Ok(()) => keep = false,
                            Err(e) => *error = Some(e),
                        }
                    }
                    if cancel {
                        keep = false;
                    }
                }
                Dialog::RenameTag { id, name } => {
                    ui.heading(tr("Rename tag", "Renomear tag"));
                    ui.add_space(6.0);
                    let r = ui.add(TextEdit::singleline(name).desired_width(f32::INFINITY));
                    r.request_focus();
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(K::Enter));
                    let (ok, cancel) = buttons(ui, tr("Rename", "Renomear"));
                    if (ok || enter) && !name.trim().is_empty() {
                        match db::rename_tag(&self.conn, *id, name.trim()) {
                            Ok(()) => {
                                if let Some(t) = self.tags.iter_mut().find(|t| t.id == *id) {
                                    t.name = name.trim().to_string();
                                }
                                self.tags.sort_by_key(|t| t.name.to_lowercase());
                                self.mutated();
                                keep = false;
                            }
                            Err(_) => self.toast(tr("A tag with that name already exists", "Já existe uma tag com esse nome")),
                        }
                    }
                    if cancel {
                        keep = false;
                    }
                }
                Dialog::NewTag { name, color } => {
                    ui.heading(tr("New tag", "Nova tag"));
                    ui.add_space(6.0);
                    let r = ui.horizontal(|ui| {
                        ui.color_edit_button_srgba(color);
                        ui.add(TextEdit::singleline(name).hint_text(tr("Tag name", "Nome da tag")).desired_width(f32::INFINITY))
                    });
                    r.inner.request_focus();
                    let enter = r.inner.lost_focus() && ui.input(|i| i.key_pressed(K::Enter));
                    ui.horizontal_wrapped(|ui| {
                        for c in TAG_PALETTE {
                            let (rr, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                            ui.painter().circle_filled(rr.center(), 8.0, c);
                            if *color == c {
                                ui.painter().circle_stroke(rr.center(), 9.0, Stroke::new(1.5, Color32::WHITE));
                            }
                            if resp.clicked() {
                                *color = c;
                            }
                        }
                    });
                    let (ok, cancel) = buttons(ui, tr("Create", "Criar"));
                    if (ok || enter) && !name.trim().is_empty() {
                        let sel = self.selected_ids();
                        if let Some(tag) = self.create_tag(name, *color) {
                            if !sel.is_empty() {
                                actions.push(Action::TagFiles { tag, files: sel, add: true });
                            }
                        }
                        keep = false;
                    }
                    if cancel {
                        keep = false;
                    }
                }
                Dialog::ConfirmDelete { ids } => {
                    ui.heading(tr("Move to Recycle Bin?", "Mover para a Lixeira?"));
                    ui.add_space(6.0);
                    let label = if ids.len() == 1 {
                        self.file(ids[0]).map(|f| f.name.clone()).unwrap_or_default()
                    } else {
                        trf(format!("{} files", ids.len()), format!("{} arquivos", ids.len()))
                    };
                    ui.label(label);
                    let (ok, cancel) = buttons(ui, tr("Delete", "Excluir"));
                    if ok {
                        let ids = ids.clone();
                        self.delete_files(&ids);
                        keep = false;
                    }
                    if cancel {
                        keep = false;
                    }
                }
                Dialog::ConfirmDeleteTag { id } => {
                    let name = self.tags.iter().find(|t| t.id == *id).map(|t| t.name.clone()).unwrap_or_default();
                    ui.heading(tr("Delete tag?", "Excluir tag?"));
                    ui.add_space(6.0);
                    ui.label(trf(format!("“{name}” will be removed from all files. The files stay where they are."), format!("“{name}” será removida de todos os arquivos. Os arquivos continuam onde estão.")));
                    let (ok, cancel) = buttons(ui, tr("Delete tag", "Excluir tag"));
                    if ok {
                        let _ = db::delete_tag(&self.conn, *id);
                        let id = *id;
                        self.tags.retain(|t| t.id != id);
                        for f in &mut self.files {
                            f.remove_tag(id);
                        }
                        self.include.remove(&id);
                        self.exclude.remove(&id);
                        self.mutated();
                        self.recount();
                        self.dirty = true;
                        keep = false;
                    }
                    if cancel {
                        keep = false;
                    }
                }
            }
        });
        if modal.should_close() {
            keep = false;
        }
        if keep {
            self.dialog = Some(dialog);
        }
    }

    fn smb_dialog(&mut self, ui: &mut Ui, d: &mut SmbDialog) -> bool {
        // Collect background results first.
        if let Some(rx) = &d.rx {
            if let Ok(res) = rx.try_recv() {
                d.busy = false;
                d.rx = None;
                match res {
                    SmbResult::Shares(Ok(list)) => {
                        if d.share.is_empty() {
                            if let Some(first) = list.first() {
                                d.share = first.clone();
                            }
                        }
                        d.shares = list;
                        d.error = None;
                    }
                    SmbResult::Shares(Err(e)) => d.error = Some(trf(format!("Could not list shares: {e}"), format!("Não foi possível listar os compartilhamentos: {e}"))),
                    SmbResult::Connected(Ok(unc)) => {
                        match d.reconnect {
                            Some(id) => actions_rescan(self, id),
                            None => self.add_folder_source(unc),
                        }
                        return false;
                    }
                    SmbResult::Connected(Err(e)) => d.error = Some(e),
                }
            }
        }

        ui.heading(if d.reconnect.is_some() { tr("Connect to share", "Conectar ao compartilhamento") } else { tr("Add network share", "Adicionar compartilhamento de rede") });
        ui.label(RichText::new(tr("SMB / Windows file sharing (NAS, other PCs, Samba).", "SMB / compartilhamento do Windows (NAS, outros PCs, Samba).")).small().color(TEXT_DIM));
        ui.add_space(8.0);
        egui::Grid::new("smb").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
            ui.label(tr("Server", "Servidor"));
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut d.server).hint_text(tr("nas  or  192.168.0.10", "nas  ou  192.168.0.10")).desired_width(200.0));
                if ui.add_enabled(!d.busy && !d.server.trim().is_empty(), egui::Button::new(tr("List shares", "Listar compartilhamentos"))).clicked() {
                    let server = smb::normalize_unc(&d.server);
                    let (user, pass) = (d.user.clone(), d.password.clone());
                    let (tx, rx) = unbounded();
                    d.rx = Some(rx);
                    d.busy = true;
                    let ctx = ui.ctx().clone();
                    std::thread::spawn(move || {
                        // Authenticate against IPC$ first so listing works on password-protected servers.
                        if !user.is_empty() {
                            let _ = smb::connect(&format!("{server}\\IPC$"), &user, &pass);
                        }
                        let _ = tx.send(SmbResult::Shares(smb::list_shares(&server)));
                        ctx.request_repaint();
                    });
                }
            });
            ui.end_row();
            ui.label(tr("Share", "Compartilhamento"));
            ui.horizontal(|ui| {
                if d.shares.is_empty() {
                    ui.add(TextEdit::singleline(&mut d.share).hint_text(tr("folder name", "nome da pasta")).desired_width(200.0));
                } else {
                    egui::ComboBox::from_id_salt("shares").selected_text(d.share.clone()).width(200.0).show_ui(ui, |ui| {
                        for s in &d.shares {
                            ui.selectable_value(&mut d.share, s.clone(), s);
                        }
                    });
                }
            });
            ui.end_row();
            ui.label(tr("User", "Usuário"));
            ui.add(TextEdit::singleline(&mut d.user).hint_text(tr("optional", "opcional")).desired_width(200.0));
            ui.end_row();
            ui.label(tr("Password", "Senha"));
            ui.add(TextEdit::singleline(&mut d.password).password(true).hint_text(tr("optional", "opcional")).desired_width(200.0));
            ui.end_row();
        });
        ui.label(
            RichText::new(tr("Leave user/password empty to use your Windows login or saved credentials.", "Deixe usuário/senha vazios para usar seu login do Windows ou credenciais salvas."))
                .small()
                .color(TEXT_DIM),
        );
        if let Some(e) = &d.error {
            ui.add_space(4.0);
            ui.label(RichText::new(e.as_str()).color(DANGER).small());
        }
        if d.busy {
            ui.add(egui::Spinner::new());
        }
        let (ok, cancel) = buttons(ui, tr("Connect", "Conectar"));
        if ok && !d.busy && !d.server.trim().is_empty() && !d.share.trim().is_empty() {
            let unc = format!("{}\\{}", smb::normalize_unc(&d.server), d.share.trim().trim_matches('\\'));
            let (user, pass) = (d.user.clone(), d.password.clone());
            let (tx, rx) = unbounded();
            d.rx = Some(rx);
            d.busy = true;
            d.error = None;
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let res = smb::connect(&unc, &user, &pass).and_then(|_| {
                    std::fs::read_dir(&unc).map(|_| unc.clone()).map_err(|e| trf(format!("Connected, but cannot read the share: {e}"), format!("Conectado, mas não foi possível ler o compartilhamento: {e}")))
                });
                let _ = tx.send(SmbResult::Connected(res));
                ctx.request_repaint();
            });
        }
        !cancel
    }

    fn rename_file(&mut self, id: i64, new_name: &str) -> Result<(), String> {
        if new_name.is_empty() || new_name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) {
            return Err(tr("Invalid file name", "Nome de arquivo inválido").into());
        }
        let Some(&i) = self.index_of.get(&id) else { return Err(tr("File not found", "Arquivo não encontrado").into()) };
        let old = self.files[i].path.clone();
        let folder = self.files[i].folder().to_string();
        let new_path = format!("{folder}\\{new_name}");
        if new_path == old {
            return Ok(());
        }
        std::fs::rename(&old, &new_path).map_err(|e| e.to_string())?;
        let ext = match new_name.rfind('.') {
            Some(p) if p > 0 => new_name[p + 1..].to_lowercase(),
            _ => String::new(),
        };
        db::rename_file(&self.conn, id, &new_path, new_name, &ext).map_err(|e| e.to_string())?;
        let f = &mut self.files[i];
        f.path = new_path;
        f.name = new_name.to_string();
        f.name_lc = new_name.to_lowercase();
        f.kind = Kind::from_ext(&ext);
        f.ext = ext;
        self.thumbs.forget(&thumbs::Key::File(id));
        self.mutated();
        self.dirty = true;
        Ok(())
    }

    fn delete_files(&mut self, ids: &[i64]) {
        let paths: Vec<String> = ids.iter().filter_map(|id| self.file(*id).map(|f| f.path.clone())).collect();
        let mut done = Vec::new();
        let mut failed = 0;
        for (id, path) in ids.iter().zip(&paths) {
            // Network shares have no Recycle Bin; trash returns an error there and we keep the file.
            match trash::delete(path) {
                Ok(()) => done.push(*id),
                Err(_) => failed += 1,
            }
        }
        if !done.is_empty() {
            let _ = db::delete_files(&mut self.conn, &done);
            let gone: HashSet<i64> = done.iter().copied().collect();
            self.files.retain(|f| !gone.contains(&f.id));
            self.index_of = self.files.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
            self.selection.retain(|id| !gone.contains(id));
            self.mutated();
            self.recount();
            self.dirty = true;
        }
        if failed > 0 {
            self.toast(trf(format!("{failed} file(s) could not be moved to the Recycle Bin"), format!("{failed} arquivo(s) não puderam ir para a Lixeira")));
        } else {
            self.toast(trf(format!("Moved {} file(s) to the Recycle Bin", done.len()), format!("{} arquivo(s) movido(s) para a Lixeira", done.len())));
        }
    }
}

fn actions_rescan(app: &mut App, id: i64) {
    app.run(vec![Action::Rescan(id)]);
}

// ------------------------------------------------------------- widgets

fn section_title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().strong().color(TEXT_DIM));
}

fn chip(ui: &mut Ui, label: &str, active: bool, color: Color32) -> bool {
    let font = FontId::proportional(12.5);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, if active { Color32::WHITE } else { TEXT_DIM });
    let size = galley.size() + vec2(20.0, 10.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if active {
        color.gamma_multiply(0.55)
    } else if resp.hovered() {
        CARD_HOVER
    } else {
        CARD
    };
    ui.painter().rect_filled(rect, CornerRadius::same(14), fill);
    ui.painter().galley(rect.min + vec2(10.0, 5.0), galley, TEXT);
    resp.clicked()
}

fn nav_row(ui: &mut Ui, icon: &str, label: &str, count: usize, active: bool, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    let p = ui.painter();
    if active {
        p.rect_filled(rect, CornerRadius::same(6), theme::selected_fill());
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(6), CARD_HOVER);
    }
    p.text(pos2(rect.left() + 10.0, rect.center().y), Align2::LEFT_CENTER, icon, FontId::proportional(14.0), color);
    p.text(pos2(rect.left() + 32.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(14.0), TEXT);
    p.text(pos2(rect.right() - 8.0, rect.center().y), Align2::RIGHT_CENTER, count.to_string(), FontId::proportional(12.0), TEXT_DIM);
    resp
}

fn tag_row(ui: &mut Ui, tag: &Tag, count: usize, included: bool, excluded: bool, hotkey: Option<usize>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    let hovering_drop = resp.dnd_hover_payload::<DragFiles>().is_some();
    let p = ui.painter();
    if hovering_drop {
        p.rect_filled(rect, CornerRadius::same(6), tag.color.gamma_multiply(0.35));
        p.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.5, tag.color), StrokeKind::Inside);
    } else if included {
        p.rect_filled(rect, CornerRadius::same(6), tag.color.gamma_multiply(0.22));
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(6), CARD_HOVER);
    }
    let dot = pos2(rect.left() + 16.0, rect.center().y);
    if included {
        p.circle_filled(dot, 6.0, tag.color);
    } else {
        p.circle(dot, 5.0, tag.color.gamma_multiply(0.85), Stroke::NONE);
    }
    let text_color = if excluded { TEXT_DIM } else { TEXT };
    let name_pos = pos2(rect.left() + 32.0, rect.center().y);
    let max_w = rect.width() - 32.0 - 56.0;
    let r = clip_text(p, name_pos, max_w, &tag.name, 14.0, text_color);
    if excluded {
        p.line_segment([pos2(r.left(), r.center().y), pos2(r.right(), r.center().y)], Stroke::new(1.5, DANGER));
    }
    p.text(pos2(rect.right() - 8.0, rect.center().y), Align2::RIGHT_CENTER, count.to_string(), FontId::proportional(12.0), TEXT_DIM);
    if let (Some(n), true) = (hotkey, resp.hovered()) {
        p.text(pos2(rect.right() - 40.0, rect.center().y), Align2::RIGHT_CENTER, n.to_string(), FontId::monospace(11.0), TEXT_DIM.gamma_multiply(0.7));
    }
    resp.on_hover_text(tr("Click to filter · Alt+click to hide · drop files to tag", "Clique para filtrar · Alt+clique para esconder · solte arquivos para etiquetar"))
}

fn tag_chip(ui: &mut Ui, label: &str, color: Color32, faded: bool, removable: bool) -> egui::Response {
    let font = FontId::proportional(12.5);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), TEXT);
    let extra = if removable { 16.0 } else { 0.0 };
    let size = vec2(galley.size().x + 26.0 + extra, 24.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    let alpha = if faded { 0.14 } else { 0.30 };
    let fill = if resp.hovered() { color.gamma_multiply(alpha + 0.12) } else { color.gamma_multiply(alpha) };
    p.rect_filled(rect, CornerRadius::same(12), fill);
    p.circle_filled(pos2(rect.left() + 11.0, rect.center().y), 4.0, color);
    p.galley(pos2(rect.left() + 20.0, rect.center().y - galley.size().y / 2.0), galley, TEXT);
    if removable {
        p.text(pos2(rect.right() - 11.0, rect.center().y), Align2::CENTER_CENTER, "×", font, if resp.hovered() { TEXT } else { TEXT_DIM });
    }
    resp
}

fn source_row(ui: &mut Ui, s: &Source, status: Option<&SourceStatus>, actions: &mut Vec<Action>) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(6), CARD_HOVER);
    }
    let icon = if s.is_network() { "🖧" } else { "🖴" };
    let (dot, tip) = match status {
        Some(SourceStatus::Scanning(n)) => (ACCENT, trf(format!("Indexing… {n} files"), format!("Indexando… {n} arquivos"))),
        Some(SourceStatus::Ready(n)) => (Color32::from_rgb(0x4c, 0xc9, 0xa4), trf(format!("{n} files"), format!("{n} arquivos"))),
        Some(SourceStatus::Error(e)) => (DANGER, format!("Offline: {e}")),
        None => (TEXT_DIM, String::new()),
    };
    p.text(pos2(rect.left() + 8.0, rect.center().y), Align2::LEFT_CENTER, icon, FontId::proportional(13.0), TEXT_DIM);
    let name = s.path.trim_end_matches('\\');
    clip_text(p, pos2(rect.left() + 28.0, rect.center().y), rect.width() - 46.0, name, 12.5, TEXT);
    p.circle_filled(pos2(rect.right() - 10.0, rect.center().y), 3.5, dot);
    let resp = resp.on_hover_text(format!("{}\n{tip}", s.path));
    resp.context_menu(|ui| {
        if ui.button(tr("Rescan", "Reindexar")).clicked() {
            actions.push(Action::Rescan(s.id));
            ui.close();
        }
        if s.is_network() && ui.button(tr("Connect / log in…", "Conectar / entrar…")).clicked() {
            actions.push(Action::ConnectSource(s.id));
            ui.close();
        }
        if ui.button(tr("Open in Explorer", "Abrir no Explorer")).clicked() {
            let _ = open::that_detached(&s.path);
            ui.close();
        }
        ui.separator();
        if ui.button(RichText::new(tr("Remove location", "Remover local")).color(DANGER)).on_hover_text(tr("Forgets the files and their tags. Nothing is deleted from disk.", "Esquece os arquivos e suas tags. Nada é apagado do disco.")).clicked() {
            actions.push(Action::RemoveSource(s.id));
            ui.close();
        }
    });
}

/// Draws single-line text clipped with an ellipsis; returns the rect actually used.
fn clip_text(p: &egui::Painter, left_center: egui::Pos2, max_w: f32, text: &str, size: f32, color: Color32) -> Rect {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), FontId::proportional(size), color);
    job.wrap.max_width = max_w.max(10.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = p.layout_job(job);
    let pos = pos2(left_center.x, left_center.y - galley.size().y / 2.0);
    let r = Rect::from_min_size(pos, galley.size());
    p.galley(pos, galley, color);
    r
}

fn buttons(ui: &mut Ui, ok_label: &str) -> (bool, bool) {
    ui.add_space(12.0);
    let mut ok = false;
    let mut cancel = false;
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ok = ui.add(egui::Button::new(RichText::new(ok_label).color(Color32::WHITE)).fill(ACCENT)).clicked();
        cancel = ui.button(tr("Cancel", "Cancelar")).clicked() || ui.input(|i| i.key_pressed(K::Escape));
    });
    (ok, cancel)
}

fn hover_text(f: &FileEntry, tags: &[Tag]) -> String {
    let names: Vec<&str> = f.tags.iter().filter_map(|t| tags.iter().find(|x| x.id == *t)).map(|t| t.name.as_str()).collect();
    let mut s = format!("{}\n{}  ·  {}", f.path, human_size(f.size), format_time(f.mtime));
    if !names.is_empty() {
        s.push_str(&format!("\n# {}", names.join(", ")));
    }
    s
}

fn item_interactions(
    resp: &egui::Response,
    pos: usize,
    id: i64,
    (ctrl, shift): (bool, bool),
    selection: &HashSet<i64>,
    actions: &mut Vec<Action>,
) {
    if resp.double_clicked() {
        actions.push(Action::Open(id));
    } else if resp.clicked() {
        actions.push(Action::Click { pos, ctrl, shift });
    }
    if resp.secondary_clicked() {
        actions.push(Action::Context(pos));
    }
    if resp.drag_started() {
        let ids: Vec<i64> = if selection.contains(&id) {
            selection.iter().copied().collect()
        } else {
            actions.push(Action::Click { pos, ctrl: false, shift: false });
            vec![id]
        };
        resp.dnd_set_drag_payload(DragFiles(ids));
    }
}

fn file_context_menu(
    resp: &egui::Response,
    f: &FileEntry,
    sel: &[i64],
    tags: &[Tag],
    files: &[FileEntry],
    index_of: &HashMap<i64, usize>,
    actions: &mut Vec<Action>,
) {
    resp.context_menu(|ui| {
        ui.set_min_width(200.0);
        if ui.button(tr("Open", "Abrir")).clicked() {
            actions.push(Action::Open(f.id));
            ui.close();
        }
        if ui.button(tr("Show in Explorer", "Mostrar no Explorer")).clicked() {
            actions.push(Action::Reveal(f.id));
            ui.close();
        }
        if ui.button(tr("Copy path", "Copiar caminho")).clicked() {
            actions.push(Action::CopyPaths(sel.to_vec()));
            ui.close();
        }
        if sel.len() == 1 && ui.button(tr("Rename…", "Renomear…")).clicked() {
            actions.push(Action::Rename(f.id));
            ui.close();
        }
        ui.separator();
        ui.menu_button("Tags", |ui| {
            for t in tags {
                let n = sel.iter().filter(|id| index_of.get(id).is_some_and(|i| files[*i].has_tag(t.id))).count();
                let mut on = n == sel.len();
                let label = RichText::new(format!("●  {}", t.name)).color(if n > 0 { t.color } else { TEXT_DIM });
                if ui.checkbox(&mut on, label).clicked() {
                    actions.push(Action::TagFiles { tag: t.id, files: sel.to_vec(), add: on });
                }
            }
            if tags.is_empty() {
                ui.label(RichText::new(tr("No tags yet", "Nenhuma tag ainda")).color(TEXT_DIM));
            }
        });
        ui.separator();
        if ui.button(RichText::new(tr("Move to Recycle Bin", "Mover para a Lixeira")).color(DANGER)).clicked() {
            actions.push(Action::Delete(sel.to_vec()));
            ui.close();
        }
    });
}

/// Paints the thumbnail (or file-type icon) of `f` fitted inside `rect`.
fn draw_thumb(ui: &Ui, thumbs: &mut Thumbs, f: &FileEntry, rect: Rect, big: bool) {
    let per_file_icon = matches!(f.ext.as_str(), "exe" | "lnk" | "ico" | "url" | "msi" | "appref-ms");
    let decodable = matches!(f.ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff");

    let mut tex = None;
    let mut pending = false;
    let mut is_icon = true;
    if big && f.kind.has_thumbnail() {
        match thumbs.get(thumbs::Key::File(f.id), &f.path, f.size, f.mtime, true, decodable) {
            Lookup::Ready(t) => {
                tex = Some(t.clone());
                is_icon = false;
            }
            Lookup::Pending => pending = true,
            Lookup::Failed => {}
        }
    }
    if tex.is_none() && !pending {
        let key = if per_file_icon { thumbs::Key::File(-f.id) } else { thumbs::Key::Ext(f.ext.clone()) };
        match thumbs.get(key, &f.path, f.size, f.mtime, false, false) {
            Lookup::Ready(t) => tex = Some(t.clone()),
            Lookup::Pending => pending = true,
            Lookup::Failed => {}
        }
    }

    let p = ui.painter();
    match tex {
        Some(t) => {
            let size = t.size_vec2();
            let max = if big && is_icon { rect.size().min(Vec2::splat(rect.height() * 0.55)) } else { rect.size() };
            let scale = (max.x / size.x).min(max.y / size.y);
            let scale = if is_icon { scale.min(1.0).max(max.x.min(max.y) / size.x.max(size.y)) } else { scale };
            let draw = Rect::from_center_size(rect.center(), size * scale);
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            if big && !is_icon {
                // Rounded corners: a soft frame under the image.
                p.rect_filled(draw.expand(1.0), CornerRadius::same(6), Color32::from_black_alpha(60));
            }
            p.image(t.id(), draw, uv, Color32::WHITE);
            if big && is_icon && !f.ext.is_empty() {
                ext_badge(p, rect, f);
            }
        }
        None => {
            // Placeholder while loading / when nothing is available.
            let c = f.kind.color();
            let r = if big { rect.shrink(rect.width().min(rect.height()) * 0.18) } else { rect };
            p.rect_filled(r, CornerRadius::same(if big { 10 } else { 4 }), c.gamma_multiply(if pending { 0.12 } else { 0.22 }));
            if big {
                let ext = if f.ext.is_empty() { tr("file", "arquivo").to_string() } else { f.ext.to_uppercase() };
                p.text(r.center(), Align2::CENTER_CENTER, ext, FontId::proportional(16.0), c);
            }
        }
    }
}

fn ext_badge(p: &egui::Painter, rect: Rect, f: &FileEntry) {
    let text = f.ext.to_uppercase();
    let galley = p.layout_no_wrap(text, FontId::proportional(10.5), Color32::WHITE);
    let r = Rect::from_min_size(
        pos2(rect.right() - galley.size().x - 16.0, rect.top() + 6.0),
        galley.size() + vec2(10.0, 4.0),
    );
    p.rect_filled(r, CornerRadius::same(4), f.kind.color().gamma_multiply(0.8));
    p.galley(r.min + vec2(5.0, 2.0), galley, Color32::WHITE);
}
