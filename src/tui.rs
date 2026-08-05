//! Terminal input and rendering.
//!
//! The TUI reports user intent and renders application state; it does not
//! construct or execute OpenSSH commands.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::application::{AppState, ForwardRuleDraft, PortProbe};
use crate::domain::{ForwardRuleId, ForwardState, SessionState, TargetId};
use crate::ssh::SshClient;

/// Pane receiving navigation keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// Target/session list.
    Targets,
    /// Saved/runtime forward list.
    Forwards,
}

/// Destructive action awaiting confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirmation {
    /// Stop one SSH session.
    Disconnect(TargetId),
    /// Cancel if needed and delete a saved rule.
    DeleteForward(ForwardRuleId),
    /// Stop all sessions and exit.
    Quit,
}

/// Current UI interaction mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Normal list navigation.
    Normal,
    /// Editing a new forward rule.
    ForwardForm,
    /// Warning before accepting a public bind.
    PublicBindWarning,
    /// Explicit destructive-action confirmation.
    Confirm(Confirmation),
    /// Showing complete diagnostic detail.
    ErrorDetails,
}

/// Editable forward-rule form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardForm {
    /// Zero-based active field.
    pub selected_field: usize,
    /// Optional label.
    pub label: String,
    /// Local bind address, loopback by default.
    pub bind_address: String,
    /// Preferred local port; blank means remote port.
    pub local_port: String,
    /// Remote-side destination host, loopback by default.
    pub remote_host: String,
    /// Required remote-side destination port.
    pub remote_port: String,
}

impl Default for ForwardForm {
    fn default() -> Self {
        Self {
            selected_field: 0,
            label: String::new(),
            bind_address: "127.0.0.1".to_owned(),
            local_port: String::new(),
            remote_host: "127.0.0.1".to_owned(),
            remote_port: String::new(),
        }
    }
}

impl ForwardForm {
    const FIELD_COUNT: usize = 5;

    /// Parses the current values using application validation.
    pub fn draft(&self) -> Result<ForwardRuleDraft, crate::application::RuleError> {
        ForwardRuleDraft::parse(
            &self.label,
            &self.bind_address,
            &self.local_port,
            &self.remote_host,
            &self.remote_port,
        )
    }

    /// Moves to the next field.
    pub fn next_field(&mut self) {
        self.selected_field = (self.selected_field + 1) % Self::FIELD_COUNT;
    }

    /// Moves to the previous field.
    pub fn previous_field(&mut self) {
        self.selected_field = self
            .selected_field
            .checked_sub(1)
            .unwrap_or(Self::FIELD_COUNT - 1);
    }

    /// Mutable text of the active field.
    pub fn selected_value_mut(&mut self) -> &mut String {
        match self.selected_field {
            0 => &mut self.label,
            1 => &mut self.bind_address,
            2 => &mut self.local_port,
            3 => &mut self.remote_host,
            _ => &mut self.remote_port,
        }
    }
}

/// TUI-only selection, modal, and status state.
#[derive(Debug, Clone)]
pub struct UiState {
    /// Active pane.
    pub focus: Focus,
    /// Selected target index.
    pub selected_target: usize,
    /// Selected rule index for the current target.
    pub selected_forward: usize,
    /// Current interaction mode.
    pub mode: Mode,
    /// One-line operation summary.
    pub status: String,
    /// Most recent detailed error text.
    pub error_detail: Option<String>,
    /// Form contents retained across the public-bind warning.
    pub form: ForwardForm,
    /// Validated public-bind form awaiting confirmation.
    pub pending_draft: Option<ForwardRuleDraft>,
    /// Whether the event loop should stop.
    pub should_quit: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            focus: Focus::Targets,
            selected_target: 0,
            selected_forward: 0,
            mode: Mode::Normal,
            status: "準備完了".to_owned(),
            error_detail: None,
            form: ForwardForm::default(),
            pending_draft: None,
            should_quit: false,
        }
    }
}

impl UiState {
    /// Keeps selections valid after catalog changes.
    pub fn clamp<B: SshClient, P: PortProbe>(&mut self, app: &AppState<B, P>) {
        let target_count = app.sessions().entries().len();
        self.selected_target = clamp_index(self.selected_target, target_count);
        let forward_count = selected_target_id(self, app)
            .map(|target_id| app.rules_for(target_id).len())
            .unwrap_or(0);
        self.selected_forward = clamp_index(self.selected_forward, forward_count);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetRow {
    alias: String,
    state: SessionState,
    active_forwards: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ForwardRow {
    label: String,
    state: ForwardState,
    local: String,
    remote: String,
}

/// Renders the complete current application state.
pub fn render<B: SshClient, P: PortProbe>(
    frame: &mut Frame<'_>,
    ui: &UiState,
    app: &AppState<B, P>,
) {
    let area = frame.area();
    if area.width < 24 || area.height < 6 {
        frame.render_widget(
            Paragraph::new("portdeck: terminal too small\nq: quit")
                .block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);
    let body = rows[0];
    if area.width >= 70 {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
            .split(body);
        render_targets(frame, panes[0], ui, app);
        render_forwards(frame, panes[1], ui, app);
    } else {
        let panes = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(4), Constraint::Min(3)])
            .split(body);
        render_targets(frame, panes[0], ui, app);
        render_forwards(frame, panes[1], ui, app);
    }
    render_footer(frame, rows[1], ui);
    render_modal(frame, area, ui);
}

fn render_targets<B: SshClient, P: PortProbe>(
    frame: &mut Frame<'_>,
    area: Rect,
    ui: &UiState,
    app: &AppState<B, P>,
) {
    let target_rows = app
        .sessions()
        .entries()
        .iter()
        .map(|entry| TargetRow {
            alias: entry.target.host_alias.clone(),
            state: entry.session.state,
            active_forwards: entry
                .forwards
                .values()
                .filter(|forward| forward.state == ForwardState::Active)
                .count(),
        })
        .collect::<Vec<_>>();
    let items = target_rows
        .iter()
        .map(|row| {
            ListItem::new(format!(
                "{} {}  {}  [{}]",
                session_symbol(row.state),
                row.alias,
                session_label(row.state),
                row.active_forwards
            ))
        })
        .collect::<Vec<_>>();
    let border_style = focus_border(ui.focus == Focus::Targets);
    let list = List::new(items)
        .block(
            Block::default()
                .title(" Targets / Sessions ")
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .highlight_symbol("> ")
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(
        (!target_rows.is_empty()).then_some(clamp_index(ui.selected_target, target_rows.len())),
    );
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_forwards<B: SshClient, P: PortProbe>(
    frame: &mut Frame<'_>,
    area: Rect,
    ui: &UiState,
    app: &AppState<B, P>,
) {
    let rows = forward_rows(ui, app);
    let items = rows
        .iter()
        .map(|row| {
            ListItem::new(format!(
                "{} {}  {} → {}  {}",
                forward_symbol(row.state),
                row.label,
                row.local,
                row.remote,
                forward_label(row.state)
            ))
        })
        .collect::<Vec<_>>();
    let border_style = focus_border(ui.focus == Focus::Forwards);
    let list = List::new(items)
        .block(
            Block::default()
                .title(" Forwards ")
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .highlight_symbol("> ")
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default()
        .with_selected((!rows.is_empty()).then_some(clamp_index(ui.selected_forward, rows.len())));
    frame.render_stateful_widget(list, area, &mut state);
}

fn forward_rows<B: SshClient, P: PortProbe>(ui: &UiState, app: &AppState<B, P>) -> Vec<ForwardRow> {
    let Some(target_id) = selected_target_id(ui, app) else {
        return Vec::new();
    };
    let entry = app.sessions().entry(target_id);
    app.rules_for(target_id)
        .iter()
        .map(|rule| {
            let runtime = entry.and_then(|entry| entry.forwards.get(&rule.id));
            let state = runtime
                .map(|forward| forward.state)
                .unwrap_or(ForwardState::Inactive);
            let local_port = runtime
                .and_then(|forward| forward.actual_local_port)
                .or(rule.requested_local_port)
                .unwrap_or(rule.remote_port);
            ForwardRow {
                label: rule
                    .label
                    .clone()
                    .unwrap_or_else(|| rule.id.as_str().to_owned()),
                state,
                local: display_endpoint(&rule.bind_address, local_port),
                remote: display_endpoint(&rule.remote_host, rule.remote_port),
            }
        })
        .collect()
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, ui: &UiState) {
    let help = match ui.mode {
        Mode::Normal => {
            "Tab: pane  ↑↓: select  c: connect  d: disconnect  a: add  Space: activate/cancel  D: delete  r: check  e: error  q: quit"
        }
        Mode::ForwardForm => "Tab/↑↓: field  Enter: save  Esc: cancel",
        Mode::PublicBindWarning | Mode::Confirm(_) => "y/Enter: confirm  n/Esc: cancel",
        Mode::ErrorDetails => "Esc/e/Enter: close",
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![Span::styled(
                &ui.status,
                Style::default().fg(Color::Yellow),
            )]),
            Line::from(help),
        ]),
        area,
    );
}

fn render_modal(frame: &mut Frame<'_>, area: Rect, ui: &UiState) {
    match &ui.mode {
        Mode::Normal => {}
        Mode::ForwardForm => render_forward_form(frame, area, ui),
        Mode::PublicBindWarning => render_message_modal(
            frame,
            area,
            " Public bind warning ",
            "このbind addressはローカル側の他ホストへ公開される可能性があります。\n\n続行しますか？ [y/N]",
            Color::Red,
        ),
        Mode::Confirm(action) => {
            let message = match action {
                Confirmation::Disconnect(_) => {
                    "選択したSSHセッションと配下の転送を終了します。\n\n続行しますか？ [y/N]"
                }
                Confirmation::DeleteForward(_) => {
                    "選択した保存済み転送ルールを削除します。\nActiveの場合は先に取消します。\n\n続行しますか？ [y/N]"
                }
                Confirmation::Quit => {
                    "すべてのportdeck所有SSHセッションを終了してquitします。\n\n続行しますか？ [y/N]"
                }
            };
            render_message_modal(frame, area, " Confirm ", message, Color::Yellow);
        }
        Mode::ErrorDetails => render_message_modal(
            frame,
            area,
            " Error details ",
            ui.error_detail.as_deref().unwrap_or("詳細はありません"),
            Color::Red,
        ),
    }
}

fn render_forward_form(frame: &mut Frame<'_>, area: Rect, ui: &UiState) {
    let modal = centered_rect(72, 16, area);
    frame.render_widget(Clear, modal);
    let fields = [
        ("Label (optional)", ui.form.label.as_str()),
        ("Local bind address", ui.form.bind_address.as_str()),
        ("Preferred local port", ui.form.local_port.as_str()),
        ("Remote destination host", ui.form.remote_host.as_str()),
        ("Remote destination port *", ui.form.remote_port.as_str()),
    ];
    let lines = fields
        .iter()
        .enumerate()
        .map(|(index, (label, value))| {
            let marker = if index == ui.form.selected_field {
                ">"
            } else {
                " "
            };
            Line::from(format!("{marker} {label}: {value}"))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Add saved forward ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: false }),
        modal,
    );
}

fn render_message_modal(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    message: &str,
    color: Color,
) {
    let modal = centered_rect(70, 11, area);
    frame.render_widget(Clear, modal);
    frame.render_widget(
        Paragraph::new(message)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(color)),
            ),
        modal,
    );
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical_margin = area.height.saturating_sub(height) / 2;
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(vertical_margin),
            Constraint::Min(height.min(area.height)),
            Constraint::Length(vertical_margin),
        ])
        .split(area);
    let horizontal_margin = area.width.saturating_mul(100 - percent_x) / 200;
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(horizontal_margin),
            Constraint::Min(1),
            Constraint::Length(horizontal_margin),
        ])
        .split(vertical[1])[1]
}

fn selected_target_id<'a, B: SshClient, P: PortProbe>(
    ui: &UiState,
    app: &'a AppState<B, P>,
) -> Option<&'a TargetId> {
    app.sessions()
        .entries()
        .get(ui.selected_target)
        .map(|entry| &entry.target.id)
}

fn session_symbol(state: SessionState) -> &'static str {
    match state {
        SessionState::Disconnected => "○",
        SessionState::Connecting => "◐",
        SessionState::Connected => "●",
        SessionState::Stopping => "◑",
        SessionState::Failed => "!",
    }
}

fn session_label(state: SessionState) -> &'static str {
    match state {
        SessionState::Disconnected => "Disconnected",
        SessionState::Connecting => "Connecting",
        SessionState::Connected => "Connected",
        SessionState::Stopping => "Stopping",
        SessionState::Failed => "Failed",
    }
}

fn forward_symbol(state: ForwardState) -> &'static str {
    match state {
        ForwardState::Inactive => "○",
        ForwardState::Adding => "+",
        ForwardState::Active => "●",
        ForwardState::Removing => "−",
        ForwardState::Failed => "!",
        ForwardState::Unavailable => "×",
    }
}

fn forward_label(state: ForwardState) -> &'static str {
    match state {
        ForwardState::Inactive => "Inactive",
        ForwardState::Adding => "Adding",
        ForwardState::Active => "Active",
        ForwardState::Removing => "Removing",
        ForwardState::Failed => "Failed",
        ForwardState::Unavailable => "Unavailable",
    }
}

fn display_endpoint(host: &str, port: u16) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn focus_border(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    }
}

fn clamp_index(index: usize, count: usize) -> usize {
    if count == 0 { 0 } else { index.min(count - 1) }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::application::{AppState, ForwardRuleDraft, PortProbe, SessionManager};
    use crate::domain::{Target, TargetId};
    use crate::runtime::RuntimeDirectory;
    use crate::ssh::{LocalForwardSpec, SshClient, SshError, SshOutput};

    use super::{Focus, Mode, UiState, render};

    #[derive(Debug, Default)]
    struct FakeSsh {
        output: RefCell<VecDeque<SshOutput>>,
    }

    impl SshClient for FakeSsh {
        fn version(&self) -> Result<SshOutput, SshError> {
            Ok(success())
        }

        fn resolve_config(&self, _host_alias: &str) -> Result<SshOutput, SshError> {
            Ok(success())
        }

        fn connect(&self, _host_alias: &str, _control_path: &Path) -> Result<SshOutput, SshError> {
            Ok(self.output.borrow_mut().pop_front().unwrap_or_else(success))
        }

        fn check(&self, _host_alias: &str, _control_path: &Path) -> Result<SshOutput, SshError> {
            Ok(self.output.borrow_mut().pop_front().unwrap_or_else(success))
        }

        fn add_local_forward(
            &self,
            _host_alias: &str,
            _control_path: &Path,
            _forward: &LocalForwardSpec,
        ) -> Result<SshOutput, SshError> {
            Ok(success())
        }

        fn cancel_local_forward(
            &self,
            _host_alias: &str,
            _control_path: &Path,
            _forward: &LocalForwardSpec,
        ) -> Result<SshOutput, SshError> {
            Ok(success())
        }

        fn disconnect(
            &self,
            _host_alias: &str,
            _control_path: &Path,
        ) -> Result<SshOutput, SshError> {
            Ok(success())
        }
    }

    #[derive(Debug, Default)]
    struct Available;

    impl PortProbe for Available {
        fn is_available(&self, _bind_address: &str, _port: u16) -> io::Result<bool> {
            Ok(true)
        }
    }

    struct Fixture {
        app: AppState<FakeSsh, Available>,
        runtime_path: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let runtime_path = std::env::temp_dir()
                .join(format!("portdeck-tui-test-{}-{unique}", std::process::id()));
            let runtime = RuntimeDirectory::prepare(&runtime_path).unwrap();
            let target = Target {
                id: TargetId::new("dev"),
                host_alias: "dev-server".to_owned(),
                source: PathBuf::from("config"),
            };
            let manager = SessionManager::with_port_probe(
                FakeSsh::default(),
                runtime,
                vec![target],
                Available,
            )
            .unwrap();
            let mut app = AppState::new(manager, Vec::new()).unwrap();
            app.add_rule(
                &TargetId::new("dev"),
                ForwardRuleDraft::parse("web", "", "8080", "", "3000").unwrap(),
            )
            .unwrap();
            Self { app, runtime_path }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.runtime_path).unwrap();
        }
    }

    fn success() -> SshOutput {
        SshOutput {
            success: true,
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    #[test]
    fn renders_targets_and_local_remote_ports_in_wide_terminal() {
        let fixture = Fixture::new();
        let ui = UiState {
            focus: Focus::Forwards,
            ..UiState::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 18)).unwrap();

        terminal
            .draw(|frame| render(frame, &ui, &fixture.app))
            .unwrap();

        let contents = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(contents.contains("dev-server"));
        assert!(contents.contains("Disconnected"));
        assert!(contents.contains("127.0.0.1:8080"));
        assert!(contents.contains("127.0.0.1:3000"));
        assert!(contents.contains("Inactive"));
    }

    #[test]
    fn small_terminal_render_never_panics_and_keeps_quit_help() {
        let fixture = Fixture::new();
        let mut terminal = Terminal::new(TestBackend::new(20, 4)).unwrap();

        terminal
            .draw(|frame| render(frame, &UiState::default(), &fixture.app))
            .unwrap();

        terminal.backend().assert_buffer_lines([
            "┌──────────────────┐",
            "│portdeck: terminal│",
            "│q: quit           │",
            "└──────────────────┘",
        ]);
    }

    #[test]
    fn form_navigation_wraps_and_parses_defaults() {
        let mut ui = UiState {
            mode: Mode::ForwardForm,
            ..UiState::default()
        };
        ui.form.previous_field();
        assert_eq!(ui.form.selected_field, 4);
        ui.form.remote_port = "3000".to_owned();
        assert_eq!(ui.form.draft().unwrap().remote_port, 3000);
    }
}
