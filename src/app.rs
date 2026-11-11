use crate::{
    connection::{History, SavedConnection, manual_url},
    db::{Database, QueryResult, TableName},
    redis_browser::{self, BrowserEntry, BrowserSnapshot, KeyFilter, KeySort},
    schema::TableView,
};
use anyhow::{Result, bail};
use crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers as M, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{prelude::*, widgets::*};
use ratatui_textarea::{CursorMove, TextArea};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const ACCENT: Color = Color::Rgb(96, 215, 174);
const BG: Color = Color::Rgb(18, 23, 32);
const MUTED: Color = Color::Rgb(140, 155, 175);
const CELL_WIDTH: usize = 25;
type Db = Arc<Mutex<Database>>;

enum Message {
    Connected(Result<(Database, Vec<TableName>)>, SavedConnection),
    Queried(Result<QueryResult>),
    Tables(Result<(Vec<TableName>, String, BrowserSnapshot)>),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Url,
    Manual,
    History,
    Password,
}
struct ConnectionForm {
    mode: Mode,
    fields: Vec<TextArea<'static>>,
    focus: usize,
    selected: usize,
    history_state: ListState,
    field_rects: Vec<Rect>,
    tabs: Vec<Rect>,
    action: Rect,
    list: Rect,
}
fn input(value: &str, secret: bool) -> TextArea<'static> {
    let mut area = TextArea::from([value.to_string()]);
    area.set_cursor_line_style(Style::default());
    area.set_style(Style::default().fg(Color::White).bg(BG));
    if secret {
        area.set_mask_char('•');
    }
    area
}
impl ConnectionForm {
    fn new(has_history: bool) -> Self {
        let mut form = Self {
            mode: Mode::Url,
            fields: vec![],
            focus: 1,
            selected: 0,
            history_state: ListState::default(),
            field_rects: vec![],
            tabs: vec![],
            action: Rect::default(),
            list: Rect::default(),
        };
        form.set_mode(if has_history {
            Mode::History
        } else {
            Mode::Url
        });
        form
    }
    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.focus = 1;
        self.fields = match mode {
            Mode::Url => vec![input("", true)],
            Mode::Manual => ["postgres", "localhost", "", "", "", ""]
                .iter()
                .enumerate()
                .map(|(i, v)| input(v, i == 5))
                .collect(),
            Mode::History => vec![],
            Mode::Password => vec![input("", true)],
        };
    }
    fn slots(&self) -> usize {
        if self.mode == Mode::History {
            3
        } else {
            self.fields.len() + 2
        }
    }
    fn cycle_mode(&mut self, backwards: bool) {
        let n = match self.mode {
            Mode::Url => 0,
            Mode::Manual => 1,
            _ => 2,
        };
        self.set_mode(
            [Mode::Url, Mode::Manual, Mode::History][(n + if backwards { 2 } else { 1 }) % 3],
        );
    }
    fn value(&self, i: usize) -> &str {
        &self.fields[i].lines()[0]
    }
}

pub struct App {
    history: History,
    clipboard: crate::clipboard::TextClipboard,
    form: ConnectionForm,
    connecting: bool,
    db: Option<Db>,
    label: String,
    tables: Vec<TableName>,
    table_state: ListState,
    table_scroll: usize,
    table_x: usize,
    selected_table: usize,
    editor: TextArea<'static>,
    result: QueryResult,
    focus: usize,
    view: TableView,
    view_tabs: Vec<Rect>,
    redis: bool,
    filtering: bool,
    redis_filter: KeyFilter,
    filter_draft: KeyFilter,
    ttl_min: TextArea<'static>,
    ttl_max: TextArea<'static>,
    filter_focus: usize,
    filter_rects: Vec<Rect>,
    redis_controls: Vec<Rect>,
    snapshot: BrowserSnapshot,
    tree: bool,
    namespace: String,
    key_sort: KeySort,
    descending: bool,
    find_value: bool,
    value_find: TextArea<'static>,
    value_query: String,
    unfiltered_result: Option<QueryResult>,
    redis_key_info: String,
    redis_document: Option<String>,
    key_filter: TextArea<'static>,
    key_pattern: String,
    detail: Option<String>,
    detail_y: u16,
    detail_limit: u16,
    active_object: String,
    row: usize,
    col: usize,
    y: usize,
    x: usize,
    status: String,
    busy: bool,
    help: bool,
    quit: bool,
    tx: mpsc::UnboundedSender<Message>,
    rx: mpsc::UnboundedReceiver<Message>,
    panes: [Rect; 3],
    buttons: [Rect; 4],
    grid: Rect,
    vbar: Rect,
    hbar: Rect,
}
impl App {
    pub fn new(history: History) -> Self {
        let form = ConnectionForm::new(!history.entries.is_empty());
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            history,
            clipboard: crate::clipboard::TextClipboard::default(),
            form,
            connecting: true,
            db: None,
            label: String::new(),
            tables: vec![],
            table_state: ListState::default(),
            table_scroll: 0,
            table_x: 0,
            selected_table: 0,
            editor: input("SELECT 1 AS hello;", false),
            result: QueryResult::default(),
            focus: 1,
            view: TableView::Records,
            view_tabs: vec![],
            redis: false,
            filtering: false,
            redis_filter: KeyFilter::default(),
            filter_draft: KeyFilter::default(),
            ttl_min: input("", false),
            ttl_max: input("", false),
            filter_focus: 0,
            filter_rects: vec![],
            redis_controls: vec![],
            snapshot: BrowserSnapshot::default(),
            tree: false,
            namespace: String::new(),
            key_sort: KeySort::Name,
            descending: false,
            find_value: false,
            value_find: input("", false),
            value_query: String::new(),
            unfiltered_result: None,
            redis_key_info: String::new(),
            redis_document: None,
            key_filter: input("*", false),
            key_pattern: "*".into(),
            detail: None,
            detail_y: 0,
            detail_limit: 0,
            active_object: String::new(),
            row: 0,
            col: 0,
            y: 0,
            x: 0,
            status: "Welcome. Choose a connection method. F1 for help.".into(),
            busy: false,
            help: false,
            quit: false,
            tx,
            rx,
            panes: [Rect::default(); 3],
            buttons: [Rect::default(); 4],
            grid: Rect::default(),
            vbar: Rect::default(),
            hbar: Rect::default(),
        }
    }
    pub fn should_quit(&self) -> bool {
        self.quit
    }
    pub fn poll(&mut self) {
        while let Ok(message) = self.rx.try_recv() {
            self.busy = false;
            match message {
                Message::Connected(result, saved) => match result {
                    Ok((db, tables)) => {
                        let redis = db.is_redis();
                        if redis != self.redis || redis {
                            self.editor =
                                input(if redis { "PING" } else { "SELECT 1 AS hello;" }, false);
                        }
                        self.redis = redis;
                        self.view = TableView::Records;
                        self.active_object.clear();
                        self.key_filter = input("*", false);
                        self.key_pattern = "*".into();
                        self.snapshot = db.redis_snapshot();
                        self.redis_filter = KeyFilter::default();
                        self.namespace.clear();
                        self.selected_table = 0;
                        self.value_query.clear();
                        self.unfiltered_result = None;
                        self.redis_document = None;
                        self.redis_key_info.clear();
                        self.detail = None;
                        let summary = db.browser_summary();
                        self.db = Some(Arc::new(Mutex::new(db)));
                        self.tables = tables;
                        self.selected_table = 0;
                        self.table_scroll = 0;
                        self.table_x = 0;
                        self.table_state = ListState::default();
                        self.result = QueryResult::default();
                        self.row = 0;
                        self.col = 0;
                        self.y = 0;
                        self.x = 0;
                        self.label = saved.url.clone();
                        self.connecting = false;
                        self.form = ConnectionForm::new(true); // Discard in-memory password input.
                        self.status = match self.history.remember(saved) {
                            Ok(()) => {
                                if self.redis {
                                    format!("Connected. {summary}")
                                } else {
                                    "Connected. Enter previews a table; F6 changes view; F5 executes SQL.".into()
                                }
                            }
                            Err(e) => format!("Connected; could not save history: {e}"),
                        };
                    }
                    Err(e) => self.status = format!("Connection failed: {e}"),
                },
                Message::Queried(result) => match result {
                    Ok(r) => {
                        self.status = format!(
                            "{} rows · {} affected · {} ms · {} statement(s){}",
                            r.rows.len(),
                            r.affected,
                            r.elapsed_ms,
                            r.statements,
                            if r.truncated {
                                " · preview limited"
                            } else {
                                ""
                            }
                        );
                        self.unfiltered_result = None;
                        self.value_query.clear();
                        self.result = r;
                        if self.redis && !self.active_object.is_empty() {
                            self.prepare_redis_inspector();
                        }
                        self.row = 0;
                        self.col = 0;
                        self.x = 0;
                        self.y = 0;
                        self.focus = 2;
                    }
                    Err(e) => {
                        self.status =
                            format!("Query failed: {e}. Earlier statements may have executed.")
                    }
                },
                Message::Tables(result) => match result {
                    Ok((t, summary, snapshot)) => {
                        let old_label =
                            self.browser_entries()
                                .get(self.selected_table)
                                .and_then(|e| {
                                    if let BrowserEntry::Key(k) = e {
                                        Some(k.label.clone())
                                    } else {
                                        None
                                    }
                                });
                        self.tables = t;
                        self.snapshot = snapshot;
                        if self.redis {
                            self.selected_table = old_label
                                .and_then(|name| {
                                    self.browser_entries().iter().position(
                                        |e| matches!(e,BrowserEntry::Key(k) if k.label==name),
                                    )
                                })
                                .unwrap_or(0);
                        }
                        self.selected_table = self
                            .selected_table
                            .min(self.browser_len().saturating_sub(1));
                        self.status = if self.redis {
                            summary
                        } else {
                            "Table list refreshed.".into()
                        };
                    }
                    Err(e) => self.status = format!("Cannot list tables: {e}"),
                },
            }
        }
    }
    fn connect(&mut self) -> Result<()> {
        if self.busy {
            return Ok(());
        }
        let raw = match self.form.mode {
            Mode::Url => self.form.value(0).trim().to_string(),
            Mode::Manual => manual_url(
                self.form.value(0).trim(),
                self.form.value(1).trim(),
                self.form.value(2).trim(),
                self.form.value(3).trim(),
                self.form.value(4),
                self.form.value(5),
            )?,
            Mode::History => {
                let saved = self.history.entries.get(self.form.selected).cloned();
                let Some(saved) = saved else {
                    bail!("No saved connections yet");
                };
                if saved.needs_password {
                    self.form.set_mode(Mode::Password);
                    self.status = "Enter the password for the selected connection.".into();
                    return Ok(());
                }
                saved.url
            }
            Mode::Password => self
                .history
                .entries
                .get(self.form.selected)
                .ok_or_else(|| anyhow::anyhow!("Select a saved connection"))?
                .with_password(self.form.value(0))?,
        };
        let saved = SavedConnection::from_url(&raw)?;
        self.busy = true;
        self.status = "Connecting… (15 second timeout)".into();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let outcome = tokio::time::timeout(Duration::from_secs(15), async {
                let mut db = Database::connect(&raw).await?;
                let tables = db.tables().await?;
                Ok((db, tables))
            })
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("Connection timed out")));
            let _ = tx.send(Message::Connected(outcome, saved));
        });
        Ok(())
    }
    fn run_query(&mut self) {
        if self.busy {
            return;
        }
        let Some(db) = self.db.clone() else {
            return;
        };
        self.view = TableView::Records;
        self.active_object.clear();
        self.redis_key_info.clear();
        self.redis_document = None;
        let sql = self.editor.lines().join("\n");
        self.busy = true;
        self.status =
            "Executing… You can keep editing. Results will show the submitted query.".into();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = db.lock().await.execute(&sql).await;
            let _ = tx.send(Message::Queried(result));
        });
    }
    fn refresh(&mut self) {
        self.refresh_scan(false);
    }
    fn refresh_scan(&mut self, restart: bool) {
        if self.busy {
            return;
        }
        let Some(db) = self.db.clone() else {
            return;
        };
        self.busy = true;
        self.status = if self.redis {
            "Scanning keys…"
        } else {
            "Refreshing tables…"
        }
        .into();
        let mut filter = self.redis_filter.clone();
        filter.pattern = self.key_pattern.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut db = db.lock().await;
            let result = match db.configure_redis(filter, restart) {
                Ok(()) => db
                    .tables()
                    .await
                    .map(|tables| (tables, db.browser_summary(), db.redis_snapshot())),
                Err(e) => Err(e),
            };
            let _ = tx.send(Message::Tables(result));
        });
    }
    fn preview(&mut self) {
        if self.busy {
            return;
        }
        let selected = if self.redis {
            match self.browser_entries().get(self.selected_table).cloned() {
                Some(BrowserEntry::Folder(prefix, _)) => {
                    self.namespace = prefix;
                    self.selected_table = 0;
                    self.table_state = ListState::default();
                    return;
                }
                Some(BrowserEntry::Key(key)) => Some(TableName {
                    schema: String::new(),
                    name: key.label,
                }),
                None => None,
            }
        } else {
            self.tables.get(self.selected_table).cloned()
        };
        let Some(table) = selected else {
            self.status = "Select a table or key first.".into();
            return;
        };
        if !self.redis && self.view == TableView::Records {
            self.editor = input(&table.preview_sql(self.label.starts_with("mysql:")), false);
            self.run_query();
            self.active_object = table.label();
            return;
        }
        let Some(db) = self.db.clone() else {
            return;
        };
        self.active_object = table.label();
        self.busy = true;
        self.status = format!(
            "Loading {} · {}…",
            self.active_object,
            if self.redis {
                "Value"
            } else {
                self.view.label()
            }
        );
        self.result = QueryResult::default();
        self.row = 0;
        self.col = 0;
        self.x = 0;
        self.y = 0;
        let tx = self.tx.clone();
        let view = self.view;
        tokio::spawn(async move {
            let result = db.lock().await.inspect(&table, view).await;
            let _ = tx.send(Message::Queried(result));
        });
    }
    fn show_view(&mut self, view: TableView) {
        if self.busy || self.redis {
            return;
        }
        if self.tables.is_empty() {
            self.status = "Select a table first.".into();
            return;
        }
        self.view = view;
        self.preview();
    }
    pub fn paste(&mut self, value: &str) {
        if self.help || self.detail.is_some() {
            return;
        }
        if self.find_value {
            self.value_find.insert_str(value.replace(['\r', '\n'], ""));
        } else if self.filtering {
            let clean = value.replace(['\r', '\n'], "");
            match self.filter_focus {
                0 => {
                    self.key_filter.insert_str(clean);
                }
                3 => {
                    self.ttl_min.insert_str(clean);
                }
                4 => {
                    self.ttl_max.insert_str(clean);
                }
                _ => {}
            }
        } else if self.connecting {
            if let Some(field) = self.form.fields.get_mut(self.form.focus.wrapping_sub(1)) {
                field.insert_str(value.replace(['\r', '\n'], ""));
            }
        } else if self.focus == 1 {
            self.editor.insert_str(value);
        }
    }
    fn active_text(&mut self) -> Option<&mut TextArea<'static>> {
        if self.help || self.detail.is_some() {
            return None;
        }
        if self.find_value {
            return Some(&mut self.value_find);
        }
        if self.filtering {
            return match self.filter_focus {
                0 => Some(&mut self.key_filter),
                3 => Some(&mut self.ttl_min),
                4 => Some(&mut self.ttl_max),
                _ => None,
            };
        }
        if self.connecting {
            return self.form.fields.get_mut(self.form.focus.wrapping_sub(1));
        }
        if self.focus == 1 {
            Some(&mut self.editor)
        } else {
            None
        }
    }
    fn can_paste(&self) -> bool {
        if self.help || self.detail.is_some() {
            return false;
        }
        if self.find_value {
            return true;
        }
        if self.filtering {
            return matches!(self.filter_focus, 0 | 3 | 4);
        }
        if self.connecting {
            return self
                .form
                .fields
                .get(self.form.focus.wrapping_sub(1))
                .is_some();
        }
        self.focus == 1
    }
    fn selected_result_text(&self) -> Option<String> {
        if self.redis
            && let Some(document) = &self.redis_document
        {
            return Some(document.clone());
        }
        self.result
            .rows
            .get(self.row)
            .and_then(|row| row.get(self.col))
            .cloned()
    }
    fn copy_text(&mut self) -> Option<String> {
        if self.help {
            return None;
        }
        if let Some(text) = &self.detail {
            return Some(text.clone());
        }
        if self.find_value {
            return Some(selected_text(&mut self.value_find));
        }
        if self.filtering {
            return match self.filter_focus {
                0 => Some(selected_text(&mut self.key_filter)),
                3 => Some(selected_text(&mut self.ttl_min)),
                4 => Some(selected_text(&mut self.ttl_max)),
                _ => None,
            };
        }
        if self.connecting {
            return self
                .form
                .fields
                .get_mut(self.form.focus.wrapping_sub(1))
                .map(selected_text);
        }
        match self.focus {
            0 => {
                if self.redis {
                    self.browser_entries()
                        .get(self.selected_table)
                        .map(|e| match e {
                            BrowserEntry::Folder(name, _) => name.clone(),
                            BrowserEntry::Key(key) => key.name.clone(),
                        })
                } else {
                    self.tables.get(self.selected_table).map(TableName::label)
                }
            }
            1 => Some(selected_text(&mut self.editor)),
            2 => self.selected_result_text(),
            _ => None,
        }
    }
    pub fn key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('q') && key.modifiers.contains(M::CONTROL) {
            self.quit = true;
            return;
        }
        if key.modifiers.contains(M::CONTROL)
            && matches!(key.code, KeyCode::Char('a' | 'A' | 'z' | 'Z' | 'y' | 'Y'))
            && let Some(text) = self.active_text()
        {
            match key.code {
                KeyCode::Char('a' | 'A') => text.select_all(),
                KeyCode::Char('z' | 'Z') => {
                    text.undo();
                }
                _ => {
                    text.redo();
                }
            }
            return;
        }
        if key.modifiers.contains(M::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'C')) {
            match self.copy_text() {
                Some(text) => {
                    let chars = text.chars().count();
                    let system = self.clipboard.copy(text);
                    self.status = format!(
                        "Copied {chars} characters to {} clipboard.",
                        if system {
                            "system"
                        } else {
                            "internal (headless)"
                        }
                    );
                }
                None => self.status = "Select a cell, key, or text field to copy.".into(),
            }
            return;
        }
        if key.modifiers.contains(M::CONTROL) && matches!(key.code, KeyCode::Char('v' | 'V')) {
            if !self.can_paste() {
                self.status = "Focus an editor or filter text field to paste.".into();
                return;
            }
            match self.clipboard.paste() {
                Ok(text) => {
                    self.paste(&text);
                    self.status = "Pasted from clipboard.".into();
                }
                Err(e) => self.status = e.to_string(),
            }
            return;
        }
        if key.code == KeyCode::F(1) {
            self.help = !self.help;
            return;
        }
        if self.help {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                self.help = false;
            }
            return;
        }
        if self.connecting {
            self.form_key(key);
            return;
        }
        if self.detail.is_some() {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => self.detail = None,
                KeyCode::Up => self.detail_y = self.detail_y.saturating_sub(1),
                KeyCode::Down => self.detail_y = (self.detail_y + 1).min(self.detail_limit),
                KeyCode::PageUp => self.detail_y = self.detail_y.saturating_sub(10),
                KeyCode::PageDown => {
                    self.detail_y = self.detail_y.saturating_add(10).min(self.detail_limit)
                }
                KeyCode::Home => self.detail_y = 0,
                KeyCode::End => self.detail_y = self.detail_limit,
                _ => {}
            }
            return;
        }
        if self.find_value {
            match key.code {
                KeyCode::Esc => self.find_value = false,
                KeyCode::Enter => {
                    self.find_value = false;
                    self.apply_value_search();
                }
                _ => {
                    self.value_find.input(key);
                }
            }
            return;
        }
        if self.filtering {
            self.filter_key(key);
            return;
        }
        match key.code {
            KeyCode::F(4) if self.redis && !self.busy => {
                self.open_filters();
            }
            KeyCode::F(3) if self.redis && key.modifiers.contains(M::SHIFT) => {
                self.refresh_scan(true)
            }
            KeyCode::F(6) if self.redis => {
                self.tree = !self.tree;
                self.namespace.clear();
                self.selected_table = 0;
                self.table_state = ListState::default();
            }
            KeyCode::F(7) if self.redis => {
                self.key_sort = self.key_sort.next();
                self.selected_table = 0;
            }
            KeyCode::F(8) if self.redis => {
                self.descending = !self.descending;
                self.selected_table = 0;
            }
            KeyCode::F(9) if self.redis => {
                self.find_value = true;
                self.value_find = input(&self.value_query, false);
            }
            KeyCode::Backspace if self.redis && self.focus == 0 && self.tree => self.namespace_up(),
            KeyCode::F(6) if !self.redis => {
                let i = TableView::ALL
                    .iter()
                    .position(|v| *v == self.view)
                    .unwrap_or(0);
                self.show_view(
                    TableView::ALL[(i + if key.modifiers.contains(M::SHIFT) {
                        4
                    } else {
                        1
                    }) % 5],
                );
            }
            KeyCode::Char(c @ '1'..='5')
                if !self.redis && (self.focus != 1 || key.modifiers.contains(M::ALT)) =>
            {
                self.show_view(TableView::ALL[c as usize - '1' as usize])
            }
            KeyCode::F(2) if !self.busy => {
                self.connecting = true;
                self.status = "Choose another connection. Esc returns to the workspace.".into();
            }
            KeyCode::F(3) => self.refresh(),
            KeyCode::F(5) => self.run_query(),
            KeyCode::Enter if key.modifiers.contains(M::CONTROL) => self.run_query(),
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = (self.focus
                    + if key.code == KeyCode::BackTab || key.modifiers.contains(M::SHIFT) {
                        2
                    } else {
                        1
                    })
                    % 3;
            }
            KeyCode::Esc => self.focus = 0,
            KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
                if key.modifiers.intersects(M::SHIFT | M::CONTROL | M::ALT) =>
            {
                let step = if key.modifiers.intersects(M::CONTROL | M::ALT) {
                    10
                } else {
                    1
                };
                let (dy, dx) = direction(key.code, step);
                self.scroll(self.focus.min(2), dy, dx);
            }
            _ => match self.focus {
                0 => match key.code {
                    KeyCode::Up => self.selected_table = self.selected_table.saturating_sub(1),
                    KeyCode::Down => {
                        self.selected_table =
                            (self.selected_table + 1).min(self.browser_len().saturating_sub(1))
                    }
                    KeyCode::Enter => self.preview(),
                    KeyCode::Left | KeyCode::Right if !self.redis => {
                        let i = TableView::ALL
                            .iter()
                            .position(|v| *v == self.view)
                            .unwrap_or(0);
                        self.show_view(
                            TableView::ALL[(i + if key.code == KeyCode::Left { 4 } else { 1 }) % 5],
                        );
                    }
                    _ => {}
                },
                1 => {
                    self.editor.input(key);
                }
                2 => match key.code {
                    KeyCode::Up => {
                        self.row = self.row.saturating_sub(1);
                        self.reveal_cell();
                    }
                    KeyCode::Down => {
                        self.row = (self.row + 1).min(self.result.rows.len().saturating_sub(1));
                        self.reveal_cell();
                    }
                    KeyCode::Left => {
                        self.col = self.col.saturating_sub(1);
                        self.reveal_cell();
                    }
                    KeyCode::Right => {
                        self.col = (self.col + 1).min(self.result.columns.len().saturating_sub(1));
                        self.reveal_cell();
                    }
                    KeyCode::PageDown => self.scroll(2, self.grid.height as i16, 0),
                    KeyCode::PageUp => self.scroll(2, -(self.grid.height as i16), 0),
                    KeyCode::Home => {
                        self.row = 0;
                        self.y = 0;
                        self.col = 0;
                        self.x = 0;
                    }
                    KeyCode::End => {
                        self.row = self.result.rows.len().saturating_sub(1);
                        self.reveal_cell();
                    }
                    KeyCode::Enter => {
                        self.detail = self.selected_result_text();
                        self.detail_y = 0;
                    }
                    _ => {}
                },
                _ => {}
            },
        }
    }
    fn form_key(&mut self, key: KeyEvent) {
        if self.busy {
            return;
        }
        let f = &mut self.form;
        match key.code {
            KeyCode::Esc => {
                if self.db.is_some() {
                    self.connecting = false;
                } else if f.mode == Mode::Password {
                    f.set_mode(Mode::History);
                }
            }
            KeyCode::F(2) => f.cycle_mode(false),
            KeyCode::Tab | KeyCode::BackTab => {
                f.focus = (f.focus
                    + if key.code == KeyCode::BackTab || key.modifiers.contains(M::SHIFT) {
                        f.slots() - 1
                    } else {
                        1
                    })
                    % f.slots()
            }
            KeyCode::Left | KeyCode::Right if f.focus == 0 => {
                f.cycle_mode(key.code == KeyCode::Left);
                f.focus = 0;
            }
            KeyCode::Left | KeyCode::Right if f.mode == Mode::Manual && f.focus == 1 => {
                let drivers = [
                    "postgres", "mysql", "sqlite", "redis", "rediss", "valkey", "keydb",
                ];
                let i = drivers.iter().position(|v| *v == f.value(0)).unwrap_or(0);
                f.fields[0] = input(
                    drivers[(i + if key.code == KeyCode::Left {
                        drivers.len() - 1
                    } else {
                        1
                    }) % drivers.len()],
                    false,
                );
            }
            KeyCode::Enter if f.focus == 0 => f.focus = 1,
            KeyCode::Enter => {
                if f.focus == f.slots() - 1 || f.mode != Mode::Manual || f.focus == f.fields.len() {
                    if let Err(e) = self.connect() {
                        self.status = e.to_string();
                    }
                } else {
                    f.focus += 1;
                }
            }
            KeyCode::Up if f.mode == Mode::History => f.selected = f.selected.saturating_sub(1),
            KeyCode::Down if f.mode == Mode::History => {
                f.selected = (f.selected + 1).min(self.history.entries.len().saturating_sub(1))
            }
            KeyCode::Up => f.focus = f.focus.saturating_sub(1),
            KeyCode::Down => f.focus = (f.focus + 1).min(f.slots() - 1),
            _ => {
                if let Some(field) = f.fields.get_mut(f.focus.wrapping_sub(1)) {
                    field.input(key);
                }
            }
        }
    }
    fn max_y(&self) -> usize {
        self.result
            .rows
            .len()
            .saturating_sub(self.grid.height.saturating_sub(1) as usize)
    }
    fn column_widths(&self) -> Vec<usize> {
        let mut widths = vec![CELL_WIDTH; self.result.columns.len()];
        if self.redis
            && let Some(last) = widths.last_mut()
        {
            let i = self.result.columns.len() - 1;
            let content = self
                .result
                .rows
                .iter()
                .filter_map(|r| r.get(i))
                .map(|v| v.width() + 2)
                .max()
                .unwrap_or(CELL_WIDTH)
                .min(1024);
            *last = content
                .max((self.grid.width as usize).saturating_sub(i * CELL_WIDTH))
                .max(CELL_WIDTH);
        }
        widths
    }
    fn max_x(&self) -> usize {
        self.column_widths()
            .iter()
            .sum::<usize>()
            .saturating_sub(self.grid.width as usize)
    }
    fn reveal_cell(&mut self) {
        let height = self.grid.height.saturating_sub(1).max(1) as usize;
        if self.row < self.y {
            self.y = self.row;
        }
        if self.row >= self.y + height {
            self.y = self.row + 1 - height;
        }
        let width = self.grid.width.max(1) as usize;
        if self.col * CELL_WIDTH < self.x {
            self.x = self.col * CELL_WIDTH;
        }
        if (self.col + 1) * CELL_WIDTH > self.x + width {
            self.x = ((self.col + 1) * CELL_WIDTH).saturating_sub(width);
        }
        self.x = self.x.min(self.max_x());
        self.y = self.y.min(self.max_y());
    }
    fn scroll(&mut self, pane: usize, dy: i16, dx: i16) {
        match pane {
            0 => {
                self.table_scroll = self
                    .table_state
                    .offset()
                    .saturating_add_signed(dy as isize)
                    .min(
                        self.browser_len()
                            .saturating_sub(self.panes[0].height.saturating_sub(2) as usize),
                    );
                *self.table_state.offset_mut() = self.table_scroll;
                let h = self.panes[0].height.saturating_sub(2).max(1) as usize;
                self.selected_table = self
                    .selected_table
                    .clamp(self.table_scroll, self.table_scroll + h - 1)
                    .min(self.browser_len().saturating_sub(1));
                let content_width = if self.redis {
                    self.browser_entries()
                        .iter()
                        .map(|e| e.label().width())
                        .max()
                        .unwrap_or(0)
                } else {
                    self.tables
                        .iter()
                        .map(|t| t.label().width())
                        .max()
                        .unwrap_or(0)
                };
                let max =
                    content_width.saturating_sub(self.panes[0].width.saturating_sub(4) as usize);
                self.table_x = self.table_x.saturating_add_signed(dx as isize).min(max);
            }
            1 => self.editor.scroll((dy, dx)),
            _ => {
                self.y = self.y.saturating_add_signed(dy as isize).min(self.max_y());
                self.x = self.x.saturating_add_signed(dx as isize).min(self.max_x());
            }
        }
    }
    pub fn mouse(&mut self, mouse: MouseEvent) {
        if self.help {
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.help = false;
            }
            return;
        }
        let point = Position::new(mouse.column, mouse.row);
        if self.find_value {
            return;
        }
        if self.filtering {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                && let Some(i) = self.filter_rects.iter().position(|r| r.contains(point))
            {
                self.filter_focus = i;
                match i {
                    1 => self.cycle_filter_type(false),
                    2 => self.filter_draft.expiry = self.filter_draft.expiry.next(),
                    5 => self.apply_filters(),
                    6 => self.reset_filters(),
                    _ => {}
                }
            }
            return;
        }
        if self.redis
            && !self.connecting
            && mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(i) = self.redis_controls.iter().position(|r| r.contains(point))
        {
            match i {
                0..=2 => {
                    self.open_filters();
                    self.filter_focus = i;
                }
                3 => self.open_filters(),
                4 => self.reset_filters(),
                5 => self.refresh(),
                6 => self.refresh_scan(true),
                7 => {
                    self.tree = !self.tree;
                    self.namespace.clear();
                    self.selected_table = 0;
                }
                8 => {
                    self.key_sort = self.key_sort.next();
                    self.selected_table = 0;
                }
                9 => {
                    self.descending = !self.descending;
                    self.selected_table = 0;
                }
                10 => {
                    self.find_value = true;
                    self.value_find = input(&self.value_query, false);
                }
                _ => {}
            }
            return;
        }
        if self.detail.is_some() {
            match mouse.kind {
                MouseEventKind::ScrollUp => self.detail_y = self.detail_y.saturating_sub(3),
                MouseEventKind::ScrollDown => {
                    self.detail_y = self.detail_y.saturating_add(3).min(self.detail_limit)
                }
                MouseEventKind::Down(MouseButton::Left) => self.detail = None,
                _ => {}
            }
            return;
        }
        if !self.connecting
            && !self.redis
            && mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(i) = self.view_tabs.iter().position(|r| r.contains(point))
        {
            self.show_view(TableView::ALL[i]);
            return;
        }
        if self.connecting {
            if self.busy {
                return;
            }
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                if let Some(i) = self.form.tabs.iter().position(|r| r.contains(point)) {
                    self.form
                        .set_mode([Mode::Url, Mode::Manual, Mode::History][i]);
                } else if let Some(i) = self.form.field_rects.iter().position(|r| r.contains(point))
                {
                    self.form.focus = i + 1;
                    click_text(&mut self.form.fields[i], self.form.field_rects[i], point);
                } else if self.form.action.contains(point) {
                    if let Err(e) = self.connect() {
                        self.status = e.to_string();
                    }
                } else if self.form.mode == Mode::History && self.form.list.contains(point) {
                    self.form.focus = 1;
                    self.form.selected = (self.form.history_state.offset()
                        + mouse.row.saturating_sub(self.form.list.y + 1) as usize)
                        .min(self.history.entries.len().saturating_sub(1));
                }
            } else if self.form.mode == Mode::History {
                match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        self.form.selected = self.form.selected.saturating_sub(1)
                    }
                    MouseEventKind::ScrollDown => {
                        self.form.selected = (self.form.selected + 1)
                            .min(self.history.entries.len().saturating_sub(1))
                    }
                    _ => {}
                }
            }
            return;
        }
        if matches!(
            mouse.kind,
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left)
        ) {
            if self.vbar.contains(point) {
                self.y = (mouse.row - self.vbar.y) as usize * self.max_y()
                    / self.vbar.height.saturating_sub(1).max(1) as usize;
                return;
            }
            if self.hbar.contains(point) {
                self.x = (mouse.column - self.hbar.x) as usize * self.max_x()
                    / self.hbar.width.saturating_sub(1).max(1) as usize;
                return;
            }
        }
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            if let Some(i) = self.buttons.iter().position(|r| r.contains(point)) {
                match i {
                    0 => self.run_query(),
                    1 => {
                        if !self.busy {
                            self.connecting = true;
                        }
                    }
                    2 => self.refresh(),
                    _ => self.help = true,
                }
                return;
            }
            if let Some(i) = self.panes.iter().position(|r| r.contains(point)) {
                self.focus = i;
                if i == 0 {
                    self.selected_table = (self.table_state.offset()
                        + mouse.row.saturating_sub(self.panes[0].y + 1) as usize)
                        .min(self.browser_len().saturating_sub(1));
                    self.preview();
                } else if i == 1 {
                    self.editor.cancel_selection();
                    click_text(&mut self.editor, self.panes[1], point);
                    self.editor.start_selection();
                } else if i == 2 && self.grid.contains(point) && mouse.row > self.grid.y {
                    self.row = (self.y + (mouse.row - self.grid.y - 1) as usize)
                        .min(self.result.rows.len().saturating_sub(1));
                    self.col = ((self.x + (mouse.column - self.grid.x) as usize) / CELL_WIDTH)
                        .min(self.result.columns.len().saturating_sub(1));
                }
            }
        }
        if mouse.kind == MouseEventKind::Drag(MouseButton::Left) && self.panes[1].contains(point) {
            click_text(&mut self.editor, self.panes[1], point);
            return;
        }
        let amount = if mouse.modifiers.intersects(M::CONTROL | M::ALT) {
            10
        } else {
            3
        };
        let (dy, dx) = match mouse.kind {
            MouseEventKind::ScrollUp if mouse.modifiers.contains(M::SHIFT) => (0, -amount),
            MouseEventKind::ScrollDown if mouse.modifiers.contains(M::SHIFT) => (0, amount),
            MouseEventKind::ScrollUp => (-amount, 0),
            MouseEventKind::ScrollDown => (amount, 0),
            MouseEventKind::ScrollLeft => (0, -amount),
            MouseEventKind::ScrollRight => (0, amount),
            _ => return,
        };
        if let Some(i) = self.panes.iter().position(|r| r.contains(point)) {
            self.scroll(i, dy, dx);
        }
    }
    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        frame.render_widget(
            Block::default().style(Style::default().bg(BG).fg(Color::White)),
            area,
        );
        if area.width < 55 || area.height < 26 {
            frame.render_widget(
                Paragraph::new("Please resize to at least 55 × 26. Ctrl+Q quits.")
                    .wrap(Wrap { trim: true }),
                area,
            );
            return;
        }
        let regions = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .split(area);
        if self.connecting {
            self.draw_form(frame, regions[1]);
        } else {
            self.draw_workspace(frame, regions[1]);
        }
        let top = Layout::horizontal([
            Constraint::Min(8),
            Constraint::Length(10),
            Constraint::Length(16),
            Constraint::Length(13),
            Constraint::Length(10),
        ])
        .split(regions[0]);
        frame.render_widget(
            Paragraph::new(if self.redis {
                " dbrust / Redis browser"
            } else {
                " dbrust / SQL workbench"
            })
            .style(Style::default().fg(ACCENT).bold()),
            top[0],
        );
        self.buttons = [top[1], top[2], top[3], top[4]];
        for (i, label) in ["F5 Run", "F2 Connections", "F3 Refresh", "F1 Help"]
            .iter()
            .enumerate()
        {
            frame.render_widget(
                Paragraph::new(*label)
                    .alignment(Alignment::Center)
                    .block(panel("", false)),
                self.buttons[i],
            );
        }
        frame.render_widget(
            Paragraph::new(format!(
                " {}{}",
                if self.busy { "◌ " } else { "" },
                self.status
            ))
            .style(Style::default().fg(if self.busy { ACCENT } else { MUTED }))
            .wrap(Wrap { trim: false }),
            regions[2],
        );
        let bottom = if self.connecting {
            " Tab / ↑↓ fields   F2 method   Enter next / connect   Ctrl+Q quit".into()
        } else {
            self.result.rows.get(self.row).and_then(|r|r.get(self.col)).map(|v|format!(" Cell [{}, {}]: {}",self.row+1,self.col+1,v.replace('\n'," ↵ "))).unwrap_or_else(||" Tab focus · Enter select · F5 run · Shift+arrows scroll · Ctrl/Alt+arrows fast · Ctrl+Q quit".into())
        };
        frame.render_widget(
            Paragraph::new(bottom).style(Style::default().fg(MUTED)),
            regions[3],
        );
        if let Some(value) = &self.detail {
            let popup = centered(area, 90, area.height.saturating_sub(4));
            let width = popup.width.saturating_sub(2).max(1) as usize;
            let lines: usize = value
                .lines()
                .map(|line| line.width().max(1).div_ceil(width))
                .sum();
            self.detail_limit = lines
                .saturating_sub(popup.height.saturating_sub(2) as usize)
                .min(u16::MAX as usize) as u16;
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(value.as_str())
                    .wrap(Wrap { trim: false })
                    .scroll((self.detail_y, 0))
                    .block(panel(
                        " Full value · ↑↓ / wheel scroll · Enter/Esc close ",
                        true,
                    ))
                    .style(Style::default().bg(BG)),
                popup,
            );
        }
        if self.filtering {
            self.draw_filters(frame, area);
        }
        if self.find_value {
            let popup = centered(area, 70, 5);
            frame.render_widget(Clear, popup);
            self.value_find.set_block(panel(
                " Find in loaded values · Enter apply · blank clears · Esc cancel ",
                true,
            ));
            frame.render_widget(&self.value_find, popup);
        }
        if self.help {
            let popup = centered(area, 80, 25);
            frame.render_widget(Clear, popup);
            frame.render_widget(Paragraph::new("Navigate with Tab / Shift+Tab; the mint border shows focus.\n\nConnections: F2 changes method. Use URL, manual fields, or history.\nDrivers: postgres, mysql, sqlite, redis, rediss, valkey, keydb.\nPasswords are masked, never saved; history asks for them again.\n\nTables/keys: ↑↓ selects; Enter previews. F6 cycles schema views.\n1–5 outside editor or Alt+1–5 switches table views. Redis: F4 filters; F6 tree; F7 sort; F8 order; F9 find values.\nEditor: arrow keys edit; Enter adds a line; paste and undo supported.\nExecute: F5, Ctrl+Enter, or click Run. Tab stays in the three panels.\nResults: arrows select cells; Enter opens full value / DDL. Tab edits SQL.\nShift+arrows scroll 1 unit; Ctrl/Alt+arrows scroll 10 units.\nPageUp/PageDown scroll results by a page. Home/End jump.\nMouse: click a table to preview; click panels, fields, cells, or buttons.\nWheel scrolls vertically; Shift+wheel or horizontal wheel scrolls sideways.\nClick or drag result scrollbars to jump.\n\nF3 refreshes tables. F2 switches connections; Esc returns.\nCtrl+C copies selected cells/keys/text; Ctrl+V pastes into active fields.\nCtrl+Q quits. Queries execute as written, including writes.\nDisplay keeps 10,000 rows from the last nonempty result set.\nUse SQL LIMIT for large queries; all submitted statements still execute.\n\nF1 / Esc / Enter / click closes help.").wrap(Wrap{trim:false}).block(panel(" Help ",true)).style(Style::default().bg(BG)),popup);
        }
    }
    fn draw_form(&mut self, frame: &mut Frame, area: Rect) {
        let outer = centered(area, 88, 27);
        frame.render_widget(panel(" Connect to a database ", true), outer);
        let inside = outer.inner(Margin::new(2, 1));
        let sections = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(inside);
        self.form.tabs = Layout::horizontal([Constraint::Ratio(1, 3); 3])
            .split(sections[0])
            .to_vec();
        for (i, label) in ["Connection URL", "Manual fields", "Recent connections"]
            .iter()
            .enumerate()
        {
            let active = matches!(
                (i, self.form.mode),
                (0, Mode::Url) | (1, Mode::Manual) | (2, Mode::History) | (2, Mode::Password)
            );
            frame.render_widget(
                Paragraph::new(*label)
                    .alignment(Alignment::Center)
                    .block(panel("", active)),
                self.form.tabs[i],
            );
        }
        self.form.field_rects.clear();
        if self.form.mode == Mode::History {
            self.form.list = sections[1];
            let items: Vec<ListItem> = self
                .history
                .entries
                .iter()
                .map(|e| ListItem::new(e.url.clone()))
                .collect();
            self.form.history_state.select(if items.is_empty() {
                None
            } else {
                Some(self.form.selected)
            });
            frame.render_stateful_widget(
                List::new(items)
                    .block(panel(" Select a saved connection ", self.form.focus == 1))
                    .highlight_style(selected())
                    .highlight_symbol("› "),
                sections[1],
                &mut self.form.history_state,
            );
            if self.history.entries.is_empty() {
                frame.render_widget(
                    Paragraph::new("No saved connections. Choose URL or Manual with F2."),
                    sections[1].inner(Margin::new(2, 2)),
                );
            }
        } else {
            let labels: Vec<&str> = match self.form.mode {
                Mode::Manual => vec![
                    "Driver ←→ (SQL / Redis / Valkey / KeyDB)",
                    "Host",
                    "Port (blank = default)",
                    "Database / SQLite file path",
                    "Username",
                    "Password",
                ],
                Mode::Password => vec!["Password for selected connection"],
                _ => vec![
                    "URL (masked; e.g. postgres://user:password@localhost/db or sqlite::memory:)",
                ],
            };
            let rows: Vec<Rect> = if self.form.mode == Mode::Manual {
                Layout::vertical([Constraint::Length(3); 3])
                    .split(sections[1])
                    .iter()
                    .flat_map(|row| {
                        Layout::horizontal([Constraint::Percentage(50); 2])
                            .split(*row)
                            .to_vec()
                    })
                    .collect()
            } else {
                vec![Rect::new(
                    sections[1].x,
                    sections[1].y,
                    sections[1].width,
                    3,
                )]
            };
            for (i, label) in labels.iter().enumerate() {
                self.form.field_rects.push(rows[i]);
                self.form.fields[i].set_block(panel(label, self.form.focus == i + 1));
                self.form.fields[i].set_cursor_style(if self.form.focus == i + 1 {
                    Style::default().reversed()
                } else {
                    Style::default()
                });
                frame.render_widget(&self.form.fields[i], rows[i]);
            }
        }
        self.form.action = sections[2];
        frame.render_widget(
            Paragraph::new(if self.busy {
                "Connecting…"
            } else {
                "Connect  ↵"
            })
            .alignment(Alignment::Center)
            .block(panel("", self.form.focus == self.form.slots() - 1)),
            sections[2],
        );
    }
    fn prepare_redis_inspector(&mut self) {
        let rows = &self.result.rows;
        if rows.len() < 3 {
            return;
        }
        let kind = rows[1][1].clone();
        let ttl = rows[2][1].clone();
        let data_start = if rows.get(3).is_some_and(|r| r[0].starts_with("Length")) {
            4
        } else {
            3
        };
        self.redis_key_info = format!(
            " {} · TTL {}{}",
            kind,
            ttl,
            if data_start == 4 {
                format!(
                    " · {} {}",
                    rows[3][1],
                    if kind == "string" { "bytes" } else { "items" }
                )
            } else {
                String::new()
            }
        );
        self.redis_document = None;
        if kind == "string" {
            let document = rows.get(4).map(|r| r[1].clone()).unwrap_or_default();
            self.result.rows = document.lines().map(|line| vec![line.to_owned()]).collect();
            if self.result.rows.is_empty() {
                self.result.rows = vec![vec![String::new()]];
            }
            self.result.columns = vec!["JSON / Text".into()];
            self.redis_document = Some(document);
        } else {
            self.result.rows = self.result.rows[data_start..].to_vec();
            self.result.columns = match kind.as_str() {
                "hash" => vec!["Field".into(), "Value".into()],
                "zset" => vec!["Member".into(), "Score".into()],
                "stream" => vec!["Entry ID".into(), "Fields".into()],
                "list" | "set" => vec!["Index".into(), "Value".into()],
                _ => vec!["Property".into(), "Value".into()],
            };
        }
    }
    fn browser_entries(&self) -> Vec<BrowserEntry> {
        redis_browser::entries(
            &self.snapshot,
            self.tree,
            &self.namespace,
            self.key_sort,
            self.descending,
        )
    }
    fn browser_len(&self) -> usize {
        if self.redis {
            self.browser_entries().len()
        } else {
            self.tables.len()
        }
    }
    fn namespace_up(&mut self) {
        if let Some((prefix, _)) = self.namespace.trim_end_matches(':').rsplit_once(':') {
            self.namespace = format!("{prefix}:");
        } else {
            self.namespace.clear();
        }
        self.selected_table = 0;
        self.table_state = ListState::default();
    }
    fn open_filters(&mut self) {
        if self.busy {
            return;
        }
        self.filter_draft = self.redis_filter.clone();
        self.key_filter = input(&self.key_pattern, false);
        self.ttl_min = input(
            &self
                .redis_filter
                .min_ttl
                .map(|n| n.to_string())
                .unwrap_or_default(),
            false,
        );
        self.ttl_max = input(
            &self
                .redis_filter
                .max_ttl
                .map(|n| n.to_string())
                .unwrap_or_default(),
            false,
        );
        self.filter_focus = 0;
        self.filtering = true;
    }
    fn cycle_filter_type(&mut self, back: bool) {
        let i = KeyFilter::TYPES
            .iter()
            .position(|v| *v == self.filter_draft.kind)
            .unwrap_or(0);
        self.filter_draft.kind = KeyFilter::TYPES[(i + if back { 6 } else { 1 }) % 7].into();
    }
    fn apply_filters(&mut self) {
        let number = |value: &str| -> Result<Option<u64>> {
            if value.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(value.trim().parse().map_err(|_| {
                    anyhow::anyhow!("TTL bounds must be nonnegative whole seconds")
                })?))
            }
        };
        let result = (|| -> Result<KeyFilter> {
            let mut f = self.filter_draft.clone();
            f.pattern = if self.key_filter.lines()[0].is_empty() {
                "*".into()
            } else {
                self.key_filter.lines()[0].clone()
            };
            f.min_ttl = number(&self.ttl_min.lines()[0])?;
            f.max_ttl = number(&self.ttl_max.lines()[0])?;
            f.validate()?;
            Ok(f)
        })();
        match result {
            Ok(f) => {
                self.redis_filter = f;
                self.key_pattern = self.redis_filter.pattern.clone();
                self.filtering = false;
                self.namespace.clear();
                self.selected_table = 0;
                self.refresh_scan(true);
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    fn reset_filters(&mut self) {
        if self.busy {
            return;
        }
        self.redis_filter = KeyFilter::default();
        self.key_pattern = "*".into();
        self.filtering = false;
        self.namespace.clear();
        self.selected_table = 0;
        self.table_state = ListState::default();
        self.table_x = 0;
        self.refresh_scan(true);
    }
    fn filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.filtering = false,
            KeyCode::Tab | KeyCode::BackTab => {
                self.filter_focus = (self.filter_focus
                    + if key.code == KeyCode::BackTab || key.modifiers.contains(M::SHIFT) {
                        6
                    } else {
                        1
                    })
                    % 7
            }
            KeyCode::Up => self.filter_focus = self.filter_focus.saturating_sub(1),
            KeyCode::Down => self.filter_focus = (self.filter_focus + 1).min(6),
            KeyCode::Left | KeyCode::Right if self.filter_focus == 1 => {
                self.cycle_filter_type(key.code == KeyCode::Left)
            }
            KeyCode::Left | KeyCode::Right if self.filter_focus == 2 => {
                self.filter_draft.expiry = self.filter_draft.expiry.next()
            }
            KeyCode::Enter => {
                if self.filter_focus == 6 {
                    self.reset_filters();
                } else {
                    self.apply_filters();
                }
            }
            _ => match self.filter_focus {
                0 => {
                    self.key_filter.input(key);
                }
                3 => {
                    self.ttl_min.input(key);
                }
                4 => {
                    self.ttl_max.input(key);
                }
                _ => {}
            },
        }
    }
    fn apply_value_search(&mut self) {
        if self.unfiltered_result.is_none() {
            self.unfiltered_result = Some(std::mem::take(&mut self.result));
        }
        self.value_query = self.value_find.lines()[0].clone();
        if let Some(source) = &self.unfiltered_result {
            let query = self.value_query.to_lowercase();
            self.result = QueryResult {
                columns: source.columns.clone(),
                rows: source
                    .rows
                    .iter()
                    .filter(|row| {
                        query.is_empty() || row.iter().any(|v| v.to_lowercase().contains(&query))
                    })
                    .cloned()
                    .collect(),
                truncated: source.truncated,
                ..Default::default()
            };
        }
        self.row = 0;
        self.col = 0;
        self.x = 0;
        self.y = 0;
    }
    fn draw_filters(&mut self, frame: &mut Frame, area: Rect) {
        let popup = centered(area, 76, 23);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            panel(
                " Redis filters · Tab fields · ←→ choices · Enter apply · Esc cancel ",
                true,
            )
            .style(Style::default().bg(BG)),
            popup,
        );
        let inner = popup.inner(Margin::new(2, 1));
        let rows = Layout::vertical([Constraint::Length(3); 6]).split(inner);
        let pair = Layout::horizontal([Constraint::Percentage(50); 2]).split(rows[4]);
        let buttons = Layout::horizontal([Constraint::Percentage(50); 2]).split(rows[5]);
        self.filter_rects = vec![
            rows[0], rows[1], rows[2], pair[0], pair[1], buttons[0], buttons[1],
        ];
        self.key_filter.set_block(panel(
            " Key pattern · Redis glob (* ? [abc]) ",
            self.filter_focus == 0,
        ));
        frame.render_widget(&self.key_filter, rows[0]);
        frame.render_widget(
            Paragraph::new(if self.filter_draft.kind.is_empty() {
                "All types"
            } else {
                &self.filter_draft.kind
            })
            .block(panel(" Type ←→ ", self.filter_focus == 1)),
            rows[1],
        );
        frame.render_widget(
            Paragraph::new(self.filter_draft.expiry.label())
                .block(panel(" Expiry ←→ ", self.filter_focus == 2)),
            rows[2],
        );
        frame.render_widget(
            Paragraph::new(
                "Name/type: server SCAN · TTL: metadata snapshot
Blank TTL bounds mean no limit. No expiry = persistent keys.",
            )
            .style(Style::default().fg(MUTED)),
            rows[3],
        );
        self.ttl_min
            .set_block(panel(" Min TTL seconds ", self.filter_focus == 3));
        self.ttl_max
            .set_block(panel(" Max TTL seconds ", self.filter_focus == 4));
        frame.render_widget(&self.ttl_min, pair[0]);
        frame.render_widget(&self.ttl_max, pair[1]);
        frame.render_widget(
            Paragraph::new("Apply filters ↵")
                .alignment(Alignment::Center)
                .block(panel("", self.filter_focus == 5)),
            buttons[0],
        );
        frame.render_widget(
            Paragraph::new("Reset all")
                .alignment(Alignment::Center)
                .block(panel("", self.filter_focus == 6)),
            buttons[1],
        );
    }
    fn draw_workspace(&mut self, frame: &mut Frame, mut area: Rect) {
        self.redis_controls.clear();
        if self.redis {
            let header = Layout::vertical([
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(area);
            let fields = Layout::horizontal([
                Constraint::Percentage(50),
                Constraint::Percentage(25),
                Constraint::Percentage(25),
            ])
            .split(header[0]);
            self.redis_controls.extend(fields.iter().copied());
            for (i, (title, value)) in [
                (" Key pattern · F4 ", self.key_pattern.clone()),
                (
                    " Type ",
                    if self.redis_filter.kind.is_empty() {
                        "All types".into()
                    } else {
                        self.redis_filter.kind.clone()
                    },
                ),
                (" Expiry ", self.redis_filter.expiry.label().into()),
            ]
            .iter()
            .enumerate()
            {
                frame.render_widget(
                    Paragraph::new(value.as_str()).block(panel(title, false)),
                    fields[i],
                );
            }
            let actions = Layout::horizontal([Constraint::Ratio(1, 8); 8]).split(header[1]);
            self.redis_controls.extend(actions.iter().copied());
            for (i, label) in [
                "F4 Filters",
                "Clear",
                "F3 More",
                "⇧F3 Rescan",
                if self.tree { "F6 List" } else { "F6 Tree" },
                "F7 Sort",
                "F8 Order",
                "F9 Find",
            ]
            .iter()
            .enumerate()
            {
                frame.render_widget(
                    Paragraph::new(*label).style(Style::default().fg(ACCENT)),
                    actions[i],
                );
            }
            frame.render_widget(
                Paragraph::new(format!(
                    " {} matched · {} returned by SCAN · DB {} · {} · sort {} {}{}",
                    self.snapshot.keys.len(),
                    self.snapshot.scanned,
                    self.snapshot
                        .total
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "?".into()),
                    if self.snapshot.capped {
                        "limit reached; narrow filters"
                    } else if self.snapshot.complete {
                        "Complete"
                    } else {
                        "Partial; Scan more"
                    },
                    self.key_sort.label(),
                    if self.descending { "↓" } else { "↑" },
                    if self.value_query.is_empty() {
                        String::new()
                    } else {
                        format!(" · values: {} (loaded preview only)", self.value_query)
                    }
                ))
                .style(Style::default().fg(MUTED)),
                header[2],
            );
            area = header[3];
        }
        let cols = Layout::horizontal([
            Constraint::Percentage(if self.redis { 43 } else { 23 }),
            Constraint::Min(25),
        ])
        .split(area);
        let right = if self.redis {
            let r = Layout::vertical([Constraint::Min(5), Constraint::Length(6)]).split(cols[1]);
            vec![r[1], r[0]]
        } else {
            Layout::vertical([Constraint::Percentage(35), Constraint::Min(5)])
                .split(cols[1])
                .to_vec()
        };
        self.panes = [cols[0], right[0], right[1]];
        let items: Vec<ListItem> = if self.redis {
            self.browser_entries()
                .iter()
                .map(|entry| {
                    let style = match entry {
                        BrowserEntry::Folder(_, _) => Style::default().fg(ACCENT),
                        BrowserEntry::Key(k) => Style::default().fg(match k.kind.as_str() {
                            "string" => Color::LightBlue,
                            "hash" => Color::LightMagenta,
                            "list" => Color::LightYellow,
                            "set" | "zset" => Color::LightGreen,
                            _ => Color::White,
                        }),
                    };
                    ListItem::new(drop_columns(&entry.label(), self.table_x)).style(style)
                })
                .collect()
        } else {
            self.tables
                .iter()
                .map(|t| ListItem::new(drop_columns(&t.label(), self.table_x)))
                .collect()
        };
        self.table_state.select(if items.is_empty() {
            None
        } else {
            Some(self.selected_table)
        });
        let empty = items.is_empty();
        let browser_title = if self.redis {
            if self.tree {
                format!(
                    " {} · Backspace up ",
                    if self.namespace.is_empty() {
                        "Namespaces /"
                    } else {
                        &self.namespace
                    }
                )
            } else {
                " Type    TTL     Key ".into()
            }
        } else {
            " Tables · Enter preview ".into()
        };
        frame.render_stateful_widget(
            List::new(items)
                .block(panel(&browser_title, self.focus == 0))
                .highlight_style(selected())
                .highlight_symbol("› "),
            cols[0],
            &mut self.table_state,
        );
        if self.redis && empty {
            frame.render_widget(
                Paragraph::new(if self.snapshot.complete {
                    "No matching keys.
F4 edits filters; Clear resets.
Shift+F3 starts a fresh scan."
                } else {
                    "No matches in this scan page.
F3 scans more keys.
F4 edits filters."
                })
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(MUTED)),
                cols[0].inner(Margin::new(2, 2)),
            );
        }
        self.editor.set_block(panel(
            if self.redis {
                " Redis commands · F5 execute "
            } else {
                " SQL editor · F5 execute "
            },
            self.focus == 1,
        ));
        self.editor.set_cursor_style(if self.focus == 1 {
            Style::default().reversed()
        } else {
            Style::default()
        });
        frame.render_widget(&self.editor, right[0]);
        frame.render_widget(
            panel(
                &format!(
                    " {} · {} ",
                    if self.redis {
                        "Key / command results"
                    } else {
                        self.view.label()
                    },
                    if self.active_object.is_empty() {
                        &self.label
                    } else {
                        &self.active_object
                    }
                ),
                self.focus == 2,
            ),
            right[1],
        );
        let mut inner = right[1].inner(Margin::new(1, 1));
        self.view_tabs.clear();
        if !self.redis {
            self.view_tabs = Layout::horizontal([Constraint::Ratio(1, 5); 5])
                .split(Rect::new(inner.x, inner.y, inner.width, 1))
                .to_vec();
            for (i, label) in ["1 Rows", "2 Cols", "3 Idx", "4 FKs", "5 DDL"]
                .iter()
                .enumerate()
            {
                frame.render_widget(
                    Paragraph::new(*label).style(if TableView::ALL[i] == self.view {
                        selected()
                    } else {
                        Style::default().fg(MUTED)
                    }),
                    self.view_tabs[i],
                );
            }
            inner.y += 1;
            inner.height = inner.height.saturating_sub(1);
        }
        if self.redis && !self.redis_key_info.is_empty() {
            frame.render_widget(
                Paragraph::new(self.redis_key_info.as_str()).style(Style::default().fg(ACCENT)),
                Rect::new(inner.x, inner.y, inner.width, 1),
            );
            inner.y += 1;
            inner.height = inner.height.saturating_sub(1);
        }
        self.grid = Rect::new(
            inner.x,
            inner.y,
            inner.width.saturating_sub(1),
            inner.height.saturating_sub(1),
        );
        self.vbar = Rect::new(
            inner.right().saturating_sub(1),
            inner.y,
            1,
            self.grid.height,
        );
        self.hbar = Rect::new(
            inner.x,
            inner.bottom().saturating_sub(1),
            self.grid.width,
            1,
        );
        self.y = self.y.min(self.max_y());
        self.x = self.x.min(self.max_x());
        let widths = self.column_widths();
        let mut lines = vec![Line::from(
            self.result
                .columns
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    Span::styled(
                        cell_text_width(c, widths[i]),
                        if i == self.col {
                            Style::default().fg(ACCENT).bold()
                        } else {
                            Style::default().fg(MUTED).bold()
                        },
                    )
                })
                .collect::<Vec<_>>(),
        )];
        lines.extend(
            self.result
                .rows
                .iter()
                .enumerate()
                .skip(self.y)
                .take(self.grid.height.saturating_sub(1) as usize)
                .map(|(ri, r)| {
                    Line::from(
                        r.iter()
                            .enumerate()
                            .map(|(ci, c)| {
                                Span::styled(
                                    cell_text_width(c, widths[ci]),
                                    if ri == self.row && ci == self.col {
                                        selected()
                                    } else if ri == self.row {
                                        Style::default().bg(Color::Rgb(32, 45, 58))
                                    } else {
                                        Style::default()
                                    },
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
                }),
        );
        if self.result.columns.is_empty() {
            lines = vec![Line::from(if self.redis {
                "Select a key to inspect its value, or run a command."
            } else {
                "Run a query to see results here."
            })];
        }
        frame.render_widget(
            Paragraph::new(lines).scroll((0, self.x.min(u16::MAX as usize) as u16)),
            self.grid,
        );
        let mut vertical = ScrollbarState::new(self.max_y() + 1)
            .position(self.y)
            .viewport_content_length(1);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None),
            self.vbar,
            &mut vertical,
        );
        let mut horizontal = ScrollbarState::new(self.max_x() + 1)
            .position(self.x)
            .viewport_content_length(1);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::HorizontalBottom)
                .begin_symbol(None)
                .end_symbol(None),
            self.hbar,
            &mut horizontal,
        );
    }
}
fn selected_text(text: &mut TextArea<'_>) -> String {
    if text
        .selection_range()
        .is_some_and(|(start, end)| start != end)
    {
        text.copy();
        text.yank_text()
    } else {
        text.lines().join("\n")
    }
}
fn click_text(text: &mut TextArea<'_>, area: Rect, point: Position) {
    let inner = area.inner(Margin::new(1, 1));
    if !inner.contains(point) {
        return;
    }
    let (top, left) = text.scroll_offset();
    let clicked = text.screen_to_data(
        (top + point.y - inner.y) as usize,
        (left + point.x - inner.x) as usize,
    );
    text.move_cursor(CursorMove::Jump(
        clicked.0.min(u16::MAX as usize) as u16,
        clicked.1.min(u16::MAX as usize) as u16,
    ));
}
fn panel(title: &str, active: bool) -> Block<'static> {
    Block::bordered()
        .title(title.to_string())
        .border_style(Style::default().fg(if active {
            ACCENT
        } else {
            Color::Rgb(55, 69, 85)
        }))
}
fn selected() -> Style {
    Style::default().bg(ACCENT).fg(BG)
}
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = area.width.min(width);
    let h = area.height.min(height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}
fn direction(code: KeyCode, step: i16) -> (i16, i16) {
    match code {
        KeyCode::Up => (-step, 0),
        KeyCode::Down => (step, 0),
        KeyCode::Left => (0, -step),
        _ => (0, step),
    }
}
fn cell_text_width(value: &str, cell_width: usize) -> String {
    let cell_width = cell_width.max(2);
    let mut out = String::new();
    let mut width = 0;
    for c in value.chars() {
        let c = if c.is_control() { ' ' } else { c };
        let w = c.width().unwrap_or(0);
        if width + w > cell_width - 2 {
            break;
        }
        out.push(c);
        width += w;
    }
    out.push_str(&" ".repeat(cell_width - 1 - width));
    out.push('│');
    out
}
fn drop_columns(value: &str, n: usize) -> String {
    let mut width = 0;
    value
        .chars()
        .skip_while(|c| {
            if width < n {
                width += c.width().unwrap_or(0);
                true
            } else {
                false
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> App {
        App::new(History::default())
    }
    #[test]
    fn keyboard_focus_and_scrolling_are_distinct() {
        let mut a = app();
        a.connecting = false;
        a.focus = 2;
        a.grid = Rect::new(0, 0, 30, 6);
        a.result.columns = vec!["a".into(), "b".into(), "c".into()];
        a.result.rows = vec![vec!["v".into(); 3]; 100];
        a.key(KeyEvent::new(KeyCode::Down, M::SHIFT));
        assert_eq!(a.y, 1);
        assert_eq!(a.row, 0);
        a.key(KeyEvent::new(KeyCode::Down, M::CONTROL));
        assert_eq!(a.y, 11);
        a.key(KeyEvent::new(KeyCode::Right, M::ALT));
        assert_eq!(a.x, 10);
        a.key(KeyEvent::new(KeyCode::Tab, M::NONE));
        assert_eq!(a.focus, 0);
        a.key(KeyEvent::new(KeyCode::BackTab, M::SHIFT));
        assert_eq!(a.focus, 2);
    }
    #[test]
    fn mouse_scrolls_under_pointer_and_selects_cells() {
        let mut a = app();
        a.connecting = false;
        a.panes[2] = Rect::new(10, 10, 30, 10);
        a.grid = Rect::new(11, 11, 27, 7);
        a.result.columns = vec!["a".into(), "b".into()];
        a.result.rows = vec![vec!["v".into(); 2]; 100];
        a.mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 15,
            row: 15,
            modifiers: M::NONE,
        });
        assert_eq!(a.y, 3);
        a.mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 15,
            row: 15,
            modifiers: M::SHIFT,
        });
        assert_eq!(a.x, 3);
        a.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 15,
            row: 13,
            modifiers: M::NONE,
        });
        assert_eq!(a.row, 4);
        assert_eq!(a.focus, 2);
    }
    #[test]
    fn all_screens_render_at_supported_sizes() {
        for (w, h) in [(100, 40), (55, 26), (20, 8)] {
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            let mut a = app();
            for mode in [Mode::Url, Mode::Manual, Mode::History, Mode::Password] {
                a.form.set_mode(mode);
                terminal.draw(|f| a.draw(f)).unwrap();
            }
            a.connecting = false;
            terminal.draw(|f| a.draw(f)).unwrap();
            a.help = true;
            terminal.draw(|f| a.draw(f)).unwrap();
        }
    }
    #[test]
    fn unicode_cells_have_fixed_display_width() {
        assert_eq!(
            cell_text_width("hello 世界 🌍", CELL_WIDTH).width(),
            CELL_WIDTH
        );
        assert_eq!(cell_text_width("a\nb\t", CELL_WIDTH).width(), CELL_WIDTH);
    }
    #[test]
    fn mouse_places_editor_cursor_after_unicode_and_scroll() {
        let mut a = app();
        a.connecting = false;
        a.editor = input("hello 世界", false);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| a.draw(f)).unwrap();
        a.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: a.panes[1].x + 9,
            row: a.panes[1].y + 1,
            modifiers: M::NONE,
        });
        assert_eq!(a.editor.cursor().1, 7);
        assert_eq!(a.focus, 1);
    }
    #[tokio::test]
    async fn startup_query_preview_and_history_reconnect() {
        let path = std::env::temp_dir().join(format!(
            "dbrust-app-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut a = App::new(History::load_from(path.clone()).unwrap());
        a.paste("sqlite::memory:");
        a.key(KeyEvent::new(KeyCode::Enter, M::NONE));
        wait_for_work(&mut a).await;
        assert!(!a.connecting, "{}", a.status);
        a.editor = input(
            "CREATE TABLE demo (id INTEGER); INSERT INTO demo VALUES (123); SELECT * FROM demo;",
            false,
        );
        a.key(KeyEvent::new(KeyCode::F(5), M::NONE));
        wait_for_work(&mut a).await;
        assert_eq!(a.result.rows[0][0], "123");
        a.refresh();
        wait_for_work(&mut a).await;
        assert_eq!(a.tables[0].name, "demo");
        a.focus = 0;
        a.key(KeyEvent::new(KeyCode::Enter, M::NONE));
        wait_for_work(&mut a).await;
        assert!(a.editor.lines()[0].contains("LIMIT 1000"));
        assert_eq!(a.result.rows[0][0], "123");
        let mut b = App::new(History::load_from(path.clone()).unwrap());
        assert!(b.form.mode == Mode::History);
        b.key(KeyEvent::new(KeyCode::Enter, M::NONE));
        wait_for_work(&mut b).await;
        assert!(!b.connecting, "{}", b.status);
        std::fs::remove_file(path).unwrap();
    }
    async fn wait_for_work(a: &mut App) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while a.busy {
                tokio::time::sleep(Duration::from_millis(5)).await;
                a.poll();
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn schema_tabs_mouse_clicks_and_editor_do_not_conflict() {
        let mut a = app();
        a.db = Some(Arc::new(Mutex::new(
            Database::connect("sqlite::memory:").await.unwrap(),
        )));
        a.db.as_ref()
            .unwrap()
            .lock()
            .await
            .execute("CREATE TABLE test (id INTEGER PRIMARY KEY, name TEXT DEFAULT 'x')")
            .await
            .unwrap();
        a.connecting = false;
        a.tables = vec![TableName {
            schema: String::new(),
            name: "test".into(),
        }];
        let editor = a.editor.lines().to_vec();
        a.key(KeyEvent::new(KeyCode::F(6), M::NONE));
        wait_for_work(&mut a).await;
        assert_eq!(a.view, TableView::Columns);
        assert_eq!(a.result.rows.len(), 2);
        assert_eq!(a.editor.lines(), editor);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| a.draw(f)).unwrap();
        let tab = a.view_tabs[4];
        a.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: tab.x,
            row: tab.y,
            modifiers: M::NONE,
        });
        wait_for_work(&mut a).await;
        assert_eq!(a.view, TableView::Definition);
        assert!(a.result.rows[0][2].contains("CREATE TABLE"));
        a.col = 2;
        a.key(KeyEvent::new(KeyCode::Enter, M::NONE));
        assert!(a.detail.as_ref().unwrap().contains("CREATE TABLE"));
        a.key(KeyEvent::new(KeyCode::Esc, M::NONE));
        assert!(a.detail.is_none());
        a.focus = 1;
        a.key(KeyEvent::new(KeyCode::Char('2'), M::NONE));
        assert_eq!(a.view, TableView::Definition);
        for _ in 0..12 {
            a.key(KeyEvent::new(KeyCode::Tab, M::NONE));
            assert!(a.focus < 3);
        }
    }
    #[test]
    fn cancelled_redis_filter_does_not_apply_and_wheel_keeps_y() {
        let mut a = app();
        a.connecting = false;
        a.redis = true;
        a.key_pattern = "users:*".into();
        a.key(KeyEvent::new(KeyCode::F(4), M::NONE));
        a.key_filter = input("other:*", false);
        a.key(KeyEvent::new(KeyCode::Esc, M::NONE));
        assert_eq!(a.key_pattern, "users:*");
        a.panes[2] = Rect::new(10, 10, 30, 10);
        a.grid = Rect::new(11, 11, 27, 7);
        a.result.columns = vec!["x".into(); 4];
        a.result.rows = vec![vec!["v".into(); 4]; 100];
        a.y = 5;
        a.mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 15,
            row: 15,
            modifiers: M::SHIFT,
        });
        assert_eq!(a.y, 5);
        assert_eq!(a.x, 3);
        a.mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 15,
            row: 15,
            modifiers: M::SHIFT,
        });
        assert_eq!(a.x, 0);
        assert_eq!(a.y, 5);
    }
    #[tokio::test]
    #[ignore = "requires DBRUST_TEST_REDIS_URL"]
    async fn redis_app_connect_filter_inspect_and_history() {
        let url = std::env::var("DBRUST_TEST_REDIS_URL").unwrap();
        let path =
            std::env::temp_dir().join(format!("dbrust-redis-app-{}.json", std::process::id()));
        let mut a = App::new(History::load_from(path.clone()).unwrap());
        a.form.set_mode(Mode::Url);
        a.paste(&url);
        a.connect().unwrap();
        wait_for_work(&mut a).await;
        assert!(a.redis, "{}", a.status);
        assert_eq!(a.editor.lines()[0], "PING");
        a.editor = input("SET app:test:json '{\"x\":42}'", false);
        a.run_query();
        wait_for_work(&mut a).await;
        assert_eq!(a.result.rows[0][1], "OK");
        a.key_pattern = "app:test:*".into();
        a.refresh();
        wait_for_work(&mut a).await;
        for _ in 0..40 {
            if a.snapshot.complete {
                break;
            }
            a.refresh();
            wait_for_work(&mut a).await;
        }
        assert_eq!(a.tables.len(), 1);
        a.preview();
        wait_for_work(&mut a).await;
        assert!(a.redis_key_info.contains("string"));
        assert!(a.result.rows.iter().any(|r| r[0].contains("42")));
        let history = History::load_from(path.clone()).unwrap();
        assert_eq!(history.entries.len(), 1);
        a.db.as_ref()
            .unwrap()
            .lock()
            .await
            .execute("DEL app:test:json")
            .await
            .unwrap();
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn redis_advanced_filter_form_and_value_search() {
        let mut a = app();
        a.connecting = false;
        a.redis = true;
        a.open_filters();
        a.key_filter = input("users:*", false);
        a.filter_focus = 1;
        a.key(KeyEvent::new(KeyCode::Right, M::NONE));
        assert_eq!(a.filter_draft.kind, "string");
        a.filter_focus = 2;
        a.key(KeyEvent::new(KeyCode::Right, M::NONE));
        assert_eq!(
            a.filter_draft.expiry,
            crate::redis_browser::ExpiryFilter::Persistent
        );
        a.ttl_min = input("10", false);
        a.key(KeyEvent::new(KeyCode::Enter, M::NONE));
        assert!(a.filtering);
        assert!(a.status.contains("TTL bounds"));
        a.ttl_min = input("", false);
        a.key(KeyEvent::new(KeyCode::Enter, M::NONE));
        assert!(!a.filtering);
        assert_eq!(a.redis_filter.pattern, "users:*");
        a.result = QueryResult {
            columns: vec!["field".into(), "value".into()],
            rows: vec![
                vec!["name".into(), "Alice".into()],
                vec!["city".into(), "Berlin".into()],
            ],
            ..Default::default()
        };
        a.value_find = input("alice", false);
        a.apply_value_search();
        assert_eq!(a.result.rows.len(), 1);
        assert_eq!(a.result.rows[0][0], "name");
        a.value_find = input("Berlin", false);
        a.apply_value_search();
        assert_eq!(a.result.rows[0][0], "city");
        a.value_find = input("", false);
        a.apply_value_search();
        assert_eq!(a.result.rows.len(), 2);
    }
    #[test]
    fn redis_screen_tree_mouse_and_empty_states_render() {
        let mut a = app();
        a.connecting = false;
        a.redis = true;
        a.focus = 0;
        a.snapshot = BrowserSnapshot {
            keys: vec![crate::redis_browser::KeyMeta {
                label: "\"users:alice\"".into(),
                name: "users:alice".into(),
                kind: "hash".into(),
                ttl_ms: -1,
            }],
            total: Some(1),
            complete: true,
            ..Default::default()
        };
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| a.draw(f)).unwrap();
        let filter = a.redis_controls[3];
        a.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: filter.x,
            row: filter.y,
            modifiers: M::NONE,
        });
        assert!(a.filtering);
        terminal.draw(|f| a.draw(f)).unwrap();
        a.key(KeyEvent::new(KeyCode::Esc, M::NONE));
        a.key(KeyEvent::new(KeyCode::F(6), M::NONE));
        assert!(a.tree);
        a.preview();
        assert_eq!(a.namespace, "users:");
        a.key(KeyEvent::new(KeyCode::Backspace, M::NONE));
        assert!(a.namespace.is_empty());
        for (w, h) in [(120, 40), (55, 26)] {
            terminal = Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| a.draw(f)).unwrap();
            a.open_filters();
            terminal.draw(|f| a.draw(f)).unwrap();
            a.filtering = false;
            a.snapshot.keys.clear();
            terminal.draw(|f| a.draw(f)).unwrap();
        }
    }
    #[test]
    fn ctrl_copy_paste_roundtrips_results_editor_and_filter_fields() {
        let mut a = app();
        a.connecting = false;
        a.focus = 2;
        a.result = QueryResult {
            columns: vec!["value".into()],
            rows: vec![vec!["hello 🌍\nline2".into()]],
            ..Default::default()
        };
        a.key(KeyEvent::new(KeyCode::Char('c'), M::CONTROL));
        a.focus = 1;
        a.editor = input("", false);
        a.key(KeyEvent::new(KeyCode::Char('v'), M::CONTROL));
        assert_eq!(a.editor.lines(), ["hello 🌍", "line2"]);
        a.redis = true;
        a.open_filters();
        a.key_filter = input("", false);
        a.key(KeyEvent::new(KeyCode::Char('v'), M::CONTROL));
        assert_eq!(a.key_filter.lines()[0], "hello 🌍line2");
        a.filtering = false;
        a.focus = 2;
        a.detail = Some("full detail".into());
        a.key(KeyEvent::new(KeyCode::Char('c'), M::CONTROL));
        a.detail = None;
        a.focus = 1;
        a.editor = input("", false);
        a.key(KeyEvent::new(KeyCode::Char('v'), M::CONTROL));
        assert_eq!(a.editor.lines()[0], "full detail");
    }
    #[test]
    fn editor_selection_is_copied_and_paste_is_undoable() {
        let mut a = app();
        a.connecting = false;
        a.focus = 1;
        a.editor = input("hello world", false);
        a.editor.move_cursor(CursorMove::Head);
        a.editor.start_selection();
        a.editor.move_cursor(CursorMove::Jump(0, 5));
        a.key(KeyEvent::new(KeyCode::Char('c'), M::CONTROL));
        assert_eq!(a.clipboard.paste().unwrap(), "hello");
        a.editor = input("", false);
        a.key(KeyEvent::new(KeyCode::Char('v'), M::CONTROL));
        assert_eq!(a.editor.lines()[0], "hello");
        a.key(KeyEvent::new(KeyCode::Char('z'), M::CONTROL));
        assert_eq!(a.editor.lines()[0], "");
    }
    #[tokio::test]
    async fn sql_clipboard_copies_real_results_and_executes_pasted_query() {
        let mut a = app();
        a.connecting = false;
        a.db = Some(Arc::new(Mutex::new(
            Database::connect("sqlite::memory:").await.unwrap(),
        )));
        a.editor = input("SELECT 'SELECT 42 AS answer;' AS query_text;", false);
        a.run_query();
        wait_for_work(&mut a).await;
        // A Redis document must never override the selected SQL result cell.
        a.redis_document = Some("stale Redis text".into());
        a.key(KeyEvent::new(KeyCode::Char('c'), M::CONTROL));
        assert_eq!(a.clipboard.paste().unwrap(), "SELECT 42 AS answer;");
        a.focus = 1;
        a.key(KeyEvent::new(KeyCode::Char('a'), M::CONTROL));
        a.key(KeyEvent::new(KeyCode::Char('v'), M::CONTROL));
        assert_eq!(a.editor.lines()[0], "SELECT 42 AS answer;");
        a.run_query();
        wait_for_work(&mut a).await;
        assert_eq!(a.result.rows[0][0], "42");
        a.focus = 0;
        a.tables = vec![TableName {
            schema: "public".into(),
            name: "users".into(),
        }];
        a.key(KeyEvent::new(KeyCode::Char('c'), M::CONTROL));
        assert_eq!(a.clipboard.paste().unwrap(), "public.users");
    }
    #[tokio::test]
    async fn switching_from_redis_to_sql_clears_inspector_but_preserves_clipboard() {
        let mut a = app();
        a.redis = true;
        a.redis_document = Some("old Redis JSON".into());
        a.redis_key_info = "string · TTL 60s".into();
        a.clipboard.copy("SELECT 7;".into());
        let db = Database::connect("sqlite::memory:").await.unwrap();
        a.tx.send(Message::Connected(
            Ok((db, vec![])),
            SavedConnection::from_url("sqlite::memory:").unwrap(),
        ))
        .unwrap();
        a.poll();
        assert!(!a.redis);
        assert!(a.redis_document.is_none());
        assert!(a.redis_key_info.is_empty());
        assert_eq!(a.clipboard.paste().unwrap(), "SELECT 7;");
    }
}
