#![cfg(unix)]

use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use portdeck::domain::TargetId;
use portdeck::runtime::RuntimeDirectory;
use portdeck::ssh::{
    CommandExecutor, ExecutionMode, LocalForwardSpec, OpenSsh, SshCommand, SshOutput,
};

#[derive(Debug)]
struct ConfiguredSshExecutor {
    client_config: PathBuf,
}

impl CommandExecutor for ConfiguredSshExecutor {
    fn execute(&self, command: &SshCommand, _mode: ExecutionMode) -> io::Result<SshOutput> {
        let output = Command::new("/usr/bin/ssh")
            .arg("-F")
            .arg(&self.client_config)
            .args(command.arguments())
            .output()?;
        Ok(SshOutput {
            success: output.status.success(),
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

struct SshdFixture {
    directory: PathBuf,
    client_config: PathBuf,
    child: Child,
}

impl SshdFixture {
    fn start() -> Self {
        for executable in ["/usr/bin/ssh", "/usr/bin/ssh-keygen", "/usr/sbin/sshd"] {
            assert!(Path::new(executable).is_file(), "missing {executable}");
        }

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("pd-it-{}-{unique}", std::process::id()));
        fs::create_dir(&directory).unwrap();

        let host_key = directory.join("ssh_host_ed25519_key");
        let client_key = directory.join("client_ed25519_key");
        generate_key(&host_key);
        generate_key(&client_key);
        let authorized_keys = directory.join("authorized_keys");
        fs::copy(client_key.with_extension("pub"), &authorized_keys).unwrap();

        let ssh_port = unused_port();
        let username = std::env::var("USER").expect("USER is required for integration testing");
        let server_config = directory.join("sshd_config");
        fs::write(
            &server_config,
            format!(
                "Port {ssh_port}\nListenAddress 127.0.0.1\nHostKey {}\nPidFile {}\nAuthorizedKeysFile {}\nPubkeyAuthentication yes\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nStrictModes no\nUsePAM no\nPermitRootLogin no\nAllowUsers {username}\nLogLevel ERROR\n",
                host_key.display(),
                directory.join("sshd.pid").display(),
                authorized_keys.display(),
            ),
        )
        .unwrap();

        let child = Command::new("/usr/sbin/sshd")
            .arg("-D")
            .arg("-e")
            .arg("-f")
            .arg(&server_config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let known_hosts = directory.join("known_hosts");
        let public_host_key = fs::read_to_string(host_key.with_extension("pub")).unwrap();
        let key_fields = public_host_key
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>();
        fs::write(
            &known_hosts,
            format!(
                "[127.0.0.1]:{ssh_port} {} {}\n",
                key_fields[0], key_fields[1]
            ),
        )
        .unwrap();

        let client_config = directory.join("ssh_config");
        fs::write(
            &client_config,
            format!(
                "Host integration-target\n  HostName 127.0.0.1\n  Port {ssh_port}\n  User {username}\n  IdentityFile {}\n  IdentitiesOnly yes\n  UserKnownHostsFile {}\n  StrictHostKeyChecking yes\n  BatchMode yes\n  LogLevel ERROR\n",
                client_key.display(),
                known_hosts.display(),
            ),
        )
        .unwrap();

        let mut fixture = Self {
            directory,
            client_config,
            child,
        };
        fixture.wait_until_ready(ssh_port);
        fixture
    }

    fn wait_until_ready(&mut self, port: u16) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().unwrap() {
                let mut stderr = String::new();
                self.child
                    .stderr
                    .as_mut()
                    .unwrap()
                    .read_to_string(&mut stderr)
                    .unwrap();
                panic!("test sshd exited with {status}: {stderr}");
            }
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("test sshd did not listen within five seconds");
    }
}

impl Drop for SshdFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

fn generate_key(path: &Path) {
    let status = Command::new("/usr/bin/ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "ssh-keygen failed for {}", path.display());
}

fn unused_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
#[ignore = "requires local /usr/sbin/sshd and isolated key generation"]
fn controlmaster_forward_traffic_cancel_and_exit() {
    let fixture = SshdFixture::start();
    let executor = ConfiguredSshExecutor {
        client_config: fixture.client_config.clone(),
    };
    let ssh = OpenSsh::with_executor("/usr/bin/ssh", executor);
    let runtime = RuntimeDirectory::prepare(fixture.directory.join("runtime")).unwrap();
    let control_path = runtime
        .control_path(&TargetId::new("integration-target"))
        .unwrap();

    let remote_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let remote_port = remote_listener.local_addr().unwrap().port();
    let echo_server = thread::spawn(move || {
        let (mut connection, _) = remote_listener.accept().unwrap();
        let mut request = [0_u8; 4];
        connection.read_exact(&mut request).unwrap();
        assert_eq!(&request, b"ping");
        connection.write_all(b"pong").unwrap();
    });
    let local_port = unused_port();
    let forward = LocalForwardSpec::new("127.0.0.1", local_port, "127.0.0.1", remote_port).unwrap();

    let connect = ssh.connect("integration-target", &control_path).unwrap();
    assert!(connect.success, "{}", connect.stderr);
    assert!(
        ssh.check("integration-target", &control_path)
            .unwrap()
            .success
    );

    let add = ssh
        .add_local_forward("integration-target", &control_path, &forward)
        .unwrap();
    assert!(add.success, "{}", add.stderr);
    let mut forwarded_connection = TcpStream::connect(("127.0.0.1", local_port)).unwrap();
    forwarded_connection.write_all(b"ping").unwrap();
    let mut response = [0_u8; 4];
    forwarded_connection.read_exact(&mut response).unwrap();
    assert_eq!(&response, b"pong");
    echo_server.join().unwrap();

    let cancel = ssh
        .cancel_local_forward("integration-target", &control_path, &forward)
        .unwrap();
    assert!(cancel.success, "{}", cancel.stderr);
    thread::sleep(Duration::from_millis(50));
    assert!(TcpStream::connect(("127.0.0.1", local_port)).is_err());

    let disconnect = ssh.disconnect("integration-target", &control_path).unwrap();
    assert!(disconnect.success, "{}", disconnect.stderr);
    assert!(
        !ssh.check("integration-target", &control_path)
            .unwrap()
            .success
    );
}
