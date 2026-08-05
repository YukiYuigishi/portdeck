//! Domain types and state transitions.
//!
//! This module does not depend on terminal rendering or process execution.

use std::path::PathBuf;

use thiserror::Error;

/// Stable identifier of an SSH target.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TargetId(String);

impl TargetId {
    /// Creates an identifier from an application-owned value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the identifier as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable identifier of a saved forward rule.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ForwardRuleId(String);

impl ForwardRuleId {
    /// Creates an identifier from an application-owned value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the identifier as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A concrete `Host` alias discovered from SSH configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Application-owned stable target identifier.
    pub id: TargetId,
    /// Exact alias passed to OpenSSH.
    pub host_alias: String,
    /// SSH configuration file where the alias was declared.
    pub source: PathBuf,
}

/// Runtime state of a portdeck-owned ControlMaster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// No usable ControlMaster exists.
    Disconnected,
    /// A connect operation is running.
    Connecting,
    /// OpenSSH confirmed the ControlMaster with `-O check`.
    Connected,
    /// A disconnect operation is running.
    Stopping,
    /// The most recent operation failed.
    Failed,
}

/// Runtime state of a local forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardState {
    /// The saved rule is not installed in a ControlMaster.
    Inactive,
    /// An add operation is running.
    Adding,
    /// OpenSSH confirmed that the forward was added.
    Active,
    /// A cancel operation is running.
    Removing,
    /// The most recent operation failed.
    Failed,
    /// The parent ControlMaster is unavailable.
    Unavailable,
}

/// Error categories presented to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The system OpenSSH client could not be executed.
    OpenSshNotFound,
    /// Authentication or connection establishment failed.
    ConnectionFailed,
    /// The expected ControlMaster is not available.
    ControlMasterUnavailable,
    /// OpenSSH could not bind the requested local port.
    LocalPortConflict,
    /// The SSH server rejected TCP forwarding.
    ForwardRejected,
    /// OpenSSH could not cancel a previously added forward.
    CancelFailed,
    /// SSH target discovery or effective configuration resolution failed.
    SshConfigurationFailed,
    /// A local runtime or persistence operation failed.
    LocalOperationFailed,
}

/// Concise failure information with optional diagnostic detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// Stable category used to select the UI summary.
    pub kind: FailureKind,
    /// Short message suitable for the status line.
    pub summary: String,
    /// Detailed stderr or local error information.
    pub detail: Option<String>,
}

impl Failure {
    /// Creates failure information while keeping diagnostic text separate.
    pub fn new(kind: FailureKind, summary: impl Into<String>, detail: Option<String>) -> Self {
        Self {
            kind,
            summary: summary.into(),
            detail,
        }
    }
}

/// Runtime representation of a portdeck-owned SSH session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Target owning this session.
    pub target_id: TargetId,
    /// Dedicated ControlPath for this session.
    pub control_path: PathBuf,
    /// Current runtime state.
    pub state: SessionState,
    /// Most recent operation failure.
    pub last_error: Option<Failure>,
}

impl Session {
    /// Creates a disconnected session for a target.
    pub fn new(target_id: TargetId, control_path: PathBuf) -> Self {
        Self {
            target_id,
            control_path,
            state: SessionState::Disconnected,
            last_error: None,
        }
    }

    /// Applies a checked state transition.
    pub fn transition(&mut self, next: SessionState) -> Result<(), TransitionError> {
        if !valid_session_transition(self.state, next) {
            return Err(TransitionError::Session {
                from: self.state,
                to: next,
            });
        }

        self.state = next;
        if next != SessionState::Failed {
            self.last_error = None;
        }
        Ok(())
    }

    /// Records a failed operation.
    pub fn fail(&mut self, failure: Failure) -> Result<(), TransitionError> {
        self.transition(SessionState::Failed)?;
        self.last_error = Some(failure);
        Ok(())
    }
}

/// Persistent local-forward definition belonging to a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardRule {
    /// Stable saved-rule identifier.
    pub id: ForwardRuleId,
    /// Target owning this definition.
    pub target_id: TargetId,
    /// Optional display label.
    pub label: Option<String>,
    /// Address on the local side where OpenSSH listens.
    pub bind_address: String,
    /// Preferred local port, or the remote port when omitted.
    pub requested_local_port: Option<u16>,
    /// Host reached from the remote side.
    pub remote_host: String,
    /// Port reached from the remote side.
    pub remote_port: u16,
}

/// Runtime state associated with a saved forward rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveForward {
    /// Saved rule being activated.
    pub rule_id: ForwardRuleId,
    /// Local port OpenSSH was asked to bind.
    pub actual_local_port: Option<u16>,
    /// Exact normalized `-L` value used for add and cancel.
    pub normalized_spec: Option<String>,
    /// Current runtime state.
    pub state: ForwardState,
    /// Most recent operation failure.
    pub last_error: Option<Failure>,
}

impl ActiveForward {
    /// Creates inactive runtime state for a saved rule.
    pub fn new(rule_id: ForwardRuleId) -> Self {
        Self {
            rule_id,
            actual_local_port: None,
            normalized_spec: None,
            state: ForwardState::Inactive,
            last_error: None,
        }
    }

    /// Applies a checked state transition.
    pub fn transition(&mut self, next: ForwardState) -> Result<(), TransitionError> {
        if !valid_forward_transition(self.state, next) {
            return Err(TransitionError::Forward {
                from: self.state,
                to: next,
            });
        }

        self.state = next;
        if next != ForwardState::Failed {
            self.last_error = None;
        }
        if next == ForwardState::Inactive {
            self.actual_local_port = None;
            self.normalized_spec = None;
        }
        Ok(())
    }

    /// Records a successful forward addition.
    pub fn activate(
        &mut self,
        actual_local_port: u16,
        normalized_spec: String,
    ) -> Result<(), TransitionError> {
        self.transition(ForwardState::Active)?;
        self.actual_local_port = Some(actual_local_port);
        self.normalized_spec = Some(normalized_spec);
        Ok(())
    }

    /// Records a failed operation.
    pub fn fail(&mut self, failure: Failure) -> Result<(), TransitionError> {
        self.transition(ForwardState::Failed)?;
        self.last_error = Some(failure);
        Ok(())
    }
}

/// Rejected domain state transition.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TransitionError {
    /// The session transition is not part of the state model.
    #[error("invalid session transition: {from:?} -> {to:?}")]
    Session {
        /// Current state.
        from: SessionState,
        /// Requested state.
        to: SessionState,
    },
    /// The forward transition is not part of the state model.
    #[error("invalid forward transition: {from:?} -> {to:?}")]
    Forward {
        /// Current state.
        from: ForwardState,
        /// Requested state.
        to: ForwardState,
    },
}

fn valid_session_transition(from: SessionState, to: SessionState) -> bool {
    use SessionState::{Connected, Connecting, Disconnected, Failed, Stopping};

    matches!(
        (from, to),
        (Disconnected | Failed, Connecting)
            | (Connecting, Connected | Failed | Disconnected)
            | (Connected, Stopping | Failed | Disconnected)
            | (Stopping, Disconnected | Failed)
            | (Failed, Disconnected)
    )
}

fn valid_forward_transition(from: ForwardState, to: ForwardState) -> bool {
    use ForwardState::{Active, Adding, Failed, Inactive, Removing, Unavailable};

    matches!(
        (from, to),
        (Inactive | Failed | Unavailable, Adding)
            | (Adding, Active | Failed | Inactive | Unavailable)
            | (Active, Removing | Failed | Unavailable)
            | (Removing, Inactive | Failed | Unavailable)
            | (Failed, Inactive | Removing | Unavailable)
            | (Unavailable, Inactive)
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        ActiveForward, Failure, FailureKind, ForwardRuleId, ForwardState, Session, SessionState,
        TargetId,
    };

    #[test]
    fn session_happy_path_follows_declared_states() {
        let mut session = Session::new(TargetId::new("dev"), PathBuf::from("control/dev"));

        session.transition(SessionState::Connecting).unwrap();
        session.transition(SessionState::Connected).unwrap();
        session.transition(SessionState::Stopping).unwrap();
        session.transition(SessionState::Disconnected).unwrap();

        assert_eq!(session.state, SessionState::Disconnected);
    }

    #[test]
    fn session_rejects_skipping_connecting() {
        let mut session = Session::new(TargetId::new("dev"), PathBuf::from("control/dev"));

        let error = session.transition(SessionState::Connected).unwrap_err();

        assert_eq!(session.state, SessionState::Disconnected);
        assert_eq!(
            error.to_string(),
            "invalid session transition: Disconnected -> Connected"
        );
    }

    #[test]
    fn session_failure_keeps_diagnostic_detail() {
        let mut session = Session::new(TargetId::new("dev"), PathBuf::from("control/dev"));
        session.transition(SessionState::Connecting).unwrap();

        session
            .fail(Failure::new(
                FailureKind::ConnectionFailed,
                "SSH connection failed",
                Some("Permission denied".to_owned()),
            ))
            .unwrap();

        assert_eq!(session.state, SessionState::Failed);
        assert_eq!(
            session
                .last_error
                .as_ref()
                .and_then(|error| error.detail.as_deref()),
            Some("Permission denied")
        );
    }

    #[test]
    fn forward_is_active_only_after_successful_addition() {
        let mut forward = ActiveForward::new(ForwardRuleId::new("web"));

        forward.transition(ForwardState::Adding).unwrap();
        assert_eq!(forward.state, ForwardState::Adding);
        assert_eq!(forward.actual_local_port, None);

        forward
            .activate(8081, "127.0.0.1:8081:127.0.0.1:3000".to_owned())
            .unwrap();

        assert_eq!(forward.state, ForwardState::Active);
        assert_eq!(forward.actual_local_port, Some(8081));
    }

    #[test]
    fn removing_forward_clears_runtime_values_only_after_success() {
        let mut forward = ActiveForward::new(ForwardRuleId::new("web"));
        forward.transition(ForwardState::Adding).unwrap();
        forward
            .activate(8080, "127.0.0.1:8080:127.0.0.1:3000".to_owned())
            .unwrap();

        forward.transition(ForwardState::Removing).unwrap();
        assert_eq!(forward.actual_local_port, Some(8080));

        forward.transition(ForwardState::Inactive).unwrap();
        assert_eq!(forward.actual_local_port, None);
        assert_eq!(forward.normalized_spec, None);
    }

    #[test]
    fn disconnected_parent_makes_active_forward_unavailable() {
        let mut forward = ActiveForward::new(ForwardRuleId::new("db"));
        forward.transition(ForwardState::Adding).unwrap();
        forward
            .activate(5432, "127.0.0.1:5432:db:5432".to_owned())
            .unwrap();

        forward.transition(ForwardState::Unavailable).unwrap();

        assert_eq!(forward.state, ForwardState::Unavailable);
        assert_eq!(forward.actual_local_port, Some(5432));
    }
}
