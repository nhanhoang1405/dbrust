use crate::{
    connection::{History, SavedConnection, manual_url},
    db::{Database, QueryResult, TableName},
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
    Tables(Result<Vec<TableName>>),
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
                            Ok(()) => "Connected. Select a table and Enter to preview, or edit SQL and press F5.".into(),
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
                                " · display capped at 10,000 rows"
                            } else {
                                ""
                            }
                        );
                        self.result = r;
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
                    Ok(t) => {
                        self.tables = t;
                        self.selected_table =
                            self.selected_table.min(self.tables.len().saturating_sub(1));
                        self.status = "Table list refreshed.".into();
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
        if self.busy {
            return;
        }
        let Some(db) = self.db.clone() else {
            return;
        };
        self.busy = true;
        self.status = "Refreshing tables…".into();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = db.lock().await.tables().await;
            let _ = tx.send(Message::Tables(result));
        });
    }
    fn preview(&mut self) {
        if self.busy {
            return;
        }
        if let Some(table) = self.tables.get(self.selected_table) {
            let mysql = self.label.starts_with("mysql:");
            self.editor = input(&table.preview_sql(mysql), false);
            self.run_query();
        }
    }
    pub fn paste(&mut self, value: &str) {
        if self.connecting {
            if let Some(field) = self.form.fields.get_mut(self.form.focus.wrapping_sub(1)) {
                field.insert_str(value.replace(['\r', '\n'], ""));
            }
        } else if self.focus == 1 {
            self.editor.insert_str(value);
        }
    }
    pub fn key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('q') && key.modifiers.contains(M::CONTROL) {
            self.quit = true;
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
        match key.code {
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
                        3
                    } else {
                        1
                    })
                    % 4;
            }
            KeyCode::Esc => self.focus = 3,
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
                            (self.selected_table + 1).min(self.tables.len().saturating_sub(1))
                    }
                    KeyCode::Enter => self.preview(),
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
                    KeyCode::Enter => self.focus = 1,
                    _ => {}
                },
                _ => {
                    if key.code == KeyCode::Enter {
                        self.run_query();
                    }
                }
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
                let drivers = ["postgres", "mysql", "sqlite"];
                let i = drivers.iter().position(|v| *v == f.value(0)).unwrap_or(0);
                f.fields[0] = input(
                    drivers[(i + if key.code == KeyCode::Left { 2 } else { 1 }) % 3],
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
    fn max_x(&self) -> usize {
        (self.result.columns.len() * CELL_WIDTH).saturating_sub(self.grid.width as usize)
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
                        self.tables
                            .len()
                            .saturating_sub(self.panes[0].height.saturating_sub(2) as usize),
                    );
                *self.table_state.offset_mut() = self.table_scroll;
                let h = self.panes[0].height.saturating_sub(2).max(1) as usize;
                self.selected_table = self
                    .selected_table
                    .clamp(self.table_scroll, self.table_scroll + h - 1)
                    .min(self.tables.len().saturating_sub(1));
                let max = self
                    .tables
                    .iter()
                    .map(|t| t.label().width())
                    .max()
                    .unwrap_or(0)
                    .saturating_sub(self.panes[0].width.saturating_sub(4) as usize);
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
                        .min(self.tables.len().saturating_sub(1));
                    self.preview();
                } else if i == 1 {
                    click_text(&mut self.editor, self.panes[1], point);
                } else if i == 2 && self.grid.contains(point) && mouse.row > self.grid.y {
                    self.row = (self.y + (mouse.row - self.grid.y - 1) as usize)
                        .min(self.result.rows.len().saturating_sub(1));
                    self.col = ((self.x + (mouse.column - self.grid.x) as usize) / CELL_WIDTH)
                        .min(self.result.columns.len().saturating_sub(1));
                }
            }
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
            Paragraph::new(" dbrust / SQL workbench").style(Style::default().fg(ACCENT).bold()),
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
                    .block(panel("", !self.connecting && self.focus == 3 && i == 0)),
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
        if self.help {
            let popup = centered(area, 80, 25);
            frame.render_widget(Clear, popup);
            frame.render_widget(Paragraph::new("Navigate with Tab / Shift+Tab; the mint border shows focus.\n\nConnections: F2 changes method. Use URL, manual fields, or history.\nManual drivers: postgres, mysql, sqlite. SQLite uses Database as a path.\nPasswords are masked, never saved; history asks for them again.\n\nTables: ↑↓ selects; Enter loads and executes a 1000-row preview.\nEditor: arrow keys edit; Enter adds a line; paste and undo supported.\nExecute: F5, Ctrl+Enter, or Tab to Run and press Enter.\nResults: arrows select cells; full value appears below. Enter edits SQL.\nShift+arrows scroll 1 unit; Ctrl/Alt+arrows scroll 10 units.\nPageUp/PageDown scroll results by a page. Home/End jump.\nMouse: click a table to preview; click panels, fields, cells, or buttons.\nWheel scrolls vertically; Shift+wheel or horizontal wheel scrolls sideways.\nClick or drag result scrollbars to jump.\n\nF3 refreshes tables. F2 switches connections; Esc returns.\nCtrl+Q quits. Queries execute as written, including writes.\nDisplay keeps 10,000 rows from the last nonempty result set.\nUse SQL LIMIT for large queries; all submitted statements still execute.\n\nF1 / Esc / Enter / click closes help.").wrap(Wrap{trim:false}).block(panel(" Help ",true)).style(Style::default().bg(BG)),popup);
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
                    "Driver ←→ postgres / mysql / sqlite",
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
    fn draw_workspace(&mut self, frame: &mut Frame, area: Rect) {
        let cols =
            Layout::horizontal([Constraint::Percentage(23), Constraint::Min(25)]).split(area);
        let right =
            Layout::vertical([Constraint::Percentage(35), Constraint::Min(5)]).split(cols[1]);
        self.panes = [cols[0], right[0], right[1]];
        let items: Vec<ListItem> = self
            .tables
            .iter()
            .map(|t| ListItem::new(drop_columns(&t.label(), self.table_x)))
            .collect();
        self.table_state.select(if items.is_empty() {
            None
        } else {
            Some(self.selected_table)
        });
        frame.render_stateful_widget(
            List::new(items)
                .block(panel(" Tables · Enter preview ", self.focus == 0))
                .highlight_style(selected())
                .highlight_symbol("› "),
            cols[0],
            &mut self.table_state,
        );
        self.editor
            .set_block(panel(" SQL editor · F5 execute ", self.focus == 1));
        self.editor.set_cursor_style(if self.focus == 1 {
            Style::default().reversed()
        } else {
            Style::default()
        });
        frame.render_widget(&self.editor, right[0]);
        frame.render_widget(
            panel(&format!(" Results · {} ", self.label), self.focus == 2),
            right[1],
        );
        let inner = right[1].inner(Margin::new(1, 1));
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
        let mut lines = vec![Line::from(
            self.result
                .columns
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    Span::styled(
                        cell_text(c),
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
                                    cell_text(c),
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
            lines = vec![Line::from("Run a query to see results here.")];
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
fn cell_text(value: &str) -> String {
    let mut out = String::new();
    let mut width = 0;
    for c in value.chars() {
        let c = if c.is_control() { ' ' } else { c };
        let w = c.width().unwrap_or(0);
        if width + w > CELL_WIDTH - 2 {
            break;
        }
        out.push(c);
        width += w;
    }
    out.push_str(&" ".repeat(CELL_WIDTH - 1 - width));
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
        assert_eq!(a.focus, 3);
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
        assert_eq!(cell_text("hello 世界 🌍").width(), CELL_WIDTH);
        assert_eq!(cell_text("a\nb\t").width(), CELL_WIDTH);
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
}
