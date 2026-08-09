//! Application services coordinating domain state and OpenSSH operations.

use std::collections::BTreeMap;
use std::io;
use std::net::TcpListener;
use std::time::Instant;

use thiserror::Error;

use crate::config::{EffectiveSshConfig, RuleStore, StoreError, parse_effective_config};
use crate::domain::{
    ActiveForward, Failure, FailureKind, ForwardRule, ForwardRuleId, ForwardState, Session,
    SessionState, Target, TargetId, TransitionError,
};
use crate::runtime::{RuntimeDirectory, RuntimeError};
use crate::ssh::{LocalForwardSpec, SshClient, SshError, SshOutput};

/// Maximum number of sequential ports considered for one add operation.
pub const DEFAULT_PORT_ATTEMPTS: usize = 20;

/// Validated values used to create a saved forward rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardRuleDraft {
    /// Optional display label.
    pub label: Option<String>,
    /// Local-side bind address.
    pub bind_address: String,
    /// Preferred local port, or the remote port when omitted.
    pub requested_local_port: Option<u16>,
    /// Host reached from the remote side.
    pub remote_host: String,
    /// Port reached from the remote side.
    pub remote_port: u16,
    /// Whether activation will expose the listener beyond loopback.
    pub public_bind: bool,
}

impl ForwardRuleDraft {
    /// Parses form values and applies loopback defaults.
    pub fn parse(
        label: &str,
        bind_address: &str,
        requested_local_port: &str,
        remote_host: &str,
        remote_port: &str,
    ) -> Result<Self, RuleError> {
        if label.contains(['\0', '\n', '\r']) {
            return Err(RuleError::InvalidLabel);
        }
        let label = (!label.trim().is_empty()).then(|| label.trim().to_owned());
        let bind_address = if bind_address.trim().is_empty() {
            "127.0.0.1".to_owned()
        } else {
            bind_address.trim().to_owned()
        };
        let remote_host = if remote_host.trim().is_empty() {
            "127.0.0.1".to_owned()
        } else {
            remote_host.trim().to_owned()
        };
        let remote_port = parse_port(remote_port, "リモート宛先ポート")?;
        let requested_local_port = if requested_local_port.trim().is_empty() {
            None
        } else {
            Some(parse_port(requested_local_port, "希望ローカルポート")?)
        };
        let validation_port = requested_local_port.unwrap_or(remote_port);
        let spec = LocalForwardSpec::new(&bind_address, validation_port, &remote_host, remote_port)
            .map_err(|error| RuleError::InvalidForward(error.to_string()))?;

        Ok(Self {
            label,
            bind_address,
            requested_local_port,
            remote_host,
            remote_port,
            public_bind: spec.is_public_bind(),
        })
    }
}

/// Persistent definitions plus runtime session/forward services.
#[derive(Debug)]
pub struct AppState<B, P = SystemPortProbe> {
    sessions: SessionManager<B, P>,
    rules: BTreeMap<TargetId, Vec<ForwardRule>>,
    next_rule_number: u64,
    rule_store: Option<RuleStore>,
}

impl<B: SshClient, P: PortProbe> AppState<B, P> {
    /// Creates application state from discovered sessions and saved rules.
    pub fn new(sessions: SessionManager<B, P>, rules: Vec<ForwardRule>) -> Result<Self, RuleError> {
        let target_ids = sessions
            .entries()
            .iter()
            .map(|entry| entry.target.id.clone())
            .collect::<Vec<_>>();
        let mut grouped_rules: BTreeMap<TargetId, Vec<ForwardRule>> = BTreeMap::new();
        let mut seen_rule_ids = std::collections::HashSet::new();
        for rule in rules {
            if !target_ids.contains(&rule.target_id) {
                continue;
            }
            if !seen_rule_ids.insert(rule.id.clone()) {
                return Err(RuleError::DuplicateRuleId(rule.id));
            }
            grouped_rules
                .entry(rule.target_id.clone())
                .or_default()
                .push(rule);
        }

        Ok(Self {
            sessions,
            rules: grouped_rules,
            next_rule_number: 1,
            rule_store: None,
        })
    }

    /// Creates application state that persists every definition mutation.
    pub fn with_store(
        sessions: SessionManager<B, P>,
        rules: Vec<ForwardRule>,
        rule_store: RuleStore,
    ) -> Result<Self, RuleError> {
        let mut state = Self::new(sessions, rules)?;
        state.rule_store = Some(rule_store);
        Ok(state)
    }

    /// Session manager used for connection-level operations.
    pub fn sessions(&self) -> &SessionManager<B, P> {
        &self.sessions
    }

    /// Mutable session manager used by the event loop.
    pub fn sessions_mut(&mut self) -> &mut SessionManager<B, P> {
        &mut self.sessions
    }

    /// Saved rules belonging to one target.
    pub fn rules_for(&self, target_id: &TargetId) -> &[ForwardRule] {
        self.rules.get(target_id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// All saved rules, grouped in target discovery order.
    pub fn all_rules(&self) -> Vec<&ForwardRule> {
        self.sessions
            .entries()
            .iter()
            .flat_map(|entry| self.rules_for(&entry.target.id))
            .collect()
    }

    /// Creates an inactive saved rule.
    pub fn add_rule(
        &mut self,
        target_id: &TargetId,
        draft: ForwardRuleDraft,
    ) -> Result<ForwardRuleId, AppActionError> {
        if self.sessions.entry(target_id).is_none() {
            return Err(RuleError::UnknownTarget(target_id.clone()).into());
        }
        let id = self.next_rule_id();
        let rule = ForwardRule {
            id: id.clone(),
            target_id: target_id.clone(),
            label: draft.label,
            bind_address: draft.bind_address,
            requested_local_port: draft.requested_local_port,
            remote_host: draft.remote_host,
            remote_port: draft.remote_port,
        };
        self.rules.entry(target_id.clone()).or_default().push(rule);
        if let Err(error) = self.persist_rules() {
            tracing::debug!(
                component = "persistence",
                operation = "rollback_add_rule",
                operation_id = crate::logging::next_operation_id(),
                target_id = target_id.as_str(),
                rule_id = id.as_str(),
                error_kind = "store",
                "rolling back unsaved forward rule"
            );
            if let Some(rules) = self.rules.get_mut(target_id)
                && let Some(index) = rules.iter().position(|rule| rule.id == id)
            {
                rules.remove(index);
            }
            return Err(error.into());
        }
        Ok(id)
    }

    /// Returns a saved rule only when it has no runtime forwarding data.
    pub fn editable_rule(&self, rule_id: &ForwardRuleId) -> Result<&ForwardRule, AppActionError> {
        let rule = self.find_rule(rule_id)?;
        self.ensure_rule_is_inactive(rule)?;
        Ok(rule)
    }

    /// Replaces the editable fields of one inactive saved rule.
    pub fn update_rule(
        &mut self,
        rule_id: &ForwardRuleId,
        draft: ForwardRuleDraft,
    ) -> Result<(), AppActionError> {
        let current = self.editable_rule(rule_id)?.clone();
        let target_id = current.target_id.clone();
        let rules = self
            .rules
            .get_mut(&target_id)
            .expect("the selected rule's target exists");
        let index = rules
            .iter()
            .position(|rule| &rule.id == rule_id)
            .expect("the selected rule exists");
        rules[index] = ForwardRule {
            id: current.id.clone(),
            target_id: current.target_id.clone(),
            label: draft.label,
            bind_address: draft.bind_address,
            requested_local_port: draft.requested_local_port,
            remote_host: draft.remote_host,
            remote_port: draft.remote_port,
        };
        if let Err(error) = self.persist_rules() {
            self.rules
                .get_mut(&target_id)
                .expect("the selected rule's target exists")[index] = current;
            return Err(error.into());
        }
        Ok(())
    }

    /// Activates one saved rule.
    pub fn activate_rule(&mut self, rule_id: &ForwardRuleId) -> Result<u16, AppActionError> {
        let rule = self.find_rule(rule_id)?.clone();
        self.sessions
            .activate_forward(&rule)
            .map_err(AppActionError::Manager)
    }

    /// Cancels one active saved rule.
    pub fn cancel_rule(&mut self, rule_id: &ForwardRuleId) -> Result<(), AppActionError> {
        let rule = self.find_rule(rule_id)?.clone();
        self.sessions
            .cancel_forward(&rule)
            .map_err(AppActionError::Manager)
    }

    /// Deletes only a rule with no live runtime forwarding data.
    pub fn remove_rule(&mut self, rule_id: &ForwardRuleId) -> Result<(), AppActionError> {
        if let Some(runtime) = self
            .sessions
            .entries()
            .iter()
            .find_map(|entry| entry.forwards.get(rule_id))
            && (runtime.state != ForwardState::Inactive || runtime.actual_local_port.is_some())
        {
            return Err(AppActionError::Rule(RuleError::RuleStillActive(
                rule_id.clone(),
            )));
        }

        let Some((target_id, index)) = self.rules.iter().find_map(|(target_id, rules)| {
            rules
                .iter()
                .position(|rule| &rule.id == rule_id)
                .map(|index| (target_id.clone(), index))
        }) else {
            return Err(AppActionError::Rule(RuleError::UnknownRule(
                rule_id.clone(),
            )));
        };
        let removed = self
            .rules
            .get_mut(&target_id)
            .expect("target was found")
            .remove(index);
        if let Err(error) = self.persist_rules() {
            tracing::debug!(
                component = "persistence",
                operation = "rollback_remove_rule",
                operation_id = crate::logging::next_operation_id(),
                target_id = target_id.as_str(),
                rule_id = removed.id.as_str(),
                error_kind = "store",
                "restoring forward rule after persistence failure"
            );
            self.rules
                .get_mut(&target_id)
                .expect("target was found")
                .insert(index, removed);
            return Err(error.into());
        }
        Ok(())
    }

    fn find_rule(&self, rule_id: &ForwardRuleId) -> Result<&ForwardRule, AppActionError> {
        self.rules
            .values()
            .flatten()
            .find(|rule| &rule.id == rule_id)
            .ok_or_else(|| AppActionError::Rule(RuleError::UnknownRule(rule_id.clone())))
    }

    fn ensure_rule_is_inactive(&self, rule: &ForwardRule) -> Result<(), AppActionError> {
        if let Some(runtime) = self
            .sessions
            .entry(&rule.target_id)
            .and_then(|entry| entry.forwards.get(&rule.id))
            && (runtime.state != ForwardState::Inactive
                || runtime.actual_local_port.is_some()
                || runtime.normalized_spec.is_some())
        {
            return Err(RuleError::RuleNotEditable(rule.id.clone()).into());
        }
        Ok(())
    }

    fn next_rule_id(&mut self) -> ForwardRuleId {
        loop {
            let id = ForwardRuleId::new(format!("rule-{:08}", self.next_rule_number));
            self.next_rule_number = self.next_rule_number.saturating_add(1);
            if self.rules.values().flatten().all(|rule| rule.id != id) {
                return id;
            }
        }
    }

    fn persist_rules(&mut self) -> Result<(), StoreError> {
        let Self {
            sessions,
            rules,
            rule_store,
            ..
        } = self;
        let Some(rule_store) = rule_store else {
            return Ok(());
        };
        let targets = sessions
            .entries()
            .iter()
            .map(|entry| entry.target.clone())
            .collect::<Vec<_>>();
        let rule_references = rules.values().flatten().collect::<Vec<_>>();
        rule_store.save(&targets, &rule_references)
    }
}

/// Saved-rule input or catalog failure.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RuleError {
    /// Labels must be a single printable line.
    #[error("ラベルに改行またはNULは使用できません")]
    InvalidLabel,
    /// Port text was empty, zero, or non-numeric.
    #[error("{field}は1から65535の数値で入力してください")]
    InvalidPort {
        /// Form field name.
        field: &'static str,
    },
    /// A bind or remote address was not representable as `-L`.
    #[error("転送指定が不正です: {0}")]
    InvalidForward(String),
    /// A loaded or selected target is not in the current SSH catalog.
    #[error("unknown SSH target: {}", .0.as_str())]
    UnknownTarget(TargetId),
    /// A selected rule does not exist.
    #[error("unknown forward rule: {}", .0.as_str())]
    UnknownRule(ForwardRuleId),
    /// Loaded configuration reused an identifier.
    #[error("duplicate forward rule ID: {}", .0.as_str())]
    DuplicateRuleId(ForwardRuleId),
    /// Definition deletion would conceal a possibly active OpenSSH forward.
    #[error("forward rule {} still has runtime state", .0.as_str())]
    RuleStillActive(ForwardRuleId),
    /// Definition editing requires cancellation of all runtime state first.
    #[error(
        "転送ルール {} は実行状態が残っています。Spaceで転送を取消してから編集してください",
        .0.as_str()
    )]
    RuleNotEditable(ForwardRuleId),
}

/// Error from a user-requested rule action.
#[derive(Debug, Error)]
pub enum AppActionError {
    /// Session or OpenSSH operation failed.
    #[error(transparent)]
    Manager(#[from] ManagerError),
    /// Saved-rule operation failed.
    #[error(transparent)]
    Rule(#[from] RuleError),
    /// Atomic persistent configuration update failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

fn parse_port(value: &str, field: &'static str) -> Result<u16, RuleError> {
    let port = value
        .trim()
        .parse::<u16>()
        .map_err(|_| RuleError::InvalidPort { field })?;
    if port == 0 {
        return Err(RuleError::InvalidPort { field });
    }
    Ok(port)
}

/// A target together with its session and runtime forwards.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    /// Discovered SSH target.
    pub target: Target,
    /// Portdeck-owned ControlMaster state.
    pub session: Session,
    /// Display-oriented effective values resolved by `ssh -G`.
    pub effective_config: Option<EffectiveSshConfig>,
    /// Runtime states keyed by saved rule ID.
    pub forwards: BTreeMap<ForwardRuleId, ActiveForward>,
}

/// Result of inspecting runtime entries left by an earlier process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Live known masters explicitly stopped during recovery.
    pub terminated_targets: Vec<TargetId>,
    /// Known socket paths removed only after a failed `-O check`.
    pub removed_stale_paths: Vec<std::path::PathBuf>,
    /// Namespaced paths that could not be mapped to a current target.
    pub unknown_paths: Vec<std::path::PathBuf>,
    /// Failures retained for user-visible diagnostics.
    pub failures: Vec<Failure>,
}

/// Side-effect-free port availability boundary used before asking OpenSSH.
pub trait PortProbe {
    /// Returns `true` when a temporary bind succeeds and `false` for a conflict.
    fn is_available(&self, bind_address: &str, port: u16) -> io::Result<bool>;
}

/// Checks availability using a temporary TCP listener.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemPortProbe;

impl PortProbe for SystemPortProbe {
    fn is_available(&self, bind_address: &str, port: u16) -> io::Result<bool> {
        let bind_address = if bind_address == "*" {
            "0.0.0.0"
        } else {
            bind_address
        };
        match TcpListener::bind((bind_address, port)) {
            Ok(listener) => {
                drop(listener);
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => Ok(false),
            Err(error) => Err(error),
        }
    }
}

/// Coordinates all portdeck-owned sessions and active forwards.
#[derive(Debug)]
pub struct SessionManager<B, P = SystemPortProbe> {
    ssh: B,
    runtime: RuntimeDirectory,
    entries: Vec<SessionEntry>,
    port_probe: P,
    port_attempts: usize,
}

impl<B: SshClient> SessionManager<B, SystemPortProbe> {
    /// Creates a manager using real local bind probes.
    pub fn new(
        ssh: B,
        runtime: RuntimeDirectory,
        targets: Vec<Target>,
    ) -> Result<Self, ManagerError> {
        Self::with_port_probe(ssh, runtime, targets, SystemPortProbe)
    }
}

impl<B: SshClient, P: PortProbe> SessionManager<B, P> {
    /// Creates a manager with a replaceable local-port probe.
    pub fn with_port_probe(
        ssh: B,
        runtime: RuntimeDirectory,
        targets: Vec<Target>,
        port_probe: P,
    ) -> Result<Self, ManagerError> {
        let entries = targets
            .into_iter()
            .map(|target| {
                let control_path = runtime.control_path(&target.id)?;
                let session = Session::new(target.id.clone(), control_path);
                Ok(SessionEntry {
                    target,
                    session,
                    effective_config: None,
                    forwards: BTreeMap::new(),
                })
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?;

        Ok(Self {
            ssh,
            runtime,
            entries,
            port_probe,
            port_attempts: DEFAULT_PORT_ATTEMPTS,
        })
    }

    /// Changes the bounded candidate count, mainly for deterministic tests.
    pub fn set_port_attempts(&mut self, attempts: usize) {
        self.port_attempts = attempts.max(1);
    }

    /// Returns all target/session entries in discovery order.
    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    /// Returns one target/session entry.
    pub fn entry(&self, target_id: &TargetId) -> Option<&SessionEntry> {
        self.entries
            .iter()
            .find(|entry| &entry.target.id == target_id)
    }

    /// Probes the configured OpenSSH executable with `ssh -V`.
    pub fn probe_openssh(&self) -> Result<String, ManagerError> {
        let output = self.ssh.version().map_err(|error| {
            ManagerError::Operation(failure_from_ssh_error(error, "OpenSSHを実行できません"))
        })?;
        if !output.success {
            return Err(ManagerError::Operation(Failure::new(
                FailureKind::OpenSshNotFound,
                "OpenSSHのバージョン確認に失敗しました",
                output.diagnostic().map(str::to_owned),
            )));
        }
        output.diagnostic().map(str::to_owned).ok_or_else(|| {
            ManagerError::Operation(Failure::new(
                FailureKind::OpenSshNotFound,
                "OpenSSHのバージョンを取得できませんでした",
                None,
            ))
        })
    }

    /// Resolves display fields for every target using OpenSSH itself.
    pub fn resolve_target_configs(&mut self) -> Vec<Failure> {
        let mut failures = Vec::new();
        for entry in &mut self.entries {
            let output = match self.ssh.resolve_config(&entry.target.host_alias) {
                Ok(output) => output,
                Err(error) => {
                    let failure = failure_from_ssh_error(error, "SSH設定を解決できません");
                    entry.session.last_error = Some(failure.clone());
                    failures.push(failure);
                    continue;
                }
            };
            if !output.success {
                let failure = Failure::new(
                    FailureKind::SshConfigurationFailed,
                    format!("{} のSSH設定を解決できません", entry.target.host_alias),
                    output.diagnostic().map(str::to_owned),
                );
                entry.session.last_error = Some(failure.clone());
                failures.push(failure);
                continue;
            }
            match parse_effective_config(&output.stdout) {
                Ok(config) => entry.effective_config = Some(config),
                Err(error) => {
                    let failure = Failure::new(
                        FailureKind::SshConfigurationFailed,
                        format!("{} のssh -G出力を解析できません", entry.target.host_alias),
                        Some(error.to_string()),
                    );
                    entry.session.last_error = Some(failure.clone());
                    failures.push(failure);
                }
            }
        }
        failures
    }

    /// Connects one target, confirming success with `-O check`.
    pub fn connect(&mut self, target_id: &TargetId) -> Result<(), ManagerError> {
        let trace = start_target_operation("connect", target_id);
        let result = self.connect_inner(target_id);
        finish_operation("connect", trace, &result);
        result
    }

    fn connect_inner(&mut self, target_id: &TargetId) -> Result<(), ManagerError> {
        let index = self.entry_index(target_id)?;
        let entry = &mut self.entries[index];
        if !matches!(
            entry.session.state,
            SessionState::Disconnected | SessionState::Failed
        ) {
            return Err(ManagerError::InvalidSessionState(entry.session.state));
        }
        entry.session.transition(SessionState::Connecting)?;

        let output = match self
            .ssh
            .connect(&entry.target.host_alias, &entry.session.control_path)
        {
            Ok(output) => output,
            Err(error) => {
                let failure = failure_from_ssh_error(error, "SSH接続を開始できません");
                entry.session.fail(failure.clone())?;
                return Err(ManagerError::Operation(failure));
            }
        };
        if !output.success {
            let failure = Failure::new(
                FailureKind::ConnectionFailed,
                "SSH接続に失敗しました",
                output.diagnostic().map(str::to_owned),
            );
            entry.session.fail(failure.clone())?;
            return Err(ManagerError::Operation(failure));
        }

        let check = match self
            .ssh
            .check(&entry.target.host_alias, &entry.session.control_path)
        {
            Ok(output) => output,
            Err(error) => {
                let failure = failure_from_ssh_error(error, "ControlMasterを確認できません");
                entry.session.fail(failure.clone())?;
                return Err(ManagerError::Operation(failure));
            }
        };
        if !check.success {
            let failure = Failure::new(
                FailureKind::ControlMasterUnavailable,
                "接続後のControlMaster確認に失敗しました",
                check.diagnostic().map(str::to_owned),
            );
            entry.session.fail(failure.clone())?;
            return Err(ManagerError::Operation(failure));
        }

        entry.session.transition(SessionState::Connected)?;
        reset_unavailable_forwards(entry)?;
        Ok(())
    }

    /// Refreshes one ControlMaster state using `-O check`.
    pub fn check(&mut self, target_id: &TargetId) -> Result<bool, ManagerError> {
        let trace = start_target_operation("check", target_id);
        let result = self.check_inner(target_id);
        finish_operation("check", trace, &result);
        result
    }

    fn check_inner(&mut self, target_id: &TargetId) -> Result<bool, ManagerError> {
        let index = self.entry_index(target_id)?;
        let entry = &mut self.entries[index];
        let output = match self
            .ssh
            .check(&entry.target.host_alias, &entry.session.control_path)
        {
            Ok(output) => output,
            Err(error) => {
                let failure = failure_from_ssh_error(error, "ControlMasterを確認できません");
                move_session_to_failed(&mut entry.session, failure.clone())?;
                mark_forwards_unavailable(entry)?;
                return Err(ManagerError::Operation(failure));
            }
        };

        if output.success {
            move_session_to_connected(&mut entry.session)?;
            reset_unavailable_forwards(entry)?;
            Ok(true)
        } else {
            move_session_to_disconnected(&mut entry.session)?;
            entry.session.last_error = Some(Failure::new(
                FailureKind::ControlMasterUnavailable,
                "ControlMasterは接続されていません",
                output.diagnostic().map(str::to_owned),
            ));
            mark_forwards_unavailable(entry)?;
            Ok(false)
        }
    }

    /// Stops one ControlMaster and deactivates all child forwards on success.
    pub fn disconnect(&mut self, target_id: &TargetId) -> Result<(), ManagerError> {
        let trace = start_target_operation("disconnect", target_id);
        let result = self.disconnect_inner(target_id);
        finish_operation("disconnect", trace, &result);
        result
    }

    fn disconnect_inner(&mut self, target_id: &TargetId) -> Result<(), ManagerError> {
        let index = self.entry_index(target_id)?;
        let entry = &mut self.entries[index];
        if entry.session.state == SessionState::Disconnected {
            deactivate_forwards(entry)?;
            return Ok(());
        }
        if !matches!(
            entry.session.state,
            SessionState::Connected | SessionState::Failed
        ) {
            return Err(ManagerError::InvalidSessionState(entry.session.state));
        }
        entry.session.transition(SessionState::Stopping)?;

        let output = match self
            .ssh
            .disconnect(&entry.target.host_alias, &entry.session.control_path)
        {
            Ok(output) => output,
            Err(error) => {
                let failure = failure_from_ssh_error(error, "SSHセッションを終了できません");
                entry.session.fail(failure.clone())?;
                return Err(ManagerError::Operation(failure));
            }
        };
        if !output.success {
            let failure = Failure::new(
                FailureKind::ControlMasterUnavailable,
                "SSHセッションの終了に失敗しました",
                output.diagnostic().map(str::to_owned),
            );
            entry.session.fail(failure.clone())?;
            return Err(ManagerError::Operation(failure));
        }

        entry.session.transition(SessionState::Disconnected)?;
        deactivate_forwards(entry)?;
        Ok(())
    }

    /// Adds a saved rule to its connected ControlMaster with bounded port fallback.
    pub fn activate_forward(&mut self, rule: &ForwardRule) -> Result<u16, ManagerError> {
        let trace = start_forward_operation("add_local_forward", rule);
        let result = self.activate_forward_inner(rule, trace.operation_id);
        finish_operation("add_local_forward", trace, &result);
        result
    }

    fn activate_forward_inner(
        &mut self,
        rule: &ForwardRule,
        operation_id: u64,
    ) -> Result<u16, ManagerError> {
        let index = self.entry_index(&rule.target_id)?;
        let entry = &mut self.entries[index];
        if entry.session.state != SessionState::Connected {
            return Err(ManagerError::InvalidSessionState(entry.session.state));
        }

        let runtime_forward = entry
            .forwards
            .entry(rule.id.clone())
            .or_insert_with(|| ActiveForward::new(rule.id.clone()));
        if runtime_forward.state == ForwardState::Active {
            return runtime_forward
                .actual_local_port
                .ok_or(ManagerError::MissingRuntimeForwardData);
        }
        runtime_forward.transition(ForwardState::Adding)?;

        let start_port = rule.requested_local_port.unwrap_or(rule.remote_port);
        let candidates = port_candidates(start_port, self.port_attempts);
        let mut last_conflict_detail = None;

        for candidate in candidates {
            tracing::debug!(
                component = "application",
                operation = "forward_port_candidate",
                operation_id,
                target_id = rule.target_id.as_str(),
                rule_id = rule.id.as_str(),
                candidate_port = candidate,
                "local forward port candidate selected"
            );
            match self.port_probe.is_available(&rule.bind_address, candidate) {
                Ok(false) => {
                    tracing::debug!(
                        component = "application",
                        operation = "forward_port_candidate",
                        operation_id,
                        target_id = rule.target_id.as_str(),
                        rule_id = rule.id.as_str(),
                        candidate_port = candidate,
                        available = false,
                        "local forward port candidate unavailable"
                    );
                    last_conflict_detail = Some(format!(
                        "ローカル側 {}:{candidate} は使用中です",
                        rule.bind_address
                    ));
                    continue;
                }
                Err(error) => {
                    let failure = Failure::new(
                        FailureKind::LocalOperationFailed,
                        "ローカル待受アドレスを確認できません",
                        Some(error.to_string()),
                    );
                    runtime_forward.fail(failure.clone())?;
                    return Err(ManagerError::Operation(failure));
                }
                Ok(true) => {}
            }

            let spec = match LocalForwardSpec::new(
                &rule.bind_address,
                candidate,
                &rule.remote_host,
                rule.remote_port,
            ) {
                Ok(spec) => spec,
                Err(error) => {
                    let failure = Failure::new(
                        FailureKind::LocalOperationFailed,
                        "転送ルールの入力が不正です",
                        Some(error.to_string()),
                    );
                    runtime_forward.fail(failure.clone())?;
                    return Err(ManagerError::Operation(failure));
                }
            };

            let output = match self.ssh.add_local_forward(
                &entry.target.host_alias,
                &entry.session.control_path,
                &spec,
            ) {
                Ok(output) => output,
                Err(error) => {
                    let failure = failure_from_ssh_error(error, "転送を追加できません");
                    runtime_forward.fail(failure.clone())?;
                    return Err(ManagerError::Operation(failure));
                }
            };
            if output.success {
                runtime_forward.activate(candidate, spec.as_argument())?;
                return Ok(candidate);
            }
            if output_indicates_controlmaster_unavailable(&output) {
                let failure = Failure::new(
                    FailureKind::ControlMasterUnavailable,
                    "転送追加中にControlMasterが切断されました",
                    output.diagnostic().map(str::to_owned),
                );
                runtime_forward.transition(ForwardState::Unavailable)?;
                move_session_to_disconnected(&mut entry.session)?;
                entry.session.last_error = Some(failure.clone());
                mark_forwards_unavailable(entry)?;
                return Err(ManagerError::Operation(failure));
            }
            if output_indicates_port_conflict(&output) {
                last_conflict_detail = output.diagnostic().map(str::to_owned);
                continue;
            }

            let failure = Failure::new(
                FailureKind::ForwardRejected,
                "SSHサーバーが転送を開始できませんでした",
                output.diagnostic().map(str::to_owned),
            );
            runtime_forward.fail(failure.clone())?;
            return Err(ManagerError::Operation(failure));
        }

        let failure = Failure::new(
            FailureKind::LocalPortConflict,
            format!(
                "利用可能なローカルポートを{}件以内で確保できません",
                self.port_attempts
            ),
            last_conflict_detail,
        );
        runtime_forward.fail(failure.clone())?;
        Err(ManagerError::Operation(failure))
    }

    /// Cancels an active forward using the exact normalized values used to add it.
    pub fn cancel_forward(&mut self, rule: &ForwardRule) -> Result<(), ManagerError> {
        let trace = start_forward_operation("cancel_local_forward", rule);
        let result = self.cancel_forward_inner(rule);
        finish_operation("cancel_local_forward", trace, &result);
        result
    }

    fn cancel_forward_inner(&mut self, rule: &ForwardRule) -> Result<(), ManagerError> {
        let index = self.entry_index(&rule.target_id)?;
        let entry = &mut self.entries[index];
        let runtime_forward = entry
            .forwards
            .get_mut(&rule.id)
            .ok_or_else(|| ManagerError::UnknownForward(rule.id.clone()))?;
        let can_cancel = runtime_forward.state == ForwardState::Active
            || (runtime_forward.state == ForwardState::Failed
                && runtime_forward.actual_local_port.is_some());
        if !can_cancel {
            return Err(ManagerError::InvalidForwardState(runtime_forward.state));
        }
        let actual_port = runtime_forward
            .actual_local_port
            .ok_or(ManagerError::MissingRuntimeForwardData)?;
        let spec = LocalForwardSpec::new(
            &rule.bind_address,
            actual_port,
            &rule.remote_host,
            rule.remote_port,
        )
        .map_err(|error| {
            ManagerError::Operation(Failure::new(
                FailureKind::LocalOperationFailed,
                "保存された転送指定を復元できません",
                Some(error.to_string()),
            ))
        })?;
        if runtime_forward.normalized_spec.as_deref() != Some(spec.as_argument().as_str()) {
            return Err(ManagerError::MissingRuntimeForwardData);
        }

        runtime_forward.transition(ForwardState::Removing)?;
        let output = match self.ssh.cancel_local_forward(
            &entry.target.host_alias,
            &entry.session.control_path,
            &spec,
        ) {
            Ok(output) => output,
            Err(error) => {
                let failure = failure_from_ssh_error(error, "転送を取消できません");
                runtime_forward.fail(failure.clone())?;
                return Err(ManagerError::Operation(failure));
            }
        };
        if !output.success {
            if output_indicates_controlmaster_unavailable(&output) {
                let failure = Failure::new(
                    FailureKind::ControlMasterUnavailable,
                    "転送取消中にControlMasterが切断されました",
                    output.diagnostic().map(str::to_owned),
                );
                runtime_forward.transition(ForwardState::Unavailable)?;
                move_session_to_disconnected(&mut entry.session)?;
                entry.session.last_error = Some(failure.clone());
                mark_forwards_unavailable(entry)?;
                return Err(ManagerError::Operation(failure));
            }
            let failure = Failure::new(
                FailureKind::CancelFailed,
                "転送の取消に失敗しました",
                output.diagnostic().map(str::to_owned),
            );
            runtime_forward.fail(failure.clone())?;
            return Err(ManagerError::Operation(failure));
        }

        runtime_forward.transition(ForwardState::Inactive)?;
        Ok(())
    }

    /// Checks and stops all known live sessions, returning every failure encountered.
    pub fn shutdown_all(&mut self) -> Vec<ManagerError> {
        let candidate_targets = self
            .entries
            .iter()
            .filter(|entry| entry.session.state != SessionState::Disconnected)
            .map(|entry| entry.target.id.clone())
            .collect::<Vec<_>>();
        let mut errors = Vec::new();
        for target_id in candidate_targets {
            match self.check(&target_id) {
                Ok(true) => {
                    if let Err(error) = self.disconnect(&target_id) {
                        errors.push(error);
                    }
                }
                Ok(false) => {}
                Err(error) => errors.push(error),
            }
        }
        errors
    }

    /// Recovers known runtime entries without deleting an unchecked live master.
    pub fn recover_previous_runtime(&mut self) -> Result<RecoveryReport, ManagerError> {
        let owned_paths = self.runtime.owned_control_paths()?;
        let mut report = RecoveryReport::default();

        for index in 0..self.entries.len() {
            let entry = &mut self.entries[index];
            if !owned_paths.contains(&entry.session.control_path) {
                continue;
            }

            let check = match self
                .ssh
                .check(&entry.target.host_alias, &entry.session.control_path)
            {
                Ok(output) => output,
                Err(error) => {
                    let failure =
                        failure_from_ssh_error(error, "残存ControlMasterを確認できません");
                    move_session_to_failed(&mut entry.session, failure.clone())?;
                    report.failures.push(failure);
                    continue;
                }
            };

            if check.success {
                move_session_to_connected(&mut entry.session)?;
                let exit = match self
                    .ssh
                    .disconnect(&entry.target.host_alias, &entry.session.control_path)
                {
                    Ok(output) => output,
                    Err(error) => {
                        let failure =
                            failure_from_ssh_error(error, "残存SSHセッションを終了できません");
                        move_session_to_failed(&mut entry.session, failure.clone())?;
                        report.failures.push(failure);
                        continue;
                    }
                };
                if exit.success {
                    move_session_to_disconnected(&mut entry.session)?;
                    report.terminated_targets.push(entry.target.id.clone());
                } else {
                    let failure = Failure::new(
                        FailureKind::ControlMasterUnavailable,
                        "残存SSHセッションの終了に失敗しました",
                        exit.diagnostic().map(str::to_owned),
                    );
                    move_session_to_failed(&mut entry.session, failure.clone())?;
                    report.failures.push(failure);
                }
            } else {
                move_session_to_disconnected(&mut entry.session)?;
                self.runtime
                    .remove_stale_control_path(&entry.session.control_path)?;
                report
                    .removed_stale_paths
                    .push(entry.session.control_path.clone());
            }
        }

        report.unknown_paths = owned_paths
            .into_iter()
            .filter(|path| {
                !self
                    .entries
                    .iter()
                    .any(|entry| entry.session.control_path == *path)
            })
            .collect();
        Ok(report)
    }

    /// Runtime directory used by this manager.
    pub fn runtime(&self) -> &RuntimeDirectory {
        &self.runtime
    }

    fn entry_index(&self, target_id: &TargetId) -> Result<usize, ManagerError> {
        self.entries
            .iter()
            .position(|entry| &entry.target.id == target_id)
            .ok_or_else(|| ManagerError::UnknownTarget(target_id.clone()))
    }
}

/// Session or forward coordination failure.
#[derive(Debug, Error)]
pub enum ManagerError {
    /// Runtime directory setup failed.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    /// A domain transition was rejected.
    #[error(transparent)]
    Transition(#[from] TransitionError),
    /// A target identifier was not in the catalog.
    #[error("unknown SSH target: {}", .0.as_str())]
    UnknownTarget(TargetId),
    /// A rule has no runtime state.
    #[error("unknown forward rule: {}", .0.as_str())]
    UnknownForward(ForwardRuleId),
    /// An action was incompatible with the current session state.
    #[error("action is unavailable while session is {0:?}")]
    InvalidSessionState(SessionState),
    /// An action was incompatible with the current forward state.
    #[error("action is unavailable while forward is {0:?}")]
    InvalidForwardState(ForwardState),
    /// Runtime state lacked the exact data needed for safe cancellation.
    #[error("active forward is missing its exact runtime specification")]
    MissingRuntimeForwardData,
    /// User-facing operation failure.
    #[error("{}", .0.summary)]
    Operation(Failure),
}

struct OperationTrace {
    operation_id: u64,
    started: Instant,
}

fn start_target_operation(operation: &'static str, target_id: &TargetId) -> OperationTrace {
    let operation_id = crate::logging::next_operation_id();
    tracing::debug!(
        component = "application",
        operation,
        operation_id,
        target_id = target_id.as_str(),
        "application operation started"
    );
    OperationTrace {
        operation_id,
        started: Instant::now(),
    }
}

fn start_forward_operation(operation: &'static str, rule: &ForwardRule) -> OperationTrace {
    let operation_id = crate::logging::next_operation_id();
    tracing::debug!(
        component = "application",
        operation,
        operation_id,
        target_id = rule.target_id.as_str(),
        rule_id = rule.id.as_str(),
        "application operation started"
    );
    OperationTrace {
        operation_id,
        started: Instant::now(),
    }
}

fn finish_operation<T>(
    operation: &'static str,
    trace: OperationTrace,
    result: &Result<T, ManagerError>,
) {
    tracing::debug!(
        component = "application",
        operation,
        operation_id = trace.operation_id,
        elapsed_ms = trace.started.elapsed().as_millis(),
        success = result.is_ok(),
        error_kind = result.as_ref().err().map(manager_error_kind),
        "application operation completed"
    );
}

fn manager_error_kind(error: &ManagerError) -> &'static str {
    match error {
        ManagerError::Runtime(_) => "runtime",
        ManagerError::Transition(_) => "transition",
        ManagerError::UnknownTarget(_) => "unknown_target",
        ManagerError::UnknownForward(_) => "unknown_forward",
        ManagerError::InvalidSessionState(_) => "invalid_session_state",
        ManagerError::InvalidForwardState(_) => "invalid_forward_state",
        ManagerError::MissingRuntimeForwardData => "missing_runtime_forward_data",
        ManagerError::Operation(failure) => match failure.kind {
            FailureKind::OpenSshNotFound => "openssh_not_found",
            FailureKind::ConnectionFailed => "connection_failed",
            FailureKind::ControlMasterUnavailable => "controlmaster_unavailable",
            FailureKind::LocalPortConflict => "local_port_conflict",
            FailureKind::ForwardRejected => "forward_rejected",
            FailureKind::CancelFailed => "cancel_failed",
            FailureKind::SshConfigurationFailed => "ssh_configuration_failed",
            FailureKind::LocalOperationFailed => "local_operation_failed",
        },
    }
}

/// Returns at most `limit` sequential non-zero port candidates.
pub fn port_candidates(start: u16, limit: usize) -> Vec<u16> {
    (start..=u16::MAX).take(limit).collect()
}

fn failure_from_ssh_error(error: SshError, summary: &str) -> Failure {
    let kind = match &error {
        SshError::Execute { source, .. } if source.kind() == io::ErrorKind::NotFound => {
            FailureKind::OpenSshNotFound
        }
        SshError::InvalidInput(_) => FailureKind::SshConfigurationFailed,
        SshError::Execute { .. } => FailureKind::LocalOperationFailed,
    };
    Failure::new(kind, summary, Some(error.to_string()))
}

fn output_indicates_port_conflict(output: &SshOutput) -> bool {
    let diagnostic = output.diagnostic().unwrap_or_default().to_ascii_lowercase();
    [
        "address already in use",
        "cannot listen to port",
        "could not request local forwarding",
        "port forwarding failed",
    ]
    .iter()
    .any(|pattern| diagnostic.contains(pattern))
}

fn output_indicates_controlmaster_unavailable(output: &SshOutput) -> bool {
    let diagnostic = output.diagnostic().unwrap_or_default().to_ascii_lowercase();
    [
        "control socket connect",
        "master is not running",
        "no such file or directory",
        "connection refused",
    ]
    .iter()
    .any(|pattern| diagnostic.contains(pattern))
}

fn move_session_to_connected(session: &mut Session) -> Result<(), TransitionError> {
    match session.state {
        SessionState::Connected => Ok(()),
        SessionState::Disconnected | SessionState::Failed => {
            session.transition(SessionState::Connecting)?;
            session.transition(SessionState::Connected)
        }
        SessionState::Connecting => session.transition(SessionState::Connected),
        SessionState::Stopping => Err(TransitionError::Session {
            from: SessionState::Stopping,
            to: SessionState::Connected,
        }),
    }
}

fn move_session_to_disconnected(session: &mut Session) -> Result<(), TransitionError> {
    match session.state {
        SessionState::Disconnected => Ok(()),
        SessionState::Connecting | SessionState::Connected | SessionState::Stopping => {
            session.transition(SessionState::Disconnected)
        }
        SessionState::Failed => session.transition(SessionState::Disconnected),
    }
}

fn move_session_to_failed(session: &mut Session, failure: Failure) -> Result<(), TransitionError> {
    if session.state == SessionState::Failed {
        session.last_error = Some(failure);
        Ok(())
    } else {
        session.fail(failure)
    }
}

fn mark_forwards_unavailable(entry: &mut SessionEntry) -> Result<(), TransitionError> {
    for forward in entry.forwards.values_mut() {
        if !matches!(
            forward.state,
            ForwardState::Inactive | ForwardState::Unavailable
        ) {
            forward.transition(ForwardState::Unavailable)?;
        }
    }
    Ok(())
}

fn reset_unavailable_forwards(entry: &mut SessionEntry) -> Result<(), TransitionError> {
    for forward in entry.forwards.values_mut() {
        if forward.state == ForwardState::Unavailable {
            forward.transition(ForwardState::Inactive)?;
        }
    }
    Ok(())
}

fn deactivate_forwards(entry: &mut SessionEntry) -> Result<(), TransitionError> {
    for forward in entry.forwards.values_mut() {
        match forward.state {
            ForwardState::Inactive => {}
            ForwardState::Active => {
                forward.transition(ForwardState::Removing)?;
                forward.transition(ForwardState::Inactive)?;
            }
            ForwardState::Unavailable | ForwardState::Failed | ForwardState::Adding => {
                forward.transition(ForwardState::Inactive)?;
            }
            ForwardState::Removing => {
                forward.transition(ForwardState::Inactive)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::fs;
    use std::io;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::config::RuleStore;
    use crate::domain::{
        FailureKind, ForwardRule, ForwardRuleId, ForwardState, SessionState, Target, TargetId,
    };
    use crate::runtime::RuntimeDirectory;
    use crate::ssh::{LocalForwardSpec, SshClient, SshError, SshOutput};

    use super::{
        AppActionError, AppState, ForwardRuleDraft, PortProbe, RuleError, SessionManager,
        port_candidates,
    };

    #[derive(Clone)]
    struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for CaptureWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn capture_debug_events(action: impl FnOnce()) -> String {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::clone(&captured);
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(move || CaptureWriter(Arc::clone(&writer)))
            .finish();
        tracing::subscriber::with_default(subscriber, action);
        let contents = captured.lock().unwrap().clone();
        String::from_utf8(contents).unwrap()
    }

    #[derive(Debug, Default)]
    struct FakeSsh {
        connect: RefCell<VecDeque<SshOutput>>,
        check: RefCell<VecDeque<SshOutput>>,
        forward: RefCell<VecDeque<SshOutput>>,
        cancel: RefCell<VecDeque<SshOutput>>,
        disconnect: RefCell<VecDeque<SshOutput>>,
        forwarded_specs: RefCell<Vec<String>>,
        canceled_specs: RefCell<Vec<String>>,
    }

    impl FakeSsh {
        fn successful() -> Self {
            let mut fake = Self::default();
            fake.connect.get_mut().push_back(success());
            fake.check.get_mut().push_back(success());
            fake.disconnect.get_mut().push_back(success());
            fake
        }
    }

    impl SshClient for FakeSsh {
        fn version(&self) -> Result<SshOutput, SshError> {
            Ok(SshOutput {
                stderr: "OpenSSH_9.6p1".to_owned(),
                ..success()
            })
        }

        fn resolve_config(&self, _host_alias: &str) -> Result<SshOutput, SshError> {
            Ok(SshOutput {
                stdout: "host dev\nhostname dev.example.test\nuser alice\nport 2222\nproxyjump bastion\n"
                    .to_owned(),
                ..success()
            })
        }

        fn connect(&self, _host_alias: &str, _control_path: &Path) -> Result<SshOutput, SshError> {
            Ok(self
                .connect
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(success))
        }

        fn check(&self, _host_alias: &str, _control_path: &Path) -> Result<SshOutput, SshError> {
            Ok(self.check.borrow_mut().pop_front().unwrap_or_else(success))
        }

        fn add_local_forward(
            &self,
            _host_alias: &str,
            _control_path: &Path,
            forward: &LocalForwardSpec,
        ) -> Result<SshOutput, SshError> {
            self.forwarded_specs
                .borrow_mut()
                .push(forward.as_argument());
            Ok(self
                .forward
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(success))
        }

        fn cancel_local_forward(
            &self,
            _host_alias: &str,
            _control_path: &Path,
            forward: &LocalForwardSpec,
        ) -> Result<SshOutput, SshError> {
            self.canceled_specs.borrow_mut().push(forward.as_argument());
            Ok(self.cancel.borrow_mut().pop_front().unwrap_or_else(success))
        }

        fn disconnect(
            &self,
            _host_alias: &str,
            _control_path: &Path,
        ) -> Result<SshOutput, SshError> {
            Ok(self
                .disconnect
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(success))
        }
    }

    #[derive(Debug, Default)]
    struct FakePortProbe {
        availability: RefCell<VecDeque<bool>>,
    }

    impl PortProbe for FakePortProbe {
        fn is_available(&self, _bind_address: &str, _port: u16) -> io::Result<bool> {
            Ok(self.availability.borrow_mut().pop_front().unwrap_or(true))
        }
    }

    struct TestRuntime(PathBuf);

    impl TestRuntime {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "portdeck-manager-test-{}-{unique}",
                std::process::id()
            ));
            let runtime = RuntimeDirectory::prepare(&path).unwrap();
            drop(runtime);
            Self(path)
        }

        fn runtime(&self) -> RuntimeDirectory {
            RuntimeDirectory::prepare(&self.0).unwrap()
        }
    }

    impl Drop for TestRuntime {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
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

    fn failure(stderr: &str) -> SshOutput {
        SshOutput {
            success: false,
            exit_code: Some(255),
            stdout: String::new(),
            stderr: stderr.to_owned(),
        }
    }

    fn target() -> Target {
        Target {
            id: TargetId::new("dev"),
            host_alias: "dev".to_owned(),
            source: PathBuf::from("config"),
        }
    }

    fn rule() -> ForwardRule {
        ForwardRule {
            id: ForwardRuleId::new("web"),
            target_id: TargetId::new("dev"),
            label: Some("web".to_owned()),
            bind_address: "127.0.0.1".to_owned(),
            requested_local_port: Some(8080),
            remote_host: "127.0.0.1".to_owned(),
            remote_port: 3000,
        }
    }

    #[test]
    fn connects_only_after_controlmaster_check_succeeds() {
        let test_runtime = TestRuntime::new();
        let mut manager = SessionManager::with_port_probe(
            FakeSsh::successful(),
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();

        manager.connect(&TargetId::new("dev")).unwrap();

        assert_eq!(
            manager.entry(&TargetId::new("dev")).unwrap().session.state,
            SessionState::Connected
        );
    }

    #[test]
    fn effective_target_values_are_resolved_by_openssh_output() {
        let test_runtime = TestRuntime::new();
        let mut manager = SessionManager::with_port_probe(
            FakeSsh::successful(),
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();

        assert!(manager.resolve_target_configs().is_empty());

        let config = manager.entries()[0].effective_config.as_ref().unwrap();
        assert_eq!(config.hostname, "dev.example.test");
        assert_eq!(config.user, "alice");
        assert_eq!(config.port, 2222);
        assert_eq!(config.proxy_jump.as_deref(), Some("bastion"));
    }

    #[test]
    fn failed_post_connect_check_does_not_report_connected() {
        let test_runtime = TestRuntime::new();
        let mut ssh = FakeSsh::successful();
        ssh.check.get_mut().clear();
        ssh.check.get_mut().push_back(failure("no master"));
        let mut manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();

        let error = manager.connect(&TargetId::new("dev")).unwrap_err();

        assert_eq!(
            manager.entry(&TargetId::new("dev")).unwrap().session.state,
            SessionState::Failed
        );
        assert!(error.to_string().contains("ControlMaster"));
    }

    #[test]
    fn shutdown_does_not_fail_when_a_failed_connection_has_no_master() {
        let test_runtime = TestRuntime::new();
        let mut ssh = FakeSsh::successful();
        ssh.connect.get_mut().clear();
        ssh.connect
            .get_mut()
            .push_back(failure("Permission denied"));
        ssh.check.get_mut().clear();
        ssh.check
            .get_mut()
            .push_back(failure("master is not running"));
        let mut manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        manager.connect(&TargetId::new("dev")).unwrap_err();

        let errors = manager.shutdown_all();

        assert!(errors.is_empty());
        assert_eq!(
            manager.entry(&TargetId::new("dev")).unwrap().session.state,
            SessionState::Disconnected
        );
        assert_eq!(manager.ssh.disconnect.borrow().len(), 1);
    }

    #[test]
    fn chooses_next_candidate_after_preflight_conflict() {
        let test_runtime = TestRuntime::new();
        let mut ssh = FakeSsh::successful();
        ssh.forward.get_mut().push_back(success());
        let probe = FakePortProbe {
            availability: RefCell::new(VecDeque::from([false, true])),
        };
        let mut manager =
            SessionManager::with_port_probe(ssh, test_runtime.runtime(), vec![target()], probe)
                .unwrap();
        manager.connect(&TargetId::new("dev")).unwrap();

        let actual_port = manager.activate_forward(&rule()).unwrap();

        assert_eq!(actual_port, 8081);
        let forward =
            &manager.entry(&TargetId::new("dev")).unwrap().forwards[&ForwardRuleId::new("web")];
        assert_eq!(forward.state, ForwardState::Active);
        assert_eq!(forward.actual_local_port, Some(8081));
    }

    #[test]
    fn retries_when_openssh_reports_a_bind_race() {
        let test_runtime = TestRuntime::new();
        let mut ssh = FakeSsh::successful();
        ssh.forward
            .get_mut()
            .extend([failure("Address already in use"), success()]);
        let mut manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        manager.connect(&TargetId::new("dev")).unwrap();

        assert_eq!(manager.activate_forward(&rule()).unwrap(), 8081);
    }

    #[test]
    fn forward_rejection_never_becomes_active() {
        let test_runtime = TestRuntime::new();
        let mut ssh = FakeSsh::successful();
        ssh.forward
            .get_mut()
            .push_back(failure("administratively prohibited"));
        let mut manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        manager.connect(&TargetId::new("dev")).unwrap();

        manager.activate_forward(&rule()).unwrap_err();

        let forward =
            &manager.entry(&TargetId::new("dev")).unwrap().forwards[&ForwardRuleId::new("web")];
        assert_eq!(forward.state, ForwardState::Failed);
        assert_eq!(
            forward.last_error.as_ref().unwrap().kind,
            FailureKind::ForwardRejected
        );
    }

    #[test]
    fn lost_controlmaster_during_add_updates_session_and_forward_together() {
        let test_runtime = TestRuntime::new();
        let mut ssh = FakeSsh::successful();
        ssh.forward
            .get_mut()
            .push_back(failure("Control socket connect: No such file or directory"));
        let mut manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        manager.connect(&TargetId::new("dev")).unwrap();

        manager.activate_forward(&rule()).unwrap_err();

        let entry = manager.entry(&TargetId::new("dev")).unwrap();
        assert_eq!(entry.session.state, SessionState::Disconnected);
        assert_eq!(
            entry.forwards[&ForwardRuleId::new("web")].state,
            ForwardState::Unavailable
        );
    }

    #[test]
    fn cancel_failure_keeps_exact_runtime_values_visible() {
        let test_runtime = TestRuntime::new();
        let mut ssh = FakeSsh::successful();
        ssh.forward.get_mut().push_back(success());
        ssh.cancel.get_mut().push_back(failure("cancel failed"));
        let mut manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        manager.connect(&TargetId::new("dev")).unwrap();
        manager.activate_forward(&rule()).unwrap();

        manager.cancel_forward(&rule()).unwrap_err();

        let forward =
            &manager.entry(&TargetId::new("dev")).unwrap().forwards[&ForwardRuleId::new("web")];
        assert_eq!(forward.state, ForwardState::Failed);
        assert_eq!(forward.actual_local_port, Some(8080));
        assert!(forward.normalized_spec.is_some());
    }

    #[test]
    fn failed_cancel_can_be_retried_with_the_same_specification() {
        let test_runtime = TestRuntime::new();
        let mut ssh = FakeSsh::successful();
        ssh.forward.get_mut().push_back(success());
        ssh.cancel
            .get_mut()
            .extend([failure("cancel failed"), success()]);
        let mut manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        manager.connect(&TargetId::new("dev")).unwrap();
        manager.activate_forward(&rule()).unwrap();
        manager.cancel_forward(&rule()).unwrap_err();

        manager.cancel_forward(&rule()).unwrap();

        assert_eq!(
            manager.entry(&TargetId::new("dev")).unwrap().forwards[&ForwardRuleId::new("web")]
                .state,
            ForwardState::Inactive
        );
    }

    #[test]
    fn lost_controlmaster_during_cancel_marks_all_runtime_state_unavailable() {
        let test_runtime = TestRuntime::new();
        let mut ssh = FakeSsh::successful();
        ssh.forward.get_mut().push_back(success());
        ssh.cancel
            .get_mut()
            .push_back(failure("master is not running"));
        let mut manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        manager.connect(&TargetId::new("dev")).unwrap();
        manager.activate_forward(&rule()).unwrap();

        manager.cancel_forward(&rule()).unwrap_err();

        let entry = manager.entry(&TargetId::new("dev")).unwrap();
        assert_eq!(entry.session.state, SessionState::Disconnected);
        assert_eq!(
            entry.forwards[&ForwardRuleId::new("web")].state,
            ForwardState::Unavailable
        );
    }

    #[test]
    fn disconnect_deactivates_child_forwards() {
        let test_runtime = TestRuntime::new();
        let mut ssh = FakeSsh::successful();
        ssh.forward.get_mut().push_back(success());
        let mut manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        manager.connect(&TargetId::new("dev")).unwrap();
        manager.activate_forward(&rule()).unwrap();

        manager.disconnect(&TargetId::new("dev")).unwrap();

        let entry = manager.entry(&TargetId::new("dev")).unwrap();
        assert_eq!(entry.session.state, SessionState::Disconnected);
        assert_eq!(
            entry.forwards[&ForwardRuleId::new("web")].state,
            ForwardState::Inactive
        );
    }

    #[test]
    fn recovery_checks_before_removing_a_stale_socket() {
        let test_runtime = TestRuntime::new();
        let mut ssh = FakeSsh::default();
        ssh.check.get_mut().push_back(failure("no master"));
        let mut manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        let control_path = manager.entries()[0].session.control_path.clone();
        fs::write(&control_path, "stale").unwrap();

        let report = manager.recover_previous_runtime().unwrap();

        assert_eq!(report.removed_stale_paths.len(), 1);
        assert_eq!(report.removed_stale_paths[0], control_path);
        assert!(!control_path.exists());
    }

    #[test]
    fn recovery_stops_a_live_known_master_and_keeps_unknown_paths() {
        let test_runtime = TestRuntime::new();
        let mut ssh = FakeSsh::default();
        ssh.check.get_mut().push_back(success());
        ssh.disconnect.get_mut().push_back(success());
        let mut manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        let known = manager.entries()[0].session.control_path.clone();
        let unknown = manager.runtime().path().join("cm-unknown");
        fs::write(&known, "live placeholder").unwrap();
        fs::write(&unknown, "unknown placeholder").unwrap();

        let report = manager.recover_previous_runtime().unwrap();

        assert_eq!(report.terminated_targets, [TargetId::new("dev")]);
        assert_eq!(report.unknown_paths.len(), 1);
        assert_eq!(report.unknown_paths[0], unknown);
        assert!(unknown.exists());
        assert!(
            known.exists(),
            "the fake ssh does not remove its placeholder"
        );
    }

    #[test]
    fn candidate_selection_is_bounded_and_never_wraps() {
        assert_eq!(port_candidates(65000, 3), [65000, 65001, 65002]);
        assert_eq!(port_candidates(u16::MAX - 1, 20), [u16::MAX - 1, u16::MAX]);
        assert!(port_candidates(1234, 0).is_empty());
    }

    #[test]
    fn forward_form_defaults_to_loopback_and_remote_port() {
        let draft = ForwardRuleDraft::parse(" web ", "", "", "", "3000").unwrap();

        assert_eq!(draft.label.as_deref(), Some("web"));
        assert_eq!(draft.bind_address, "127.0.0.1");
        assert_eq!(draft.requested_local_port, None);
        assert_eq!(draft.remote_host, "127.0.0.1");
        assert_eq!(draft.remote_port, 3000);
        assert!(!draft.public_bind);
    }

    #[test]
    fn forward_form_marks_public_binds_and_rejects_zero_port() {
        let draft = ForwardRuleDraft::parse("", "0.0.0.0", "8080", "db", "5432").unwrap();
        assert!(draft.public_bind);
        assert_eq!(
            ForwardRuleDraft::parse("", "", "", "", "0").unwrap_err(),
            RuleError::InvalidPort {
                field: "リモート宛先ポート"
            }
        );
    }

    #[test]
    fn saved_definition_remains_separate_from_runtime_state() {
        let test_runtime = TestRuntime::new();
        let ssh = FakeSsh::successful();
        let manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        let mut app = AppState::new(manager, Vec::new()).unwrap();
        let draft = ForwardRuleDraft::parse("web", "", "", "", "3000").unwrap();

        let rule_id = app.add_rule(&TargetId::new("dev"), draft).unwrap();

        assert_eq!(app.rules_for(&TargetId::new("dev")).len(), 1);
        assert!(
            !app.sessions()
                .entry(&TargetId::new("dev"))
                .unwrap()
                .forwards
                .contains_key(&rule_id)
        );
    }

    #[test]
    fn inactive_rule_update_preserves_identity_target_and_order() {
        let test_runtime = TestRuntime::new();
        let manager = SessionManager::with_port_probe(
            FakeSsh::successful(),
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        let original_id = ForwardRuleId::new("web");
        let original = rule();
        let mut second = rule();
        second.id = ForwardRuleId::new("database");
        let mut app = AppState::new(manager, vec![original, second.clone()]).unwrap();

        app.update_rule(
            &original_id,
            ForwardRuleDraft::parse("edited web", "::1", "18080", "web.internal", "4000").unwrap(),
        )
        .unwrap();

        let rules = app.rules_for(&TargetId::new("dev"));
        assert_eq!(rules[0].id, original_id);
        assert_eq!(rules[0].target_id, TargetId::new("dev"));
        assert_eq!(rules[0].label.as_deref(), Some("edited web"));
        assert_eq!(rules[0].bind_address, "::1");
        assert_eq!(rules[0].requested_local_port, Some(18080));
        assert_eq!(rules[0].remote_host, "web.internal");
        assert_eq!(rules[0].remote_port, 4000);
        assert_eq!(rules[1], second);
    }

    #[test]
    fn runtime_state_or_forward_data_blocks_rule_editing() {
        for state in [
            ForwardState::Adding,
            ForwardState::Active,
            ForwardState::Removing,
            ForwardState::Failed,
            ForwardState::Unavailable,
        ] {
            let test_runtime = TestRuntime::new();
            let mut manager = SessionManager::with_port_probe(
                FakeSsh::successful(),
                test_runtime.runtime(),
                vec![target()],
                FakePortProbe::default(),
            )
            .unwrap();
            manager
                .entries
                .iter_mut()
                .find(|entry| entry.target.id == TargetId::new("dev"))
                .unwrap()
                .forwards
                .insert(
                    ForwardRuleId::new("web"),
                    crate::domain::ActiveForward {
                        rule_id: ForwardRuleId::new("web"),
                        actual_local_port: None,
                        normalized_spec: None,
                        state,
                        last_error: None,
                    },
                );
            let mut app = AppState::new(manager, vec![rule()]).unwrap();

            let error = app
                .update_rule(
                    &ForwardRuleId::new("web"),
                    ForwardRuleDraft::parse("edited", "", "", "", "4000").unwrap(),
                )
                .unwrap_err();

            assert!(matches!(
                error,
                AppActionError::Rule(RuleError::RuleNotEditable(_))
            ));
            assert_eq!(
                app.rules_for(&TargetId::new("dev"))[0].label.as_deref(),
                Some("web")
            );
        }

        let test_runtime = TestRuntime::new();
        let mut manager = SessionManager::with_port_probe(
            FakeSsh::successful(),
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        manager
            .entries
            .iter_mut()
            .find(|entry| entry.target.id == TargetId::new("dev"))
            .unwrap()
            .forwards
            .insert(
                ForwardRuleId::new("web"),
                crate::domain::ActiveForward {
                    rule_id: ForwardRuleId::new("web"),
                    actual_local_port: Some(8080),
                    normalized_spec: Some("127.0.0.1:8080:127.0.0.1:3000".to_owned()),
                    state: ForwardState::Inactive,
                    last_error: None,
                },
            );
        let app = AppState::new(manager, vec![rule()]).unwrap();
        assert!(matches!(
            app.editable_rule(&ForwardRuleId::new("web")),
            Err(AppActionError::Rule(RuleError::RuleNotEditable(_)))
        ));
    }

    #[test]
    fn activation_after_edit_uses_the_new_forward_specification() {
        let test_runtime = TestRuntime::new();
        let manager = SessionManager::with_port_probe(
            FakeSsh::successful(),
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        let mut app = AppState::new(manager, vec![rule()]).unwrap();
        let id = ForwardRuleId::new("web");
        app.update_rule(
            &id,
            ForwardRuleDraft::parse("edited", "::1", "18080", "db.internal", "5432").unwrap(),
        )
        .unwrap();
        app.sessions_mut().connect(&TargetId::new("dev")).unwrap();

        assert_eq!(app.activate_rule(&id).unwrap(), 18080);
        assert_eq!(
            app.sessions().ssh.forwarded_specs.borrow().as_slice(),
            ["[::1]:18080:db.internal:5432"]
        );
    }

    #[test]
    fn active_definition_cannot_be_deleted_until_cancel_succeeds() {
        let test_runtime = TestRuntime::new();
        let ssh = FakeSsh::successful();
        let manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        let mut app = AppState::new(manager, Vec::new()).unwrap();
        let rule_id = app
            .add_rule(
                &TargetId::new("dev"),
                ForwardRuleDraft::parse("web", "", "8080", "", "3000").unwrap(),
            )
            .unwrap();
        app.sessions_mut().connect(&TargetId::new("dev")).unwrap();
        app.activate_rule(&rule_id).unwrap();

        assert!(app.remove_rule(&rule_id).is_err());
        app.cancel_rule(&rule_id).unwrap();
        app.remove_rule(&rule_id).unwrap();
        assert!(app.rules_for(&TargetId::new("dev")).is_empty());
    }

    #[test]
    fn definition_changes_are_persisted_immediately_when_store_is_attached() {
        let test_runtime = TestRuntime::new();
        let config_path = test_runtime.0.join("config/portdeck.toml");
        let ssh = FakeSsh::successful();
        let manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        let mut app =
            AppState::with_store(manager, Vec::new(), RuleStore::at(&config_path)).unwrap();

        let id = app
            .add_rule(
                &TargetId::new("dev"),
                ForwardRuleDraft::parse("web", "", "", "", "3000").unwrap(),
            )
            .unwrap();
        let mut reloaded_store = RuleStore::at(&config_path);
        assert_eq!(reloaded_store.load(&[target()]).unwrap().len(), 1);

        app.remove_rule(&id).unwrap();
        assert!(reloaded_store.load(&[target()]).unwrap().is_empty());
    }

    #[test]
    fn edited_definition_round_trips_through_attached_store() {
        let test_runtime = TestRuntime::new();
        let config_path = test_runtime.0.join("config/portdeck.toml");
        let manager = SessionManager::with_port_probe(
            FakeSsh::successful(),
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        let mut app =
            AppState::with_store(manager, vec![rule()], RuleStore::at(&config_path)).unwrap();

        app.update_rule(
            &ForwardRuleId::new("web"),
            ForwardRuleDraft::parse("database", "::1", "15432", "db.internal", "5432").unwrap(),
        )
        .unwrap();

        let mut reloaded_store = RuleStore::at(&config_path);
        assert_eq!(
            reloaded_store.load(&[target()]).unwrap(),
            app.rules_for(&TargetId::new("dev"))
        );
    }

    #[test]
    fn failed_persistence_rolls_back_definition_mutation() {
        let test_runtime = TestRuntime::new();
        let blocked_parent = test_runtime.0.join("not-a-directory");
        fs::write(&blocked_parent, "block directory creation").unwrap();
        let ssh = FakeSsh::successful();
        let manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        let mut app = AppState::with_store(
            manager,
            Vec::new(),
            RuleStore::at(blocked_parent.join("config.toml")),
        )
        .unwrap();

        let events = capture_debug_events(|| {
            assert!(
                app.add_rule(
                    &TargetId::new("dev"),
                    ForwardRuleDraft::parse("web", "", "", "", "3000").unwrap(),
                )
                .is_err()
            );
        });
        assert!(app.rules_for(&TargetId::new("dev")).is_empty());
        assert!(events.contains("operation=\"save\""));
        assert!(events.contains("operation=\"rollback_add_rule\""));
        assert!(events.contains("operation_id"));
    }

    #[test]
    fn debug_events_correlate_connect_forward_and_shutdown_without_raw_output() {
        let test_runtime = TestRuntime::new();
        let mut ssh = FakeSsh::successful();
        ssh.forward.get_mut().push_back(success());
        let mut manager = SessionManager::with_port_probe(
            ssh,
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();

        let events = capture_debug_events(|| {
            manager.connect(&TargetId::new("dev")).unwrap();
            manager.activate_forward(&rule()).unwrap();
            assert!(manager.shutdown_all().is_empty());
        });

        assert!(events.contains("operation=\"connect\""));
        assert!(events.contains("operation=\"add_local_forward\""));
        assert!(events.contains("operation=\"disconnect\""));
        assert!(events.contains("operation_id"));
        assert!(events.contains("elapsed_ms"));
        assert!(!events.contains("OpenSSH_9.6p1"));
        assert!(!events.contains("Permission denied"));
    }

    #[test]
    fn failed_persistence_rolls_back_rule_update_in_place() {
        let test_runtime = TestRuntime::new();
        let blocked_parent = test_runtime.0.join("not-a-directory");
        fs::write(&blocked_parent, "block directory creation").unwrap();
        let manager = SessionManager::with_port_probe(
            FakeSsh::successful(),
            test_runtime.runtime(),
            vec![target()],
            FakePortProbe::default(),
        )
        .unwrap();
        let original = rule();
        let mut app = AppState::with_store(
            manager,
            vec![original.clone()],
            RuleStore::at(blocked_parent.join("config.toml")),
        )
        .unwrap();

        assert!(
            app.update_rule(
                &ForwardRuleId::new("web"),
                ForwardRuleDraft::parse("edited", "", "", "", "4000").unwrap(),
            )
            .is_err()
        );
        assert_eq!(app.rules_for(&TargetId::new("dev")), [original]);
    }
}
