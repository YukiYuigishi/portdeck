//! Application services coordinating domain state and OpenSSH operations.

use std::collections::BTreeMap;
use std::io;
use std::net::TcpListener;

use thiserror::Error;

use crate::domain::{
    ActiveForward, Failure, FailureKind, ForwardRule, ForwardRuleId, ForwardState, Session,
    SessionState, Target, TargetId, TransitionError,
};
use crate::runtime::{RuntimeDirectory, RuntimeError};
use crate::ssh::{LocalForwardSpec, SshClient, SshError, SshOutput};

/// Maximum number of sequential ports considered for one add operation.
pub const DEFAULT_PORT_ATTEMPTS: usize = 20;

/// A target together with its session and runtime forwards.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    /// Discovered SSH target.
    pub target: Target,
    /// Portdeck-owned ControlMaster state.
    pub session: Session,
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

    /// Connects one target, confirming success with `-O check`.
    pub fn connect(&mut self, target_id: &TargetId) -> Result<(), ManagerError> {
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
            match self.port_probe.is_available(&rule.bind_address, candidate) {
                Ok(false) => {
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
        let index = self.entry_index(&rule.target_id)?;
        let entry = &mut self.entries[index];
        let runtime_forward = entry
            .forwards
            .get_mut(&rule.id)
            .ok_or_else(|| ManagerError::UnknownForward(rule.id.clone()))?;
        if runtime_forward.state != ForwardState::Active {
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

    /// Stops all known live sessions, returning every failure encountered.
    pub fn shutdown_all(&mut self) -> Vec<ManagerError> {
        let connected_targets = self
            .entries
            .iter()
            .filter(|entry| entry.session.state != SessionState::Disconnected)
            .map(|entry| entry.target.id.clone())
            .collect::<Vec<_>>();
        connected_targets
            .into_iter()
            .filter_map(|target_id| self.disconnect(&target_id).err())
            .collect()
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
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::domain::{
        FailureKind, ForwardRule, ForwardRuleId, ForwardState, SessionState, Target, TargetId,
    };
    use crate::runtime::RuntimeDirectory;
    use crate::ssh::{LocalForwardSpec, SshClient, SshError, SshOutput};

    use super::{PortProbe, SessionManager, port_candidates};

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
            Ok(success())
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
}
