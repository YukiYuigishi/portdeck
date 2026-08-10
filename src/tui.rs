//! Terminal input and rendering.
//!
//! The TUI reports user intent and renders application state; it does not
//! construct or execute OpenSSH commands.

use std::io::{self, Stdout, stdout};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    Clear as TerminalClear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
    disable_raw_mode, enable_raw_mode,
};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::flag;
use thiserror::Error;

use crate::application::{AppActionError, AppState, ForwardRuleDraft, ManagerError, PortProbe};
use crate::domain::{
    ForwardKind, ForwardRule, ForwardRuleId, ForwardState, SessionState, TargetId,
};
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
    /// Editing the case-insensitive target alias filter.
    TargetSearch,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardFormKind {
    /// Fixed remote destination (`ssh -L`).
    Local,
    /// OpenSSH SOCKS listener (`ssh -D`).
    Socks,
}

impl ForwardFormKind {
    fn label(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Socks => "SOCKS",
        }
    }
}

/// Editable forward-rule form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardForm {
    /// Zero-based active field.
    pub selected_field: usize,
    /// Explicit OpenSSH forwarding capability.
    pub kind: ForwardFormKind,
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
            kind: ForwardFormKind::Local,
            label: String::new(),
            bind_address: "127.0.0.1".to_owned(),
            local_port: String::new(),
            remote_host: "127.0.0.1".to_owned(),
            remote_port: String::new(),
        }
    }
}

impl ForwardForm {
    fn field_count(&self) -> usize {
        match self.kind {
            ForwardFormKind::Local => 6,
            ForwardFormKind::Socks => 4,
        }
    }

    /// Prefills the shared add/edit form from one saved definition.
    pub fn from_rule(rule: &ForwardRule) -> Self {
        let (kind, remote_host, remote_port) = match &rule.kind {
            ForwardKind::Local {
                remote_host,
                remote_port,
            } => (
                ForwardFormKind::Local,
                remote_host.clone(),
                remote_port.to_string(),
            ),
            ForwardKind::Socks => (ForwardFormKind::Socks, String::new(), String::new()),
        };
        Self {
            selected_field: 0,
            kind,
            label: rule.label.clone().unwrap_or_default(),
            bind_address: rule.bind_address.clone(),
            local_port: rule
                .requested_local_port
                .map(|port| port.to_string())
                .unwrap_or_else(|| match kind {
                    ForwardFormKind::Local => String::new(),
                    ForwardFormKind::Socks => "1080".to_owned(),
                }),
            remote_host,
            remote_port,
        }
    }

    /// Parses the current values using application validation.
    pub fn draft(&self) -> Result<ForwardRuleDraft, crate::application::RuleError> {
        match self.kind {
            ForwardFormKind::Local => ForwardRuleDraft::parse(
                &self.label,
                &self.bind_address,
                &self.local_port,
                &self.remote_host,
                &self.remote_port,
            ),
            ForwardFormKind::Socks => {
                ForwardRuleDraft::parse_socks(&self.label, &self.bind_address, &self.local_port)
            }
        }
    }

    /// Moves to the next field.
    pub fn next_field(&mut self) {
        self.selected_field = (self.selected_field + 1) % self.field_count();
    }

    /// Moves to the previous field.
    pub fn previous_field(&mut self) {
        self.selected_field = self
            .selected_field
            .checked_sub(1)
            .unwrap_or(self.field_count() - 1);
    }

    /// Mutable text of the active field.
    pub fn selected_value_mut(&mut self) -> Option<&mut String> {
        match self.selected_field {
            0 => None,
            1 => Some(&mut self.label),
            2 => Some(&mut self.bind_address),
            3 => Some(&mut self.local_port),
            4 => Some(&mut self.remote_host),
            _ => Some(&mut self.remote_port),
        }
    }

    fn set_kind(&mut self, kind: ForwardFormKind) {
        self.kind = kind;
        if kind == ForwardFormKind::Socks && self.local_port.is_empty() {
            self.local_port = "1080".to_owned();
        }
        self.selected_field = self.selected_field.min(self.field_count() - 1);
    }

    fn toggle_kind(&mut self) {
        let kind = match self.kind {
            ForwardFormKind::Local => ForwardFormKind::Socks,
            ForwardFormKind::Socks => ForwardFormKind::Local,
        };
        self.set_kind(kind);
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
    /// Case-insensitive substring filter for target aliases.
    pub target_filter: String,
    /// Confirmed filter restored when search editing is cancelled.
    pub search_original_filter: String,
    /// Target selected when the current search edit began.
    pub search_anchor: Option<TargetId>,
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
    /// Rule being edited in the shared form, or `None` while adding.
    pub editing_rule: Option<ForwardRuleId>,
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
    UpdateForward(ForwardRuleId, ForwardRuleDraft),
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
    /// Details opened with `E`.
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
        execute!(
            terminal.backend_mut(),
            TerminalClear(ClearType::All),
            MoveTo(0, 0)
        )?;
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
        execute!(
            self.terminal.backend_mut(),
            TerminalClear(ClearType::All),
            MoveTo(0, 0)
        )?;
        invalidate_previous_frame(&mut self.terminal);
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

fn invalidate_previous_frame<B: Backend>(terminal: &mut Terminal<B>) {
    // Leaving and re-entering the alternate screen discards its visible contents,
    // while ratatui still remembers the last frame. Reset that comparison frame
    // so the next draw writes every cell instead of only the state differences.
    terminal.swap_buffers();
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
        Mode::TargetSearch => handle_search_key(ui, app, key),
        Mode::ForwardForm => handle_form_key(ui, app, key),
        Mode::PublicBindWarning => handle_public_warning_key(ui, app, key),
        Mode::Confirm(confirmation) => handle_confirmation_key(ui, key, confirmation),
        Mode::ErrorDetails => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('E')) {
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
        KeyCode::Tab => {
            ui.focus = match ui.focus {
                Focus::Targets => Focus::Forwards,
                Focus::Forwards => Focus::Targets,
            };
            UiCommand::None
        }
        KeyCode::Left | KeyCode::Char('h') => {
            ui.focus = Focus::Targets;
            UiCommand::None
        }
        KeyCode::Right | KeyCode::Char('l') => {
            ui.focus = Focus::Forwards;
            UiCommand::None
        }
        KeyCode::Char('/') => {
            ui.search_original_filter.clone_from(&ui.target_filter);
            ui.search_anchor = selected_target_id(ui, app).cloned();
            ui.focus = Focus::Targets;
            ui.mode = Mode::TargetSearch;
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
                ui.editing_rule = None;
                ui.pending_draft = None;
                ui.mode = Mode::ForwardForm;
            }
            UiCommand::None
        }
        KeyCode::Char('e') => begin_rule_edit(ui, app),
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
        KeyCode::Char('E') => {
            if ui.error_detail.is_some() {
                ui.mode = Mode::ErrorDetails;
            }
            UiCommand::None
        }
        KeyCode::Char('q') => request_quit(ui, app),
        _ => UiCommand::None,
    }
}

fn handle_search_key<B: SshClient, P: PortProbe>(
    ui: &mut UiState,
    app: &AppState<B, P>,
    key: KeyEvent,
) -> UiCommand {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return request_quit(ui, app);
    }

    match key.code {
        KeyCode::Esc => {
            ui.target_filter.clone_from(&ui.search_original_filter);
            let anchor = ui.search_anchor.take();
            sync_target_selection(ui, app, anchor.as_ref());
            ui.mode = Mode::Normal;
            UiCommand::None
        }
        KeyCode::Enter => {
            if ui.target_filter.is_empty() {
                let anchor = ui.search_anchor.clone();
                sync_target_selection(ui, app, anchor.as_ref());
            }
            ui.search_anchor = None;
            ui.search_original_filter.clear();
            ui.mode = Mode::Normal;
            UiCommand::None
        }
        KeyCode::Backspace => {
            let current = selected_target_id(ui, app).cloned();
            ui.target_filter.pop();
            let preferred = if ui.target_filter.is_empty() {
                ui.search_anchor.clone()
            } else {
                current
            };
            sync_target_selection(ui, app, preferred.as_ref());
            UiCommand::None
        }
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            let current = selected_target_id(ui, app).cloned();
            ui.target_filter.push(character);
            sync_target_selection(ui, app, current.as_ref());
            UiCommand::None
        }
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
            ui.form = ForwardForm::default();
            ui.editing_rule = None;
            ui.pending_draft = None;
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
        KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if ui.form.selected_field == 0 => {
            ui.form.toggle_kind();
            UiCommand::None
        }
        KeyCode::Backspace => {
            if let Some(value) = ui.form.selected_value_mut() {
                value.pop();
            }
            UiCommand::None
        }
        KeyCode::Char('l' | 'L') if ui.form.selected_field == 0 => {
            ui.form.set_kind(ForwardFormKind::Local);
            UiCommand::None
        }
        KeyCode::Char('s' | 'S') if ui.form.selected_field == 0 => {
            ui.form.set_kind(ForwardFormKind::Socks);
            UiCommand::None
        }
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            if let Some(value) = ui.form.selected_value_mut() {
                value.push(character);
            }
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
                Ok(draft) => finish_forward_form(ui, target_id, draft),
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
            finish_forward_form(ui, target_id, draft)
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
    log_ui_command(&command);
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
        UiCommand::UpdateForward(rule_id, draft) => match app.update_rule(&rule_id, draft) {
            Ok(()) => set_success(ui, "転送ルールを更新しました（未有効）"),
            Err(error) => set_action_error(ui, &error),
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

fn log_ui_command(command: &UiCommand) {
    let operation_id = crate::logging::next_operation_id();
    match command {
        UiCommand::None => {}
        UiCommand::Connect(target_id) => log_target_command("connect", operation_id, target_id),
        UiCommand::Disconnect(target_id) => {
            log_target_command("disconnect", operation_id, target_id);
        }
        UiCommand::Check(target_id) => log_target_command("check", operation_id, target_id),
        UiCommand::AddForward(target_id, _) => {
            log_target_command("add_forward_rule", operation_id, target_id);
        }
        UiCommand::UpdateForward(rule_id, _) => {
            log_rule_command("update_forward_rule", operation_id, rule_id);
        }
        UiCommand::ActivateForward(rule_id) => {
            log_rule_command("activate_forward", operation_id, rule_id);
        }
        UiCommand::CancelForward(rule_id) => {
            log_rule_command("cancel_forward", operation_id, rule_id);
        }
        UiCommand::DeleteForward(rule_id) => {
            log_rule_command("delete_forward_rule", operation_id, rule_id);
        }
        UiCommand::Quit => tracing::debug!(
            component = "tui",
            operation = "quit",
            operation_id,
            "TUI command requested"
        ),
    }
}

fn log_target_command(operation: &'static str, operation_id: u64, target_id: &TargetId) {
    tracing::debug!(
        component = "tui",
        operation,
        operation_id,
        target_id = target_id.as_str(),
        "TUI command requested"
    );
}

fn log_rule_command(operation: &'static str, operation_id: u64, rule_id: &ForwardRuleId) {
    tracing::debug!(
        component = "tui",
        operation,
        operation_id,
        rule_id = rule_id.as_str(),
        "TUI command requested"
    );
}

fn move_selection<B: SshClient, P: PortProbe>(
    ui: &mut UiState,
    app: &AppState<B, P>,
    forward: bool,
) {
    match ui.focus {
        Focus::Targets => {
            let count = visible_target_indices(ui, app).len();
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

fn begin_rule_edit<B: SshClient, P: PortProbe>(
    ui: &mut UiState,
    app: &AppState<B, P>,
) -> UiCommand {
    if ui.focus != Focus::Forwards {
        return UiCommand::None;
    }
    let Some(rule_id) = selected_rule_id(ui, app).cloned() else {
        return UiCommand::None;
    };
    match app.editable_rule(&rule_id) {
        Ok(rule) => {
            ui.form = ForwardForm::from_rule(rule);
            ui.editing_rule = Some(rule_id);
            ui.pending_draft = None;
            ui.mode = Mode::ForwardForm;
        }
        Err(error) => set_action_error(ui, &error),
    }
    UiCommand::None
}

fn finish_forward_form(
    ui: &mut UiState,
    target_id: TargetId,
    draft: ForwardRuleDraft,
) -> UiCommand {
    let command = match ui.editing_rule.take() {
        Some(rule_id) => UiCommand::UpdateForward(rule_id, draft),
        None => UiCommand::AddForward(target_id, draft),
    };
    ui.form = ForwardForm::default();
    ui.pending_draft = None;
    ui.mode = Mode::Normal;
    command
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
            target_filter: String::new(),
            search_original_filter: String::new(),
            search_anchor: None,
            mode: Mode::Normal,
            status: "準備完了".to_owned(),
            error_detail: None,
            form: ForwardForm::default(),
            pending_draft: None,
            editing_rule: None,
            should_quit: false,
        }
    }
}

impl UiState {
    /// Keeps selections valid after catalog changes.
    pub fn clamp<B: SshClient, P: PortProbe>(&mut self, app: &AppState<B, P>) {
        let target_count = visible_target_indices(self, app).len();
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
    remote: Option<String>,
    local_port: u16,
    remote_port: Option<u16>,
    kind: ForwardFormKind,
}

/// Renders the complete current application state.
pub fn render<B: SshClient, P: PortProbe>(
    frame: &mut Frame<'_>,
    ui: &UiState,
    app: &AppState<B, P>,
) {
    let area = frame.area();
    if area.width < 24 || area.height < 6 {
        let message = if ui.mode == Mode::TargetSearch {
            format!(
                "Search /{} [{}]\nq: quit",
                ui.target_filter,
                visible_target_indices(ui, app).len()
            )
        } else {
            "portdeck: terminal too small\nq: quit".to_owned()
        };
        frame.render_widget(
            Paragraph::new(message).block(Block::default().borders(Borders::ALL)),
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
    render_footer(frame, rows[1], ui, app);
    render_modal(frame, area, ui);
}

fn render_targets<B: SshClient, P: PortProbe>(
    frame: &mut Frame<'_>,
    area: Rect,
    ui: &UiState,
    app: &AppState<B, P>,
) {
    let entries = app.sessions().entries();
    let target_rows = visible_target_indices(ui, app)
        .into_iter()
        .map(|index| &entries[index])
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
    let mut items = target_rows
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
    if items.is_empty() {
        items.push(ListItem::new("一致する接続先がありません"));
    }
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
                match row.kind {
                    ForwardFormKind::Local => ListItem::new(format!(
                        "{} {}→{} {} {}",
                        forward_symbol(row.state),
                        row.local_port,
                        row.remote_port.expect("Local rows have remote ports"),
                        row.label,
                        forward_label(row.state)
                    )),
                    ForwardFormKind::Socks => ListItem::new(format!(
                        "{} SOCKS {} {} {}",
                        forward_symbol(row.state),
                        row.local_port,
                        row.label,
                        forward_label(row.state)
                    )),
                }
            } else {
                match &row.remote {
                    Some(remote) => ListItem::new(format!(
                        "{} {}  {} → {}  {}",
                        forward_symbol(row.state),
                        row.label,
                        row.local,
                        remote,
                        forward_label(row.state)
                    )),
                    None => ListItem::new(format!(
                        "{} {}  SOCKS {}  {}",
                        forward_symbol(row.state),
                        row.label,
                        row.local,
                        forward_label(row.state)
                    )),
                }
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
                .unwrap_or_else(|| rule.default_local_port());
            let (kind, remote, remote_port) = match &rule.kind {
                ForwardKind::Local {
                    remote_host,
                    remote_port,
                } => (
                    ForwardFormKind::Local,
                    Some(display_endpoint(remote_host, *remote_port)),
                    Some(*remote_port),
                ),
                ForwardKind::Socks => (ForwardFormKind::Socks, None, None),
            };
            ForwardRow {
                label: rule
                    .label
                    .clone()
                    .unwrap_or_else(|| rule.id.as_str().to_owned()),
                state,
                local: display_endpoint(&rule.bind_address, local_port),
                remote,
                local_port,
                remote_port,
                kind,
            }
        })
        .collect()
}

fn render_footer<B: SshClient, P: PortProbe>(
    frame: &mut Frame<'_>,
    area: Rect,
    ui: &UiState,
    app: &AppState<B, P>,
) {
    let help = match ui.mode {
        Mode::Normal => {
            "/: search  Tab/h/l: pane  ↑↓: select  c: connect  d: disconnect  a: add  e: edit  Space: activate/cancel  D: delete  r: check  E: error  q: quit"
        }
        Mode::TargetSearch => "Type: filter  Backspace: delete  Enter: apply  Esc: cancel",
        Mode::ForwardForm => "Tab/↑↓: field  Enter: save  Esc: cancel",
        Mode::PublicBindWarning | Mode::Confirm(_) => "y/Enter: confirm  n/Esc: cancel",
        Mode::ErrorDetails => "Esc/E/Enter: close",
    };
    let status = if ui.mode == Mode::TargetSearch {
        format!(
            "Search /{}  [{} matches]",
            ui.target_filter,
            visible_target_indices(ui, app).len()
        )
    } else {
        ui.status.clone()
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![Span::styled(
                status,
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
        Mode::TargetSearch => {}
        Mode::ForwardForm => render_forward_form(frame, area, ui),
        Mode::PublicBindWarning => {
            let message = if ui
                .pending_draft
                .as_ref()
                .is_some_and(|draft| draft.kind == ForwardKind::Socks)
            {
                "警告: 認証のないSOCKS proxyがローカル側の他ホストへ公開されます。\n接続可能な利用者はSSH経由で任意の宛先へ通信できます。\n\n続行しますか？ [y/N]"
            } else {
                "このbind addressはローカル側の他ホストへ公開される可能性があります。\n\n続行しますか？ [y/N]"
            };
            render_message_modal(frame, area, " Public bind warning ", message, Color::Red)
        }
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
    let mut fields = vec![
        ("Forward kind (←/→)", ui.form.kind.label()),
        ("Label (optional)", ui.form.label.as_str()),
        ("Local bind address", ui.form.bind_address.as_str()),
        ("Preferred local port", ui.form.local_port.as_str()),
    ];
    if ui.form.kind == ForwardFormKind::Local {
        fields.extend([
            ("Remote destination host", ui.form.remote_host.as_str()),
            ("Remote destination port *", ui.form.remote_port.as_str()),
        ]);
    }
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
                    .title(if ui.editing_rule.is_some() {
                        " Edit saved forward "
                    } else {
                        " Add saved forward "
                    })
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
    let entry_index = *visible_target_indices(ui, app).get(ui.selected_target)?;
    app.sessions()
        .entries()
        .get(entry_index)
        .map(|entry| &entry.target.id)
}

fn visible_target_indices<B: SshClient, P: PortProbe>(
    ui: &UiState,
    app: &AppState<B, P>,
) -> Vec<usize> {
    let query = ui.target_filter.to_lowercase();
    app.sessions()
        .entries()
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            (query.is_empty() || entry.target.host_alias.to_lowercase().contains(&query))
                .then_some(index)
        })
        .collect()
}

fn sync_target_selection<B: SshClient, P: PortProbe>(
    ui: &mut UiState,
    app: &AppState<B, P>,
    preferred: Option<&TargetId>,
) {
    let visible = visible_target_indices(ui, app);
    ui.selected_target = preferred
        .and_then(|target_id| {
            visible
                .iter()
                .position(|index| app.sessions().entries()[*index].target.id == *target_id)
        })
        .unwrap_or(0);
    ui.selected_forward = 0;
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
    use std::sync::atomic::{AtomicU64, Ordering};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::{Backend, TestBackend};

    use crate::application::{AppState, ForwardRuleDraft, PortProbe, SessionManager};
    use crate::domain::{ForwardKind, ForwardRuleId, Target, TargetId};
    use crate::runtime::RuntimeDirectory;
    use crate::ssh::{DynamicForwardSpec, LocalForwardSpec, SshClient, SshError, SshOutput};

    use super::{
        Confirmation, Focus, ForwardForm, Mode, UiCommand, UiState, handle_key,
        invalidate_previous_frame, render,
    };

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

        fn add_dynamic_forward(
            &self,
            _host_alias: &str,
            _control_path: &Path,
            _forward: &DynamicForwardSpec,
        ) -> Result<SshOutput, SshError> {
            Ok(success())
        }

        fn cancel_dynamic_forward(
            &self,
            _host_alias: &str,
            _control_path: &Path,
            _forward: &DynamicForwardSpec,
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

    static NEXT_TEST_RUNTIME_ID: AtomicU64 = AtomicU64::new(0);

    impl Fixture {
        fn new() -> Self {
            Self::with_aliases(&["dev-server"])
        }

        fn with_aliases(aliases: &[&str]) -> Self {
            let unique = NEXT_TEST_RUNTIME_ID.fetch_add(1, Ordering::Relaxed);
            let runtime_path =
                Path::new("/tmp").join(format!("t{:x}{unique:x}", std::process::id()));
            let runtime = RuntimeDirectory::prepare(&runtime_path).unwrap();
            let targets = aliases
                .iter()
                .enumerate()
                .map(|(index, alias)| Target {
                    id: TargetId::new(if index == 0 {
                        "dev".to_owned()
                    } else {
                        format!("target-{index}")
                    }),
                    host_alias: (*alias).to_owned(),
                    source: PathBuf::from("config"),
                })
                .collect();
            let manager =
                SessionManager::with_port_probe(FakeSsh::default(), runtime, targets, Available)
                    .unwrap();
            let mut app = AppState::new(manager, Vec::new()).unwrap();
            if !aliases.is_empty() {
                app.add_rule(
                    &TargetId::new("dev"),
                    ForwardRuleDraft::parse("web", "", "8080", "", "3000").unwrap(),
                )
                .unwrap();
            }
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
    fn invalidating_after_external_clear_forces_an_identical_frame_to_redraw() {
        let mut terminal = Terminal::new(TestBackend::new(12, 2)).unwrap();
        terminal
            .draw(|frame| frame.render_widget("unchanged", frame.area()))
            .unwrap();
        terminal.backend_mut().clear().unwrap();

        invalidate_previous_frame(&mut terminal);
        terminal
            .draw(|frame| frame.render_widget("unchanged", frame.area()))
            .unwrap();

        let contents = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(contents.contains("unchanged"));
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
    fn mixed_local_and_socks_rules_render_in_wide_and_narrow_terminals() {
        let mut fixture = Fixture::new();
        fixture
            .app
            .add_rule(
                &TargetId::new("dev"),
                ForwardRuleDraft::parse_socks("browser", "", "").unwrap(),
            )
            .unwrap();

        for (width, expected) in [(100, "SOCKS 127.0.0.1:1080"), (50, "SOCKS 1080")] {
            let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
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
            assert!(contents.contains("8080"));
            assert!(contents.contains("3000"));
            assert!(
                contents.contains(expected),
                "missing {expected:?} at width {width}"
            );
        }
    }

    #[test]
    fn form_navigation_wraps_and_parses_defaults() {
        let mut ui = UiState {
            mode: Mode::ForwardForm,
            ..UiState::default()
        };
        ui.form.previous_field();
        assert_eq!(ui.form.selected_field, 5);
        ui.form.remote_port = "3000".to_owned();
        assert_eq!(
            ui.form.draft().unwrap().kind,
            ForwardKind::Local {
                remote_host: "127.0.0.1".to_owned(),
                remote_port: 3000,
            }
        );
    }

    #[test]
    fn form_selects_socks_with_loopback_1080_and_hides_remote_fields() {
        let fixture = Fixture::new();
        let mut ui = UiState {
            mode: Mode::ForwardForm,
            ..UiState::default()
        };

        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        );

        assert_eq!(ui.form.kind, super::ForwardFormKind::Socks);
        assert_eq!(ui.form.bind_address, "127.0.0.1");
        assert_eq!(ui.form.local_port, "1080");
        assert_eq!(ui.form.field_count(), 4);
        let draft = ui.form.draft().unwrap();
        assert_eq!(draft.kind, ForwardKind::Socks);
        assert_eq!(draft.requested_local_port, Some(1080));

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
        assert!(contents.contains("Forward kind (←/→): SOCKS"));
        assert!(!contents.contains("Remote destination"));
    }

    #[test]
    fn socks_edit_form_preserves_kind_and_listener() {
        let mut fixture = Fixture::new();
        fixture
            .app
            .add_rule(
                &TargetId::new("dev"),
                ForwardRuleDraft::parse_socks("browser", "::1", "1081").unwrap(),
            )
            .unwrap();
        let mut ui = UiState {
            focus: Focus::Forwards,
            selected_forward: 1,
            ..UiState::default()
        };

        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        );

        assert_eq!(ui.form.kind, super::ForwardFormKind::Socks);
        assert_eq!(ui.form.bind_address, "::1");
        assert_eq!(ui.form.local_port, "1081");
        assert_eq!(ui.form.remote_host, "");
        assert_eq!(ui.form.remote_port, "");
    }

    #[test]
    fn edit_form_is_prefilled_from_the_selected_rule() {
        let fixture = Fixture::new();
        let mut ui = UiState {
            focus: Focus::Forwards,
            ..UiState::default()
        };

        assert_eq!(
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            ),
            UiCommand::None
        );

        assert_eq!(ui.mode, Mode::ForwardForm);
        assert_eq!(ui.editing_rule, Some(ForwardRuleId::new("rule-00000001")));
        assert_eq!(
            ui.form,
            ForwardForm {
                selected_field: 0,
                kind: super::ForwardFormKind::Local,
                label: "web".to_owned(),
                bind_address: "127.0.0.1".to_owned(),
                local_port: "8080".to_owned(),
                remote_host: "127.0.0.1".to_owned(),
                remote_port: "3000".to_owned(),
            }
        );
    }

    #[test]
    fn edit_form_render_identifies_editing_and_normal_help_shows_distinct_keys() {
        let fixture = Fixture::new();
        let mut ui = UiState {
            focus: Focus::Forwards,
            ..UiState::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(180, 20)).unwrap();
        terminal
            .draw(|frame| render(frame, &ui, &fixture.app))
            .unwrap();
        let normal = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(normal.contains("e: edit"));
        assert!(normal.contains("E: error"));

        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        );
        terminal
            .draw(|frame| render(frame, &ui, &fixture.app))
            .unwrap();
        let editing = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(editing.contains("Edit saved forward"));
        assert!(editing.contains("web"));
        assert!(editing.contains("8080"));
        assert!(editing.contains("3000"));
    }

    #[test]
    fn edit_form_enter_updates_in_place_and_escape_cancels() {
        let mut fixture = Fixture::new();
        let mut ui = UiState {
            focus: Focus::Forwards,
            ..UiState::default()
        };
        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        );
        ui.form.label = "edited".to_owned();
        ui.form.bind_address = "::1".to_owned();
        ui.form.local_port = "18080".to_owned();
        ui.form.remote_host = "web.internal".to_owned();
        ui.form.remote_port = "4000".to_owned();

        let command = handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        let UiCommand::UpdateForward(rule_id, draft) = command else {
            panic!("expected an update command");
        };
        fixture.app.update_rule(&rule_id, draft).unwrap();

        assert_eq!(ui.selected_forward, 0);
        let edited = &fixture.app.rules_for(&TargetId::new("dev"))[0];
        assert_eq!(edited.id, ForwardRuleId::new("rule-00000001"));
        assert_eq!(edited.label.as_deref(), Some("edited"));
        assert_eq!(edited.bind_address, "::1");
        assert_eq!(edited.requested_local_port, Some(18080));
        assert_eq!(
            edited.kind,
            ForwardKind::Local {
                remote_host: "web.internal".to_owned(),
                remote_port: 4000,
            }
        );

        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        );
        ui.form.label = "discarded".to_owned();
        assert_eq!(
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            ),
            UiCommand::None
        );
        assert_eq!(ui.mode, Mode::Normal);
        assert_eq!(ui.editing_rule, None);
        assert_eq!(
            fixture.app.rules_for(&TargetId::new("dev"))[0]
                .label
                .as_deref(),
            Some("edited")
        );
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
    fn vim_and_direction_keys_select_panes_without_toggling() {
        let fixture = Fixture::new();
        let mut ui = UiState::default();

        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
        );
        assert_eq!(ui.focus, Focus::Forwards);
        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
        );
        assert_eq!(ui.focus, Focus::Forwards);
        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
        );
        assert_eq!(ui.focus, Focus::Targets);
        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
        );
        assert_eq!(ui.focus, Focus::Targets);

        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        );
        assert_eq!(ui.focus, Focus::Forwards);
        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        );
        assert_eq!(ui.focus, Focus::Targets);
        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        );
        assert_eq!(ui.focus, Focus::Forwards);
    }

    #[test]
    fn pane_navigation_and_render_are_safe_with_empty_lists() {
        let fixture = Fixture::with_aliases(&[]);
        let mut ui = UiState::default();

        for code in [
            KeyCode::Char('l'),
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Char('h'),
            KeyCode::Char('j'),
            KeyCode::Char('k'),
        ] {
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(code, KeyModifiers::NONE),
            );
        }
        assert_eq!(ui.focus, Focus::Targets);
        assert_eq!(ui.selected_target, 0);
        assert_eq!(ui.selected_forward, 0);

        let mut terminal = Terminal::new(TestBackend::new(60, 8)).unwrap();
        terminal
            .draw(|frame| render(frame, &ui, &fixture.app))
            .unwrap();
    }

    #[test]
    fn vim_pane_keys_remain_text_in_forward_form() {
        let fixture = Fixture::new();
        let mut ui = UiState {
            mode: Mode::ForwardForm,
            ..UiState::default()
        };
        ui.form.selected_field = 1;

        for character in ['h', 'l'] {
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            );
        }

        assert_eq!(ui.form.label, "hl");
        assert_eq!(ui.focus, Focus::Targets);
    }

    #[test]
    fn target_search_filters_case_insensitively_and_keeps_matching_selection() {
        let fixture = Fixture::with_aliases(&["dev-server", "PROD-DB", "staging-dev"]);
        let mut ui = UiState {
            selected_target: 2,
            focus: Focus::Forwards,
            ..UiState::default()
        };

        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        );
        for character in "DEV".chars() {
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            );
        }

        assert_eq!(ui.mode, Mode::TargetSearch);
        assert_eq!(ui.focus, Focus::Targets);
        assert_eq!(ui.target_filter, "DEV");
        assert_eq!(
            super::selected_target_id(&ui, &fixture.app),
            Some(&TargetId::new("target-2"))
        );

        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(ui.mode, Mode::Normal);
    }

    #[test]
    fn target_search_falls_back_to_first_match_and_routes_target_commands() {
        let fixture = Fixture::with_aliases(&["dev-server", "PROD-DB", "staging-dev"]);
        let mut ui = UiState::default();

        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        );
        for character in "prod".chars() {
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            );
        }
        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );

        assert_eq!(
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
            ),
            UiCommand::Connect(TargetId::new("target-1"))
        );
        assert_eq!(
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
            ),
            UiCommand::Check(TargetId::new("target-1"))
        );
        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert_eq!(
            ui.mode,
            Mode::Confirm(Confirmation::Disconnect(TargetId::new("target-1")))
        );
    }

    #[test]
    fn filtered_target_and_forwards_panes_reference_the_same_target() {
        let mut fixture = Fixture::with_aliases(&["dev-server", "PROD-DB"]);
        fixture
            .app
            .add_rule(
                &TargetId::new("target-1"),
                ForwardRuleDraft::parse("database", "", "5432", "db", "5432").unwrap(),
            )
            .unwrap();
        let ui = UiState {
            target_filter: "prod".to_owned(),
            ..UiState::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();

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
        assert!(contents.contains("PROD-DB"));
        assert!(contents.contains("database"));
        assert!(contents.contains("127.0.0.1:5432"));
        assert!(!contents.contains("dev-server"));
        assert!(!contents.contains("web"));
    }

    #[test]
    fn edit_uses_the_rule_from_the_filtered_target() {
        let mut fixture = Fixture::with_aliases(&["dev-server", "PROD-DB"]);
        let rule_id = fixture
            .app
            .add_rule(
                &TargetId::new("target-1"),
                ForwardRuleDraft::parse("database", "", "5432", "db", "5432").unwrap(),
            )
            .unwrap();
        let mut ui = UiState {
            focus: Focus::Forwards,
            target_filter: "prod".to_owned(),
            ..UiState::default()
        };

        assert_eq!(
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            ),
            UiCommand::None
        );

        assert_eq!(ui.mode, Mode::ForwardForm);
        assert_eq!(ui.editing_rule, Some(rule_id));
        assert_eq!(ui.form.label, "database");
        assert_eq!(ui.form.remote_host, "db");
        assert_eq!(ui.form.remote_port, "5432");
    }

    #[test]
    fn mixed_local_and_socks_edit_uses_the_filtered_target() {
        let mut fixture = Fixture::with_aliases(&["dev-server", "PROD-DB"]);
        fixture
            .app
            .add_rule(
                &TargetId::new("target-1"),
                ForwardRuleDraft::parse("database", "", "5432", "db", "5432").unwrap(),
            )
            .unwrap();
        let socks_id = fixture
            .app
            .add_rule(
                &TargetId::new("target-1"),
                ForwardRuleDraft::parse_socks("browser", "::1", "1081").unwrap(),
            )
            .unwrap();
        let mut ui = UiState {
            focus: Focus::Forwards,
            selected_forward: 1,
            target_filter: "prod".to_owned(),
            ..UiState::default()
        };

        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        );
        assert_eq!(ui.editing_rule, Some(socks_id.clone()));
        assert_eq!(ui.form.kind, super::ForwardFormKind::Socks);
        assert_eq!(ui.form.bind_address, "::1");
        assert_eq!(ui.form.local_port, "1081");
        ui.form.label = "edited proxy".to_owned();

        let UiCommand::UpdateForward(rule_id, draft) = handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        ) else {
            panic!("expected filtered SOCKS update command");
        };
        assert_eq!(rule_id, socks_id);
        assert_eq!(draft.kind, ForwardKind::Socks);
        fixture.app.update_rule(&rule_id, draft).unwrap();

        let rules = fixture.app.rules_for(&TargetId::new("target-1"));
        assert!(matches!(rules[0].kind, ForwardKind::Local { .. }));
        assert_eq!(rules[1].kind, ForwardKind::Socks);
        assert_eq!(rules[1].label.as_deref(), Some("edited proxy"));
    }

    #[test]
    fn cancelling_search_restores_filter_and_pre_search_target() {
        let fixture = Fixture::with_aliases(&["dev-server", "PROD-DB", "staging-dev"]);
        let mut ui = UiState {
            selected_target: 1,
            target_filter: "dev".to_owned(),
            ..UiState::default()
        };

        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        );
        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert!(super::selected_target_id(&ui, &fixture.app).is_none());
        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );

        assert_eq!(ui.mode, Mode::Normal);
        assert_eq!(ui.target_filter, "dev");
        assert_eq!(
            super::selected_target_id(&ui, &fixture.app),
            Some(&TargetId::new("target-2"))
        );
    }

    #[test]
    fn empty_confirmed_search_clears_filter_and_restores_anchor() {
        let fixture = Fixture::with_aliases(&["dev-server", "PROD-DB", "staging-dev"]);
        let mut ui = UiState {
            target_filter: "prod".to_owned(),
            ..UiState::default()
        };

        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        );
        for _ in 0..4 {
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            );
        }
        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );

        assert!(ui.target_filter.is_empty());
        assert_eq!(
            super::selected_target_id(&ui, &fixture.app),
            Some(&TargetId::new("target-1"))
        );
    }

    #[test]
    fn search_accepts_vim_keys_as_text_and_ctrl_c_keeps_quit_semantics() {
        let fixture = Fixture::new();
        let mut ui = UiState {
            mode: Mode::TargetSearch,
            ..UiState::default()
        };

        for character in ['h', 'l'] {
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            );
        }
        assert_eq!(ui.target_filter, "hl");
        assert_eq!(
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            ),
            UiCommand::Quit
        );
    }

    #[test]
    fn zero_match_search_renders_empty_state_and_search_status() {
        let fixture = Fixture::new();
        let ui = UiState {
            target_filter: "missing".to_owned(),
            mode: Mode::TargetSearch,
            ..UiState::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 10)).unwrap();

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
        assert!(
            contents
                .replace(' ', "")
                .contains("一致する接続先がありません")
        );
        assert!(contents.contains("Search /missing  [0 matches]"));
    }

    #[test]
    fn tiny_terminal_search_render_never_panics() {
        let fixture = Fixture::new();
        let ui = UiState {
            target_filter: "x".to_owned(),
            mode: Mode::TargetSearch,
            ..UiState::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(20, 4)).unwrap();

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
        assert!(contents.contains("Search /x [0]"));
        assert!(contents.contains("q: quit"));
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
    fn public_socks_bind_warns_about_unauthenticated_proxy_access() {
        let fixture = Fixture::new();
        let mut ui = UiState {
            mode: Mode::ForwardForm,
            ..UiState::default()
        };
        ui.form.set_kind(super::ForwardFormKind::Socks);
        ui.form.bind_address = "0.0.0.0".to_owned();

        assert_eq!(
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            ),
            UiCommand::None
        );
        assert_eq!(ui.mode, Mode::PublicBindWarning);
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
        assert!(contents.contains("SOCKS proxy"));
        assert!(contents.contains("任 意 の 宛 先"));
    }

    #[test]
    fn public_bind_edit_keeps_update_intent_across_warning_cancel_and_confirm() {
        let fixture = Fixture::new();
        let mut ui = UiState {
            focus: Focus::Forwards,
            ..UiState::default()
        };
        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        );
        ui.form.bind_address = "0.0.0.0".to_owned();

        assert_eq!(
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            ),
            UiCommand::None
        );
        assert_eq!(ui.mode, Mode::PublicBindWarning);
        assert_eq!(
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            ),
            UiCommand::None
        );
        assert_eq!(ui.mode, Mode::ForwardForm);
        assert!(ui.editing_rule.is_some());

        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        let confirmed = handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        );
        assert!(matches!(confirmed, UiCommand::UpdateForward(_, _)));
    }

    #[test]
    fn active_rule_edit_is_rejected_with_cancellation_guidance() {
        let mut fixture = Fixture::new();
        let rule_id = fixture.app.rules_for(&TargetId::new("dev"))[0].id.clone();
        fixture
            .app
            .sessions_mut()
            .connect(&TargetId::new("dev"))
            .unwrap();
        fixture.app.activate_rule(&rule_id).unwrap();
        let mut ui = UiState {
            focus: Focus::Forwards,
            ..UiState::default()
        };

        assert_eq!(
            handle_key(
                &mut ui,
                &fixture.app,
                KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            ),
            UiCommand::None
        );
        assert_eq!(ui.mode, Mode::Normal);
        assert!(ui.status.contains("Space"));
        assert!(ui.status.contains("取消してから編集"));
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
        let key = KeyEvent::new(KeyCode::Char('E'), KeyModifiers::SHIFT);

        handle_key(&mut ui, &fixture.app, key);
        assert_eq!(ui.mode, Mode::Normal);

        ui.error_detail = Some("OpenSSH stderr".to_owned());
        handle_key(&mut ui, &fixture.app, key);
        assert_eq!(ui.mode, Mode::ErrorDetails);

        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('E'), KeyModifiers::SHIFT),
        );
        assert_eq!(ui.mode, Mode::Normal);

        handle_key(
            &mut ui,
            &fixture.app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        );
        assert_eq!(ui.mode, Mode::Normal);
    }
}
