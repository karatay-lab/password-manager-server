use std::env;
use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Padding, Paragraph, Row, TableState};
use ratatui::Frame;
use ratatui::Terminal;
use reqwest::Client;
use serde_json::Value;

const PAGE_SIZE: usize = 10;
const TAB_DEVICES: usize = 0;
const TAB_USERS: usize = 1;
const TAB_EXPORT: usize = 2;
const TAB_IMPORT: usize = 3;
const TAB_COUNT: usize = 4;

struct IdentityEntry {
    uuid: String,
    user_id: Option<String>,
    ip_address: String,
    confirmed: bool,
    device_token: String,
    created_at: String,
    updated_at: String,
}

struct UserEntry {
    uuid: String,
    name: String,
    is_deleted: bool,
    identity_count: i64,
    created_at: String,
    updated_at: String,
}

struct App {
    identities: Vec<IdentityEntry>,
    table_state: TableState,
    page: usize,
    total_pages: usize,
    users: Vec<UserEntry>,
    user_state: TableState,
    loading: bool,
    message: String,
    client: Client,
    base: String,
    admin_key: String,
    tab: usize,
    /// Directory exports are written to and imports are read from. Defaults to
    /// `./exports`; in Docker it's set (via `EXPORT_DIR`) to a host-bind-mounted
    /// dir at the project root so the archive is reachable outside the ephemeral
    /// `pwd-admin` container.
    export_dir: String,
    import_path: String,
    busy: bool,
    pending_delete: Option<String>,
    /// Armed when ENTER is pressed on the Export tab and an archive already
    /// exists — the next keypress confirms (`y`) the overwrite or cancels.
    pending_export: bool,
    /// Set when an HTTP request fails to connect (backend not running /
    /// unreachable). When true the UI shows a full-screen "backend down" notice
    /// instead of the tabs, and any of Enter/q/Esc quits so the user can go
    /// start the server.
    backend_down: bool,
}

impl App {
    fn new(client: Client, base: String, admin_key: String, export_dir: String) -> Self {
        let import_path = format!("{export_dir}/pwd-export.tar.gz");
        Self {
            identities: Vec::new(),
            table_state: TableState::default(),
            page: 0,
            total_pages: 0,
            users: Vec::new(),
            user_state: TableState::default(),
            loading: true,
            message: String::new(),
            client,
            base,
            admin_key,
            tab: TAB_DEVICES,
            export_dir,
            import_path,
            busy: false,
            pending_delete: None,
            pending_export: false,
            backend_down: false,
        }
    }

    /// Resolved path of the export archive (`<export_dir>/pwd-export.tar.gz`).
    fn export_path(&self) -> String {
        format!("{}/pwd-export.tar.gz", self.export_dir)
    }

    /// Fetch the export archive from the backend and write it to `export_path()`,
    /// overwriting any existing file. Result (or error) is reported via `message`.
    async fn do_export(&mut self) {
        self.busy = true;
        self.message = String::new();
        let url = format!("{}/admin/export", self.base);
        let resp = self
            .client
            .get(&url)
            .header("admin-key", &self.admin_key)
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => {
                let bytes = r.bytes().await.unwrap();
                let path = self.export_path();
                let _ = std::fs::create_dir_all(&self.export_dir);
                match std::fs::write(&path, &bytes) {
                    Ok(()) => {
                        self.message = format!("Exported {} bytes to {path}", bytes.len());
                    }
                    Err(e) => {
                        self.message = format!("Export: write to {path} failed: {e}");
                    }
                }
            }
            _ => {
                self.message = "Export failed".to_string();
            }
        }
        self.busy = false;
    }

    fn page_start(&self) -> usize {
        self.page * PAGE_SIZE
    }

    fn selected_global_index(&self) -> Option<usize> {
        self.table_state.selected().map(|i| self.page_start() + i)
    }

    async fn fetch_all(&mut self) {
        self.loading = true;
        self.backend_down = false;
        let url = format!("{}/admin/identities", self.base);
        let resp = self
            .client
            .get(&url)
            .header("admin-key", &self.admin_key)
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => {
                let body: Value = r.json().await.unwrap_or(Value::Null);
                let mut entries = Vec::new();
                if let Some(arr) = body.as_array() {
                    for entry in arr {
                        entries.push(IdentityEntry {
                            uuid: entry["uuid"].as_str().unwrap_or("?").to_string(),
                            user_id: entry["user_id"].as_str().map(|s| s.to_string()),
                            ip_address: entry["ip_address"].as_str().unwrap_or("?").to_string(),
                            confirmed: entry["is_confirmed"].as_bool().unwrap_or(false),
                            device_token: entry["device_token"].as_str().unwrap_or("-").to_string(),
                            created_at: entry["created_at"].as_str().unwrap_or("?").to_string(),
                            updated_at: entry["updated_at"].as_str().unwrap_or("?").to_string(),
                        });
                    }
                }
                self.identities = entries;
                self.total_pages = (self.identities.len().max(1) - 1) / PAGE_SIZE + 1;
                if self.page >= self.total_pages {
                    self.page = self.total_pages.saturating_sub(1);
                }
                self.table_state.select(if self.identities.is_empty() {
                    None
                } else {
                    if self.page_start() < self.identities.len() {
                        Some(0)
                    } else {
                        None
                    }
                });
            }
            Ok(_) => {
                self.message = "Failed to fetch identities".to_string();
            }
            Err(_) => {
                self.backend_down = true;
                self.message = "Failed to fetch identities".to_string();
            }
        }
        self.loading = false;
    }

    async fn toggle_selected(&mut self) {
        let Some(global_idx) = self.selected_global_index() else {
            return;
        };
        let uuid = self.identities[global_idx].uuid.clone();
        let was_confirmed = self.identities[global_idx].confirmed;
        let action = if was_confirmed {
            "unconfirm"
        } else {
            "confirm"
        };
        let url = format!("{}/admin/identities/{uuid}/{action}", self.base);
        let resp = self
            .client
            .post(&url)
            .header("admin-key", &self.admin_key)
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => {
                self.identities[global_idx].confirmed = !was_confirmed;
                self.message = format!("{action}ed {uuid}");
            }
            _ => {
                self.message = format!("Failed to {action} {uuid}");
            }
        }
    }

    fn arm_delete(&mut self) {
        let Some(global_idx) = self.selected_global_index() else {
            return;
        };
        if global_idx >= self.identities.len() {
            return;
        }
        let uuid = self.identities[global_idx].uuid.clone();
        self.message =
            format!("Delete device {uuid}? (the user's passwords are kept) Press 'y' to confirm");
        self.pending_delete = Some(uuid);
    }

    async fn delete_pending(&mut self) {
        let Some(uuid) = self.pending_delete.take() else {
            return;
        };
        let url = format!("{}/admin/identities/{uuid}", self.base);
        let resp = self
            .client
            .delete(&url)
            .header("admin-key", &self.admin_key)
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => {
                self.message = format!("Deleted {uuid}");
                self.fetch_all().await;
            }
            Ok(r) => {
                self.message = format!("Failed to delete {uuid} ({})", r.status());
            }
            Err(_) => {
                self.message = format!("Failed to delete {uuid}");
            }
        }
    }

    async fn fetch_users(&mut self) {
        self.backend_down = false;
        let url = format!("{}/admin/users", self.base);
        let resp = self
            .client
            .get(&url)
            .header("admin-key", &self.admin_key)
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => {
                let body: Value = r.json().await.unwrap_or(Value::Null);
                let mut entries = Vec::new();
                if let Some(arr) = body.as_array() {
                    for entry in arr {
                        entries.push(UserEntry {
                            uuid: entry["uuid"].as_str().unwrap_or("?").to_string(),
                            name: entry["name"].as_str().unwrap_or("?").to_string(),
                            is_deleted: entry["is_deleted"].as_bool().unwrap_or(false),
                            identity_count: entry["identity_count"].as_i64().unwrap_or(0),
                            created_at: entry["created_at"].as_str().unwrap_or("?").to_string(),
                            updated_at: entry["updated_at"].as_str().unwrap_or("?").to_string(),
                        });
                    }
                }
                self.users = entries;
                self.user_state.select(if self.users.is_empty() {
                    None
                } else {
                    Some(
                        self.user_state
                            .selected()
                            .unwrap_or(0)
                            .min(self.users.len() - 1),
                    )
                });
            }
            Ok(_) => {
                self.message = "Failed to fetch users".to_string();
            }
            Err(_) => {
                self.backend_down = true;
                self.message = "Failed to fetch users".to_string();
            }
        }
    }

    async fn toggle_user_deleted(&mut self, set_deleted: bool) {
        let Some(idx) = self.user_state.selected() else {
            return;
        };
        let Some(user) = self.users.get(idx) else {
            return;
        };
        let uuid = user.uuid.clone();
        let action = if set_deleted { "delete" } else { "restore" };
        let url = format!("{}/admin/users/{uuid}/{action}", self.base);
        let resp = self
            .client
            .post(&url)
            .header("admin-key", &self.admin_key)
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => {
                self.message = format!("{action}d user {uuid}");
                self.fetch_users().await;
            }
            Ok(r) => {
                self.message = format!("Failed to {action} user {uuid} ({})", r.status());
            }
            Err(_) => {
                self.message = format!("Failed to {action} user {uuid}");
            }
        }
    }

    /// Count `(confirmed, total)` devices belonging to a user. A user is
    /// "approved" once at least one of their devices is confirmed; passwords are
    /// gated per-device on the server (`identity.is_confirmed`).
    fn user_conf(&self, user_uuid: &str) -> (usize, usize) {
        let mut confirmed = 0;
        let mut total = 0;
        for i in &self.identities {
            if i.user_id.as_deref() == Some(user_uuid) {
                total += 1;
                if i.confirmed {
                    confirmed += 1;
                }
            }
        }
        (confirmed, total)
    }

    /// Approve a user by confirming all of their devices; if every device is
    /// already confirmed, revoke them all instead (toggle). Granular per-device
    /// control still lives on the Devices tab.
    async fn toggle_user_approval(&mut self) {
        let Some(idx) = self.user_state.selected() else {
            return;
        };
        let Some(user) = self.users.get(idx) else {
            return;
        };
        let user_uuid = user.uuid.clone();
        let devices: Vec<String> = self
            .identities
            .iter()
            .filter(|i| i.user_id.as_deref() == Some(user_uuid.as_str()))
            .map(|i| i.uuid.clone())
            .collect();
        let all_confirmed = self
            .identities
            .iter()
            .filter(|i| i.user_id.as_deref() == Some(user_uuid.as_str()))
            .all(|i| i.confirmed);

        if devices.is_empty() {
            self.message = format!("User {user_uuid} has no devices to approve");
            return;
        }

        let confirm = !all_confirmed;
        let action = if confirm { "confirm" } else { "unconfirm" };
        let mut ok = 0usize;
        for uuid in &devices {
            let url = format!("{}/admin/identities/{uuid}/{action}", self.base);
            if let Ok(r) = self
                .client
                .post(&url)
                .header("admin-key", &self.admin_key)
                .send()
                .await
            {
                if r.status().is_success() {
                    ok += 1;
                    // Optimistically reflect the new state locally so the Conf
                    // column flips immediately — even if the follow-up refresh
                    // GET is unavailable or rate-limited (matches the Devices
                    // tab's toggle_selected behaviour).
                    if let Some(ident) = self.identities.iter_mut().find(|i| &i.uuid == uuid) {
                        ident.confirmed = confirm;
                    }
                }
            }
        }
        let verb = if confirm { "approved" } else { "revoked" };
        // Reconcile with the server; if this GET fails the optimistic update
        // above keeps the Conf column correct. Set the result message *after*
        // so it isn't clobbered by fetch_all's own error message.
        self.fetch_all().await;
        self.message = format!("{verb} {ok}/{} device(s) for {user_uuid}", devices.len());
    }

    fn next_user(&mut self) {
        if self.users.is_empty() {
            return;
        }
        let i = self.user_state.selected().unwrap_or(0);
        self.user_state
            .select(Some((i + 1).min(self.users.len() - 1)));
    }

    fn prev_user(&mut self) {
        let i = self.user_state.selected().unwrap_or(0);
        if i > 0 {
            self.user_state.select(Some(i - 1));
        }
    }

    fn next_page(&mut self) {
        if self.page + 1 < self.total_pages {
            self.page += 1;
            self.table_state.select(Some(0));
        }
    }

    fn prev_page(&mut self) {
        if self.page > 0 {
            self.page -= 1;
            self.table_state.select(Some(0));
        }
    }

    fn next_row(&mut self) {
        let items_len = self.identities.len();
        let start = self.page_start();
        let end = (start + PAGE_SIZE).min(items_len);
        let page_count = if start < items_len { end - start } else { 0 };
        if page_count == 0 {
            return;
        }
        let i = self.table_state.selected().unwrap_or(0);
        let next = (i + 1).min(page_count.saturating_sub(1));
        self.table_state.select(Some(next));
    }

    fn prev_row(&mut self) {
        let i = self.table_state.selected().unwrap_or(0);
        if i > 0 {
            self.table_state.select(Some(i - 1));
        }
    }
}

fn render_tab_bar(app: &App) -> Line<'_> {
    let tabs = ["Devices", "Users", "Export", "Import"];
    let spans: Vec<Span> = tabs
        .iter()
        .enumerate()
        .flat_map(|(i, label)| {
            let selected = i == app.tab;
            let style = if selected {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default().fg(Color::Cyan)
            };
            let sep = if i > 0 {
                Span::from("  ")
            } else {
                Span::from(" ")
            };
            vec![sep, Span::styled(format!(" {label} "), style)]
        })
        .collect();
    Line::from(spans)
}

/// One-line info bar showing what each key does on the active tab. Rendered
/// directly under the tab bar so the controls are always visible.
fn render_key_bar(app: &App) -> Line<'static> {
    let key = |k: String| {
        Span::styled(
            format!(" {k} "),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    };
    let desc = |d: &str| Span::styled(format!(" {d}   "), Style::default().fg(Color::Gray));

    // While a delete or overwrite is armed the whole UI is a yes/no modal —
    // show that instead of the normal per-tab keys.
    if app.pending_delete.is_some() || app.pending_export {
        let confirm = if app.pending_export {
            "confirm overwrite"
        } else {
            "confirm delete"
        };
        return Line::from(vec![
            Span::styled(
                " y ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            desc(confirm),
            key("any other key".to_string()),
            desc("cancel"),
        ]);
    }

    let pairs: &[(&str, &str)] = match app.tab {
        TAB_DEVICES => &[
            ("↑/↓", "select"),
            ("←/→", "page"),
            ("Space/Enter", "confirm/unconfirm"),
            ("d", "delete device"),
            ("Tab", "next tab"),
            ("q", "quit"),
        ],
        TAB_USERS => &[
            ("↑/↓", "select"),
            ("Space/Enter", "approve/revoke devices"),
            ("d", "soft-delete"),
            ("r", "restore"),
            ("Tab", "next tab"),
            ("q", "quit"),
        ],
        TAB_EXPORT => &[("Enter", "export"), ("Tab", "next tab"), ("q", "quit")],
        TAB_IMPORT => &[("Enter", "import"), ("Tab", "next tab"), ("q", "quit")],
        _ => &[],
    };

    let mut spans = Vec::with_capacity(pairs.len() * 2);
    for (k, d) in pairs {
        spans.push(key((*k).to_string()));
        spans.push(desc(d));
    }
    Line::from(spans)
}

fn render_devices(frame: &mut Frame, app: &App, area: Rect) {
    let selected = app.table_state.selected().unwrap_or(0);
    let start = app.page * PAGE_SIZE;
    let end = (start + PAGE_SIZE).min(app.identities.len());

    if start >= app.identities.len() || app.identities.is_empty() {
        let text = Text::from(" No identities found");
        frame.render_widget(text, area);
        return;
    }

    let trunc = |s: &str, max: usize| -> String {
        if s.len() > max {
            format!("{}...", &s[..max])
        } else {
            s.to_string()
        }
    };

    let page_entries: Vec<(String, String, bool, String, String, String)> = app.identities
        [start..end]
        .iter()
        .map(|e| {
            (
                e.uuid.clone(),
                e.ip_address.clone(),
                e.confirmed,
                trunc(&e.device_token, 45),
                e.created_at.clone(),
                e.updated_at.clone(),
            )
        })
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Length(16),
        Constraint::Length(16),
        Constraint::Length(49),
        Constraint::Length(19),
        Constraint::Length(19),
        Constraint::Length(10),
    ];
    let header_cells = [
        "",
        "Created",
        "Updated",
        "Device Token",
        "UUID",
        "IP",
        "Conf",
    ];
    let header = Row::new(header_cells.iter().map(|c| Text::from(*c))).style(
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Cyan),
    );

    let rows: Vec<Row> = page_entries
        .iter()
        .enumerate()
        .map(
            |(idx, (uuid, ip, confirmed, device_token, created_at, updated_at))| {
                let mark = if *confirmed { "✓" } else { "  " };
                let cells = [
                    Text::from(mark),
                    Text::from(created_at.as_str()),
                    Text::from(updated_at.as_str()),
                    Text::from(device_token.as_str()),
                    Text::from(uuid.as_str()),
                    Text::from(ip.as_str()),
                    Text::from(if *confirmed { "Yes" } else { "No" }),
                ];
                let style = if selected == idx {
                    Style::default()
                        .bg(Color::DarkGray)
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Row::new(cells).style(style)
            },
        )
        .collect();

    let table = ratatui::widgets::Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().borders(Borders::NONE));

    frame.render_stateful_widget(table, area, &mut app.table_state.clone());
}

fn render_users(frame: &mut Frame, app: &App, area: Rect) {
    if app.users.is_empty() {
        frame.render_widget(Text::from(" No users found"), area);
        return;
    }

    let selected = app.user_state.selected().unwrap_or(0);
    let widths = [
        Constraint::Length(24),
        Constraint::Length(38),
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Length(9),
        Constraint::Length(21),
        Constraint::Length(21),
    ];
    let header = Row::new(
        [
            "Name", "UUID", "Devices", "Conf", "Deleted", "Created", "Updated",
        ]
        .iter()
        .map(|c| Text::from(*c)),
    )
    .style(
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Cyan),
    );

    let rows: Vec<Row> = app
        .users
        .iter()
        .enumerate()
        .map(|(idx, u)| {
            // Approval status is derived from the user's devices (server gates
            // passwords per-device on identity.is_confirmed).
            let (confirmed, total) = app.user_conf(&u.uuid);
            let (conf_text, conf_color) = if total == 0 {
                ("—".to_string(), Color::DarkGray)
            } else if confirmed == total {
                ("Yes".to_string(), Color::Green)
            } else if confirmed == 0 {
                ("No".to_string(), Color::Red)
            } else {
                (format!("{confirmed}/{total}"), Color::Yellow)
            };
            let cells = [
                Cell::from(u.name.as_str()),
                Cell::from(u.uuid.as_str()),
                Cell::from(u.identity_count.to_string()),
                Cell::from(conf_text)
                    .style(Style::default().fg(conf_color).add_modifier(Modifier::BOLD)),
                Cell::from(if u.is_deleted { "Yes" } else { "No" }),
                Cell::from(u.created_at.as_str()),
                Cell::from(u.updated_at.as_str()),
            ];
            let mut style = if selected == idx {
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            if u.is_deleted {
                style = style.fg(Color::Red);
            }
            Row::new(cells).style(style)
        })
        .collect();

    let table = ratatui::widgets::Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().borders(Borders::NONE));
    frame.render_stateful_widget(table, area, &mut app.user_state.clone());
}

fn render_export(frame: &mut Frame, app: &App, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from(" Export All Data"),
        Line::from(""),
        Line::from(" Downloads the database, secrets (.env), and a README"),
        Line::from(" as a single pwd-export.tar.gz file."),
        Line::from(""),
        Line::from(format!(" Writes to: {}", app.export_path())),
        Line::from(""),
        Line::from(" Press ENTER to export"),
        Line::from(""),
        if app.busy {
            Line::from(" Exporting...")
        } else if !app.message.is_empty() {
            Line::from(app.message.as_str())
        } else {
            Line::from("")
        },
    ];
    let p = Paragraph::new(lines).style(Style::default().fg(Color::White));
    frame.render_widget(p, area);
}

fn render_import(frame: &mut Frame, app: &App, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from(" Import Data"),
        Line::from(""),
        Line::from(" Drop a .tar.gz export into the shared export dir on the"),
        Line::from(" host, then press ENTER to import from the path below."),
        Line::from(""),
        Line::from(format!(" Host dir mounts to: {}", app.export_dir)),
        Line::from(""),
        Line::from(format!(" Path: {}", app.import_path)),
        Line::from(""),
        if app.busy {
            Line::from(" Importing...")
        } else if !app.message.is_empty() {
            Line::from(app.message.as_str())
        } else {
            Line::from("")
        },
    ];
    let p = Paragraph::new(lines).style(Style::default().fg(Color::White));
    frame.render_widget(p, area);
}

/// Full-screen notice shown when the backend can't be reached. Replaces the
/// normal tabbed UI so the operator immediately sees what's wrong and how to
/// get out (the data tables would be empty/stale anyway).
fn render_backend_down(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered()
        .title(" Backend Unavailable ")
        .title_alignment(Alignment::Center)
        .border_style(Style::default().fg(Color::Red))
        .padding(Padding::uniform(1));
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  ⚠  The backend server is not running.",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("  Could not reach {}", app.base)),
        Line::from(""),
        Line::from("  Start the backend server, then relaunch this admin CLI."),
        Line::from(""),
        Line::from(Span::styled(
            "  Press Enter (or q / Ctrl+C) to quit now.",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    let p = Paragraph::new(lines)
        .block(block)
        .style(Style::default().fg(Color::White));
    frame.render_widget(p, area);
}

fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Backend unreachable: show only the notice; the tabs would be empty.
    if app.backend_down {
        render_backend_down(frame, app, area);
        return;
    }

    // Outer frame so the content sits in a padded panel instead of being flush
    // against the terminal edges.
    let outer = Block::bordered()
        .title(" PWD Manager — Admin ")
        .title_alignment(Alignment::Center)
        .border_style(Style::default().fg(Color::Cyan))
        .padding(Padding::horizontal(1));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let [tab_bar, key_bar, sep, content, footer_area] = Layout::vertical([
        Constraint::Length(1), // tabs
        Constraint::Length(1), // info bar
        Constraint::Length(1), // separator
        Constraint::Min(0),    // content
        Constraint::Length(1), // footer
    ])
    .areas(inner);

    // Tab bar: Devices | Users | Export | Import.
    frame.render_widget(render_tab_bar(app), tab_bar);

    // Info bar: what each key does on the active tab.
    frame.render_widget(render_key_bar(app), key_bar);

    // Separator between the header and the content.
    frame.render_widget(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
        sep,
    );

    let page_info = match app.tab {
        TAB_DEVICES => format!(
            "Page {}/{}  |  {} identities  |  {}",
            app.page + 1,
            app.total_pages,
            app.identities.len(),
            app.message,
        ),
        TAB_USERS => format!("{} users  |  {}", app.users.len(), app.message),
        TAB_EXPORT | TAB_IMPORT => String::new(),
        _ => String::new(),
    };
    let footer = Paragraph::new(Text::from(page_info)).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, footer_area);

    match app.tab {
        TAB_DEVICES => render_devices(frame, app, content),
        TAB_USERS => render_users(frame, app, content),
        TAB_EXPORT => render_export(frame, app, content),
        TAB_IMPORT => render_import(frame, app, content),
        _ => {}
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let base = std::env::var("BACKEND_URL").expect("BACKEND_URL must be set");
    let admin_key = std::env::var("ADMIN_KEY").expect("ADMIN_KEY must be set");
    // Where export archives are written / imports read from. In Docker this is a
    // host-bind-mounted dir at the project root (EXPORT_DIR=/exports) so the file
    // is reachable on the host; outside Docker it falls back to ./exports under
    // the current working directory. Never /tmp — that isn't mounted to the host.
    let export_dir = std::env::var("EXPORT_DIR").unwrap_or_else(|_| "./exports".to_string());
    // Bounded timeouts so an unreachable backend fails fast instead of hanging
    // the (blocking) initial fetch behind a blank alternate screen.
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(15))
        .build()
        .expect("failed to build HTTP client");

    let args: Vec<String> = env::args().collect();
    if args.len() >= 2 {
        let cmd = &args[1];
        match cmd.as_str() {
            "list" => {
                let resp = client
                    .get(format!("{base}/admin/identities"))
                    .header("admin-key", &admin_key)
                    .send()
                    .await
                    .unwrap();
                let status = resp.status().as_u16();
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                let trunc = |s: &str, max: usize| -> String {
                    if s.len() > max {
                        format!("{}...", &s[..max])
                    } else {
                        s.to_string()
                    }
                };
                println!("Status: {status}");
                if let Some(arr) = body.as_array() {
                    for entry in arr {
                        let uuid = entry["uuid"].as_str().unwrap_or("?");
                        let confirmed = entry["is_confirmed"].as_bool().unwrap_or(false);
                        let device = trunc(entry["device_token"].as_str().unwrap_or("-"), 45);
                        let created = entry["created_at"].as_str().unwrap_or("?");
                        let updated = entry["updated_at"].as_str().unwrap_or("?");
                        let ip = entry["ip_address"].as_str().unwrap_or("?");
                        let user = entry["user_id"].as_str().unwrap_or("-");
                        let mark = if confirmed { "✓" } else { "✗" };
                        println!("  [{mark}] {uuid}");
                        println!("         user:    {user}");
                        println!("         ip:      {ip}");
                        println!("         device:  {device}");
                        println!("         created: {created}");
                        println!("         updated: {updated}");
                    }
                } else {
                    println!("{body:#}");
                }
            }
            "confirm" | "unconfirm" => {
                let uuid = args
                    .get(2)
                    .expect("Usage: admin_cli confirm|unconfirm <uuid>");
                let resp = client
                    .post(format!("{base}/admin/identities/{uuid}/{cmd}"))
                    .header("admin-key", &admin_key)
                    .send()
                    .await
                    .unwrap();
                let status = resp.status().as_u16();
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                println!("Status: {status}");
                if status == 200 {
                    println!("{cmd}ed {uuid}");
                } else {
                    println!("{body:#}");
                }
            }
            "delete" => {
                let uuid = args.get(2).expect("Usage: admin_cli delete <uuid>");
                let resp = client
                    .delete(format!("{base}/admin/identities/{uuid}"))
                    .header("admin-key", &admin_key)
                    .send()
                    .await
                    .unwrap();
                let status = resp.status().as_u16();
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                println!("Status: {status}");
                if status == 200 {
                    println!("deleted {uuid}");
                } else {
                    println!("{body:#}");
                }
            }
            "users" => {
                let resp = client
                    .get(format!("{base}/admin/users"))
                    .header("admin-key", &admin_key)
                    .send()
                    .await
                    .unwrap();
                let status = resp.status().as_u16();
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                println!("Status: {status}");
                if let Some(arr) = body.as_array() {
                    for entry in arr {
                        let uuid = entry["uuid"].as_str().unwrap_or("?");
                        let name = entry["name"].as_str().unwrap_or("?");
                        let deleted = entry["is_deleted"].as_bool().unwrap_or(false);
                        let count = entry["identity_count"].as_i64().unwrap_or(0);
                        let created = entry["created_at"].as_str().unwrap_or("?");
                        let mark = if deleted { "deleted" } else { "active " };
                        println!("  [{mark}] {name}  {uuid}  devices:{count}  created:{created}");
                    }
                } else {
                    println!("{body:#}");
                }
            }
            "user-delete" | "user-restore" => {
                let uuid = args
                    .get(2)
                    .expect("Usage: admin_cli user-delete|user-restore <uuid>");
                let action = if cmd == "user-delete" {
                    "delete"
                } else {
                    "restore"
                };
                let resp = client
                    .post(format!("{base}/admin/users/{uuid}/{action}"))
                    .header("admin-key", &admin_key)
                    .send()
                    .await
                    .unwrap();
                let status = resp.status().as_u16();
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                println!("Status: {status}");
                if status == 200 {
                    println!("{action}d user {uuid}");
                } else {
                    println!("{body:#}");
                }
            }
            "export" => {
                let resp = client
                    .get(format!("{base}/admin/export"))
                    .header("admin-key", &admin_key)
                    .send()
                    .await
                    .unwrap();
                let status = resp.status().as_u16();
                if status == 200 {
                    let bytes = resp.bytes().await.unwrap();
                    let path = format!("{export_dir}/pwd-export.tar.gz");
                    let _ = std::fs::create_dir_all(&export_dir);
                    std::fs::write(&path, &bytes).unwrap();
                    println!("Exported {} bytes to {path}", bytes.len());
                } else {
                    println!("Export failed: {status}");
                }
            }
            "import" => {
                let default_path = format!("{export_dir}/pwd-export.tar.gz");
                let path = args.get(2).map(|s| s.as_str()).unwrap_or(&default_path);
                let data = std::fs::read(path).unwrap_or_else(|e| {
                    eprintln!("Failed to read {path}: {e}");
                    std::process::exit(1);
                });
                let resp = client
                    .post(format!("{base}/admin/import"))
                    .header("admin-key", &admin_key)
                    .body(data)
                    .send()
                    .await
                    .unwrap();
                let status = resp.status().as_u16();
                println!("Status: {status}");
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                if status == 200 {
                    println!("Import successful");
                } else {
                    println!("{body:#}");
                }
            }
            _ => {
                eprintln!(
                    "Usage: admin_cli [list|confirm|unconfirm|delete|users|user-delete|user-restore|export|import [path]]"
                );
            }
        }
        return Ok(());
    }

    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(client, base, admin_key, export_dir);
    app.fetch_all().await;
    app.fetch_users().await;

    let res = run_tui(&mut terminal, &mut app).await;

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res
}

async fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| render(f, &mut *app))?;

        if event::poll(Duration::from_millis(250))
            .ok()
            .unwrap_or(false)
        {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            // Global quit: Ctrl+C / Ctrl+D always exit, even mid-modal. In raw
            // mode the terminal doesn't generate SIGINT, so handle it here.
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
            {
                break;
            }
            // Backend unreachable: the UI is just a notice — Enter/q/Esc quits so
            // the user can go start the server, then relaunch.
            if app.backend_down {
                match key.code {
                    KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                        break
                    }
                    _ => {}
                }
                continue;
            }
            // Pending delete acts as a modal: only 'y' confirms, anything cancels.
            if app.pending_delete.is_some() {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => app.delete_pending().await,
                    _ => {
                        app.pending_delete = None;
                        app.message = "Delete cancelled".to_string();
                    }
                }
                continue;
            }
            // Pending export overwrite: 'y' overwrites the existing archive,
            // anything else cancels.
            if app.pending_export {
                app.pending_export = false;
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => app.do_export().await,
                    _ => app.message = "Export cancelled".to_string(),
                }
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Tab => {
                    app.tab = (app.tab + 1) % TAB_COUNT;
                    app.message = String::new();
                }
                KeyCode::BackTab => {
                    app.tab = (app.tab + TAB_COUNT - 1) % TAB_COUNT;
                    app.message = String::new();
                }
                KeyCode::Enter => match app.tab {
                    TAB_EXPORT if !app.busy => {
                        // Don't clobber an existing archive without asking — arm
                        // a yes/no modal; the export runs only on confirmation.
                        if std::path::Path::new(&app.export_path()).exists() {
                            app.pending_export = true;
                            app.message = format!(
                                "{} already exists — overwrite? (y = yes, any other key = cancel)",
                                app.export_path()
                            );
                        } else {
                            app.do_export().await;
                        }
                    }
                    TAB_IMPORT if !app.busy => {
                        app.busy = true;
                        app.message = String::new();
                        let path = app.import_path.clone();
                        let data = match std::fs::read(&path) {
                            Ok(d) => d,
                            Err(e) => {
                                app.message = format!("Cannot read {path}: {e}");
                                app.busy = false;
                                continue;
                            }
                        };
                        let url = format!("{}/admin/import", app.base);
                        let resp = app
                            .client
                            .post(&url)
                            .header("admin-key", &app.admin_key)
                            .body(data)
                            .send()
                            .await;
                        match resp {
                            Ok(r) if r.status().is_success() => {
                                app.busy = false;
                                // The server hot-reloads its DB pool on import, so
                                // re-fetching pulls the just-imported data straight
                                // into the tables — no admin-CLI restart needed.
                                app.fetch_all().await;
                                app.fetch_users().await;
                                app.page = 0;
                                app.message = "Import successful — data reloaded".to_string();
                            }
                            Ok(r) => {
                                app.busy = false;
                                app.message = format!("Import failed ({})", r.status());
                            }
                            Err(_) => {
                                app.busy = false;
                                app.message = "Import failed".to_string();
                            }
                        }
                    }
                    _ => {
                        if app.tab == TAB_DEVICES {
                            app.toggle_selected().await;
                        } else if app.tab == TAB_USERS {
                            app.toggle_user_approval().await;
                        }
                    }
                },
                KeyCode::Up => {
                    if app.tab == TAB_DEVICES {
                        app.prev_row();
                    } else if app.tab == TAB_USERS {
                        app.prev_user();
                    }
                }
                KeyCode::Down => {
                    if app.tab == TAB_DEVICES {
                        app.next_row();
                    } else if app.tab == TAB_USERS {
                        app.next_user();
                    }
                }
                KeyCode::Left if app.tab == TAB_DEVICES => {
                    app.prev_page();
                }
                KeyCode::Right if app.tab == TAB_DEVICES => {
                    app.next_page();
                }
                KeyCode::Char(' ') => {
                    if app.tab == TAB_DEVICES {
                        app.toggle_selected().await;
                    } else if app.tab == TAB_USERS {
                        app.toggle_user_approval().await;
                    }
                }
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    if app.tab == TAB_DEVICES {
                        app.arm_delete();
                    } else if app.tab == TAB_USERS {
                        app.toggle_user_deleted(true).await;
                    }
                }
                KeyCode::Char('r') | KeyCode::Char('R') if app.tab == TAB_USERS => {
                    app.toggle_user_deleted(false).await;
                }
                _ => {}
            }
        }
    }
    Ok(())
}
