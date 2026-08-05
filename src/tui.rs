//! Terminal input and rendering.
//!
//! The TUI reports user intent and renders application state; it does not
//! construct or execute OpenSSH commands.

use std::io::{self, Stdout, stdout};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::flag;
use thiserror::Error;

use crate::application::{AppActionError, AppState, ForwardRuleDraft, ManagerError, PortProbe};
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum UiCommand {
    None,
    Connect(TargetId),
    Disconnect(TargetId),
    Check(TargetId),
    AddForward(TargetId, ForwardRuleDraft),
    ActivateForward(ForwardRuleId),
    CancelForward(ForwardRuleId),
    DeleteForward(ForwardRuleId),
    Quit,
}

/// Terminal setup, event, or restoration failure.
#[derive(Debug, Error)]
pub enum TuiError {
    /// Crossterm or terminal backend I/O failed.
    #[error("terminal operation failed: {0}")]
    Io(#[from] io::Error),
}

/// Initial status and diagnostic information from startup recovery.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupNotice {
    /// One-line startup summary.
    pub status: String,
    /// Details opened with `e`.
    pub error_detail: Option<String>,
}

type PortdeckTerminal = Terminal<CrosstermBackend<Stdout>>;

struct TerminalGuard {
    terminal: PortdeckTerminal,
    active: bool,
}

impl TerminalGuard {
    fn enter() -> Result<Self, TuiError> {
        enable_raw_mode()?;
        let mut output = stdout();
        if let Err(error) = execute!(output, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        let backend = CrosstermBackend::new(output);
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => {
                let mut output = stdout();
                let _ = execute!(output, LeaveAlternateScreen);
                let _ = disable_raw_mode();
                return Err(error.into());
            }
        };
        terminal.hide_cursor()?;
        terminal.clear()?;
        Ok(Self {
            terminal,
            active: true,
        })
    }

    fn suspend(&mut self) -> Result<(), TuiError> {
        if !self.active {
            return Ok(());
        }
        self.terminal.show_cursor()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        disable_raw_mode()?;
        self.active = false;
        Ok(())
    }

    fn resume(&mut self) -> Result<(), TuiError> {
        if self.active {
            return Ok(());
        }
        enable_raw_mode()?;
        if let Err(error) = execute!(self.terminal.backend_mut(), EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        self.terminal.hide_cursor()?;
        self.terminal.clear()?;
        self.active = true;
        Ok(())
    }

    fn restore(&mut self) -> Result<(), TuiError> {
        if !self.active {
            return Ok(());
        }
        self.terminal.show_cursor()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        disable_raw_mode()?;
        self.active = false;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = self.terminal.show_cursor();
            let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
            let _ = disable_raw_mode();
            self.active = false;
        }
    }
}

/// Runs the blocking MVP event loop until the user confirms quit.
pub fn run<B: SshClient, P: PortProbe>(
    app: &mut AppState<B, P>,
    startup_notice: Option<StartupNotice>,
) -> Result<(), TuiError> {
    let mut terminal = TerminalGuard::enter()?;
    let mut ui = UiState::default();
    if let Some(notice) = startup_notice {
        ui.status = notice.status;
        ui.error_detail = notice.error_detail;
    }
    ui.clamp(app);
    let termination_requested = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM, SIGHUP] {
        flag::register(signal, Arc::clone(&termination_requested))?;
    }

    while !ui.should_quit {
        terminal.terminal.draw(|frame| render(frame, &ui, app))?;
        if termination_requested.load(Ordering::Relaxed) {
            ui.should_quit = true;
            continue;
        }
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let event = event::read()?;
        let Event::Key(key) = event else {
            continue;
        };
        if matches!(key.kind, KeyEventKind::Release) {
            continue;
        }
        let command = handle_key(&mut ui, app, key);
        execute_ui_command(command, &mut ui, app, &mut terminal)?;
        ui.clamp(app);
    }

    terminal.restore()
}

fn handle_key<B: SshClient, P: PortProbe>(
    ui: &mut UiState,
    app: &AppState<B, P>,
    key: KeyEvent,
) -> UiCommand {
    match ui.mode.clone() {
        Mode::Normal => handle_normal_key(ui, app, key),
        Mode::ForwardForm => handle_form_key(ui, app, key),
        Mode::PublicBindWarning => handle_public_warning_key(ui, app, key),
        Mode::Confirm(confirmation) => handle_confirmation_key(ui, key, confirmation),
        Mode::ErrorDetails => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('e')) {
                ui.mode = Mode::Normal;
            }
            UiCommand::None
        }
    }
}

fn handle_normal_key<B: SshClient, P: PortProbe>(
    ui: &mut UiState,
    app: &AppState<B, P>,
    key: KeyEvent,
) -> UiCommand {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return request_quit(ui, app);
    }

    match key.code {
        KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
            ui.focus = match ui.focus {
                Focus::Targets => Focus::Forwards,
                Focus::Forwards => Focus::Targets,
            };
            UiCommand::None
        }
        KeyCode::Up | KeyCode::Char('k') => {
            move_selection(ui, app, false);
            UiCommand::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            move_selection(ui, app, true);
            UiCommand::None
        }
        KeyCode::Char('c') => selected_target_id(ui, app)
            .cloned()
            .map(UiCommand::Connect)
            .unwrap_or(UiCommand::None),
        KeyCode::Char('d') => {
            if let Some(target_id) = selected_target_id(ui, app).cloned() {
                ui.mode = Mode::Confirm(Confirmation::Disconnect(target_id));
            }
            UiCommand::None
        }
        KeyCode::Char('a') => {
            if selected_target_id(ui, app).is_some() {
                ui.form = ForwardForm::default();
                ui.mode = Mode::ForwardForm;
            }
            UiCommand::None
        }
        KeyCode::Char(' ') => selected_forward_command(ui, app),
        KeyCode::Char('D') => {
            if let Some(rule_id) = selected_rule_id(ui, app).cloned() {
                ui.mode = Mode::Confirm(Confirmation::DeleteForward(rule_id));
            }
            UiCommand::None
        }
        KeyCode::Char('r') => selected_target_id(ui, app)
            .cloned()
            .map(UiCommand::Check)
            .unwrap_or(UiCommand::None),
        KeyCode::Char('e') => {
            if ui.error_detail.is_some() {
                ui.mode = Mode::ErrorDetails;
            }
            UiCommand::None
        }
        KeyCode::Char('q') => request_quit(ui, app),
        _ => UiCommand::None,
    }
}

fn handle_form_key<B: SshClient, P: PortProbe>(
    ui: &mut UiState,
    app: &AppState<B, P>,
    key: KeyEvent,
) -> UiCommand {
    match key.code {
        KeyCode::Esc => {
            ui.mode = Mode::Normal;
            UiCommand::None
        }
        KeyCode::Tab | KeyCode::Down => {
            ui.form.next_field();
            UiCommand::None
        }
        KeyCode::BackTab | KeyCode::Up => {
            ui.form.previous_field();
            UiCommand::None
        }
        KeyCode::Backspace => {
            ui.form.selected_value_mut().pop();
            UiCommand::None
        }
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            ui.form.selected_value_mut().push(character);
            UiCommand::None
        }
        KeyCode::Enter => {
            let Some(target_id) = selected_target_id(ui, app).cloned() else {
                ui.mode = Mode::Normal;
                return UiCommand::None;
            };
            match ui.form.draft() {
                Ok(draft) if draft.public_bind => {
                    ui.pending_draft = Some(draft);
                    ui.mode = Mode::PublicBindWarning;
                    UiCommand::None
                }
                Ok(draft) => {
                    ui.form = ForwardForm::default();
                    ui.mode = Mode::Normal;
                    UiCommand::AddForward(target_id, draft)
                }
                Err(error) => {
                    set_error(ui, "転送ルールを保存できません", Some(error.to_string()));
                    UiCommand::None
                }
            }
        }
        _ => UiCommand::None,
    }
}

fn handle_public_warning_key<B: SshClient, P: PortProbe>(
    ui: &mut UiState,
    app: &AppState<B, P>,
    key: KeyEvent,
) -> UiCommand {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            let Some(target_id) = selected_target_id(ui, app).cloned() else {
                ui.mode = Mode::Normal;
                ui.pending_draft = None;
                return UiCommand::None;
            };
            let Some(draft) = ui.pending_draft.take() else {
                ui.mode = Mode::Normal;
                return UiCommand::None;
            };
            ui.form = ForwardForm::default();
            ui.mode = Mode::Normal;
            UiCommand::AddForward(target_id, draft)
        }
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
            ui.pending_draft = None;
            ui.mode = Mode::ForwardForm;
            UiCommand::None
        }
        _ => UiCommand::None,
    }
}

fn handle_confirmation_key(
    ui: &mut UiState,
    key: KeyEvent,
    confirmation: Confirmation,
) -> UiCommand {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            ui.mode = Mode::Normal;
            match confirmation {
                Confirmation::Disconnect(target_id) => UiCommand::Disconnect(target_id),
                Confirmation::DeleteForward(rule_id) => UiCommand::DeleteForward(rule_id),
                Confirmation::Quit => UiCommand::Quit,
            }
        }
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
            ui.mode = Mode::Normal;
            UiCommand::None
        }
        _ => UiCommand::None,
    }
}

fn execute_ui_command<B: SshClient, P: PortProbe>(
    command: UiCommand,
    ui: &mut UiState,
    app: &mut AppState<B, P>,
    terminal: &mut TerminalGuard,
) -> Result<(), TuiError> {
    match command {
        UiCommand::None => {}
        UiCommand::Connect(target_id) => {
            ui.status = "OpenSSHへ端末を引き渡します…".to_owned();
            terminal.suspend()?;
            let result = app.sessions_mut().connect(&target_id);
            terminal.resume()?;
            match result {
                Ok(()) => set_success(ui, "SSH接続を開始しました"),
                Err(error) => set_manager_error(ui, &error),
            }
        }
        UiCommand::Disconnect(target_id) => match app.sessions_mut().disconnect(&target_id) {
            Ok(()) => set_success(ui, "SSHセッションを終了しました"),
            Err(error) => set_manager_error(ui, &error),
        },
        UiCommand::Check(target_id) => match app.sessions_mut().check(&target_id) {
            Ok(true) => set_success(ui, "ControlMasterは接続中です"),
            Ok(false) => {
                ui.status = "ControlMasterは接続されていません".to_owned();
                ui.error_detail = selected_session_error(ui, app);
            }
            Err(error) => set_manager_error(ui, &error),
        },
        UiCommand::AddForward(target_id, draft) => match app.add_rule(&target_id, draft) {
            Ok(_) => set_success(ui, "転送ルールを保存しました（未有効）"),
            Err(error) => set_error(ui, "転送ルールを保存できません", Some(error.to_string())),
        },
        UiCommand::ActivateForward(rule_id) => match app.activate_rule(&rule_id) {
            Ok(port) => set_success(ui, &format!("ローカル側port {port} で転送を開始しました")),
            Err(error) => set_action_error(ui, &error),
        },
        UiCommand::CancelForward(rule_id) => match app.cancel_rule(&rule_id) {
            Ok(()) => set_success(ui, "転送を取消しました（定義は保存済み）"),
            Err(error) => set_action_error(ui, &error),
        },
        UiCommand::DeleteForward(rule_id) => {
            let needs_cancel =
                runtime_forward(ui, app).is_some_and(|runtime| runtime.actual_local_port.is_some());
            if needs_cancel && let Err(error) = app.cancel_rule(&rule_id) {
                set_action_error(ui, &error);
                return Ok(());
            }
            match app.remove_rule(&rule_id) {
                Ok(()) => set_success(ui, "保存済み転送ルールを削除しました"),
                Err(error) => set_action_error(ui, &error),
            }
        }
        UiCommand::Quit => ui.should_quit = true,
    }
    Ok(())
}

fn move_selection<B: SshClient, P: PortProbe>(
    ui: &mut UiState,
    app: &AppState<B, P>,
    forward: bool,
) {
    match ui.focus {
        Focus::Targets => {
            let count = app.sessions().entries().len();
            ui.selected_target = wrapped_index(ui.selected_target, count, forward);
            ui.selected_forward = 0;
        }
        Focus::Forwards => {
            let count = selected_target_id(ui, app)
                .map(|target_id| app.rules_for(target_id).len())
                .unwrap_or(0);
            ui.selected_forward = wrapped_index(ui.selected_forward, count, forward);
        }
    }
}

fn selected_forward_command<B: SshClient, P: PortProbe>(
    ui: &UiState,
    app: &AppState<B, P>,
) -> UiCommand {
    let Some(rule_id) = selected_rule_id(ui, app).cloned() else {
        return UiCommand::None;
    };
    if runtime_forward(ui, app).is_some_and(|runtime| runtime.actual_local_port.is_some()) {
        UiCommand::CancelForward(rule_id)
    } else {
        UiCommand::ActivateForward(rule_id)
    }
}

fn request_quit<B: SshClient, P: PortProbe>(ui: &mut UiState, app: &AppState<B, P>) -> UiCommand {
    let has_live_session = app
        .sessions()
        .entries()
        .iter()
        .any(|entry| entry.session.state != SessionState::Disconnected);
    if has_live_session {
        ui.mode = Mode::Confirm(Confirmation::Quit);
        UiCommand::None
    } else {
        UiCommand::Quit
    }
}

fn selected_rule_id<'a, B: SshClient, P: PortProbe>(
    ui: &UiState,
    app: &'a AppState<B, P>,
) -> Option<&'a ForwardRuleId> {
    let target_id = selected_target_id(ui, app)?;
    app.rules_for(target_id)
        .get(ui.selected_forward)
        .map(|rule| &rule.id)
}

fn runtime_forward<'a, B: SshClient, P: PortProbe>(
    ui: &UiState,
    app: &'a AppState<B, P>,
) -> Option<&'a crate::domain::ActiveForward> {
    let target_id = selected_target_id(ui, app)?;
    let rule_id = selected_rule_id(ui, app)?;
    app.sessions().entry(target_id)?.forwards.get(rule_id)
}

fn selected_session_error<B: SshClient, P: PortProbe>(
    ui: &UiState,
    app: &AppState<B, P>,
) -> Option<String> {
    let target_id = selected_target_id(ui, app)?;
    app.sessions()
        .entry(target_id)?
        .session
        .last_error
        .as_ref()
        .and_then(|failure| failure.detail.clone())
}

fn set_success(ui: &mut UiState, message: &str) {
    ui.status = message.to_owned();
    ui.error_detail = None;
}

fn set_manager_error(ui: &mut UiState, error: &ManagerError) {
    match error {
        ManagerError::Operation(failure) => {
            set_error(ui, &failure.summary, failure.detail.clone());
        }
        _ => set_error(ui, &error.to_string(), Some(error.to_string())),
    }
}

fn set_action_error(ui: &mut UiState, error: &AppActionError) {
    if let AppActionError::Manager(error) = error {
        set_manager_error(ui, error);
    } else {
        set_error(ui, &error.to_string(), Some(error.to_string()));
    }
}

fn set_error(ui: &mut UiState, summary: &str, detail: Option<String>) {
    ui.status = format!("ERROR: {summary}");
    ui.error_detail = detail;
}

fn wrapped_index(index: usize, count: usize, forward: bool) -> usize {
    if count == 0 {
        0
    } else if forward {
        (index + 1) % count
    } else {
        index.checked_sub(1).unwrap_or(count - 1)
    }
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
    destination: Option<String>,
    state: SessionState,
    active_forwards: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ForwardRow {
    label: String,
    state: ForwardState,
    local: String,
    remote: String,
    local_port: u16,
    remote_port: u16,
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
            destination: entry.effective_config.as_ref().map(|config| {
                let proxy = config
                    .proxy_jump
                    .as_ref()
                    .map(|jump| format!(" via {jump}"))
                    .unwrap_or_default();
                format!("{}@{}:{}{proxy}", config.user, config.hostname, config.port)
            }),
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
            let destination = row
                .destination
                .as_ref()
                .map(|value| format!("  {value}"))
                .unwrap_or_default();
            ListItem::new(format!(
                "{} {}  {}  [{}]{destination}",
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
            if area.width < 60 {
                ListItem::new(format!(
                    "{} {}→{} {} {}",
                    forward_symbol(row.state),
                    row.local_port,
                    row.remote_port,
                    row.label,
                    forward_label(row.state)
                ))
            } else {
                ListItem::new(format!(
                    "{} {}  {} → {}  {}",
                    forward_symbol(row.state),
                    row.label,
                    row.local,
                    row.remote,
                    forward_label(row.state)
                ))
            }
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
                local_port,
                remote_port: rule.remote_port,
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

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::application::{AppState, ForwardRuleDraft, PortProbe, SessionManager};
    use crate::domain::{Target, TargetId};
    use crate::runtime::RuntimeDirectory;
    use crate::ssh::{LocalForwardSpec, SshClient, SshError, SshOutput};

    use super::{Confirmation, Focus, Mode, UiCommand, UiState, handle_key, render};

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
    fn narrow_terminal_keeps_both_forward_port_numbers_visible() {
        let fixture = Fixture::new();
        let mut terminal = Terminal::new(TestBackend::new(50, 10)).unwrap();

        terminal
            .draw(|frame| render(frame, &UiState::default(), &fixture.app))
            .unwrap();

        let contents = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(contents.contains("8080→3000"));
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

    #[test]
    fn add_form_event_returns_validated_application_intent() {
        let fixture = Fixture::new();
        let mut ui = UiState::default();

        assert_eq!(
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)
            ),
            UiCommand::None
        );
        assert_eq!(ui.mode, Mode::ForwardForm);
        ui.form.remote_port = "5432".to_owned();

        let command = handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );

        assert!(matches!(command, UiCommand::AddForward(_, _)));
        assert_eq!(ui.mode, Mode::Normal);
    }

    #[test]
    fn public_bind_requires_an_extra_confirmation_event() {
        let fixture = Fixture::new();
        let mut ui = UiState {
            mode: Mode::ForwardForm,
            ..UiState::default()
        };
        ui.form.bind_address = "0.0.0.0".to_owned();
        ui.form.remote_port = "3000".to_owned();

        let first = handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(first, UiCommand::None);
        assert_eq!(ui.mode, Mode::PublicBindWarning);

        let confirmed = handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        );
        assert!(matches!(confirmed, UiCommand::AddForward(_, _)));
    }

    #[test]
    fn destructive_events_enter_confirmation_before_returning_intent() {
        let fixture = Fixture::new();
        let mut ui = UiState::default();

        let first = handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT),
        );
        assert_eq!(first, UiCommand::None);
        assert!(matches!(
            ui.mode,
            Mode::Confirm(Confirmation::DeleteForward(_))
        ));

        let confirmed = handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(matches!(confirmed, UiCommand::DeleteForward(_)));
    }

    #[test]
    fn error_detail_mode_opens_only_when_detail_exists() {
        let fixture = Fixture::new();
        let mut ui = UiState::default();
        let key = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE);

        handle_key(&mut ui, &fixture.app, key);
        assert_eq!(ui.mode, Mode::Normal);

        ui.error_detail = Some("OpenSSH stderr".to_owned());
        handle_key(&mut ui, &fixture.app, key);
        assert_eq!(ui.mode, Mode::ErrorDetails);
    }
}
