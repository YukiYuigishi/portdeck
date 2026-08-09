#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use portdeck::domain::TargetId;
use portdeck::runtime::RuntimeDirectory;
use portdeck::ssh::{LocalForwardSpec, OpenSsh};

struct SshdFixture {
    directory: PathBuf,
    client_executable: PathBuf,
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
                "Host integration-target integration-jump integration-target-via-jump\n  HostName 127.0.0.1\n  Port {ssh_port}\n  User {username}\n  IdentityFile {}\n  IdentitiesOnly yes\n  UserKnownHostsFile {}\n  StrictHostKeyChecking yes\n  BatchMode yes\n  LogLevel DEBUG3\n\nHost integration-target-via-jump\n  ProxyJump integration-jump\n",
                client_key.display(),
                known_hosts.display(),
            ),
        )
        .unwrap();

        let client_executable = directory.join("ssh");
        fs::write(
            &client_executable,
            format!(
                "#!/bin/sh\nexec /usr/bin/ssh -F '{}' \"$@\"\n",
                client_config.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&client_executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&client_executable, permissions).unwrap();

        let mut fixture = Self {
            directory,
            client_executable,
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

fn echo_once(
    listener: TcpListener,
    expected: &'static [u8],
    response: &'static [u8],
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let (mut connection, _) = listener.accept().unwrap();
        let mut request = vec![0_u8; expected.len()];
        connection.read_exact(&mut request).unwrap();
        assert_eq!(request, expected);
        connection.write_all(response).unwrap();
    })
}

fn exchange(port: u16, request: &[u8], expected_response: &[u8]) {
    let mut connection = TcpStream::connect(("127.0.0.1", port)).unwrap();
    connection.write_all(request).unwrap();
    let mut response = vec![0_u8; expected_response.len()];
    connection.read_exact(&mut response).unwrap();
    assert_eq!(response, expected_response);
}

#[test]
#[ignore = "requires local /usr/sbin/sshd and isolated key generation"]
fn controlmaster_forward_traffic_cancel_and_exit() {
    let fixture = SshdFixture::start();
    let ssh = OpenSsh::new(&fixture.client_executable);
    let runtime = RuntimeDirectory::prepare(fixture.directory.join("runtime")).unwrap();
    let control_path = runtime
        .control_path(&TargetId::new("integration-target"))
        .unwrap();

    let remote_listener_a = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let remote_listener_b = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let remote_port_a = remote_listener_a.local_addr().unwrap().port();
    let remote_port_b = remote_listener_b.local_addr().unwrap().port();
    let echo_server_a = echo_once(remote_listener_a, b"one!", b"first");
    let echo_server_b = echo_once(remote_listener_b, b"two!", b"second");
    let local_reservation_a = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let local_reservation_b = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let local_port_a = local_reservation_a.local_addr().unwrap().port();
    let local_port_b = local_reservation_b.local_addr().unwrap().port();
    drop((local_reservation_a, local_reservation_b));
    let forward_a =
        LocalForwardSpec::new("127.0.0.1", local_port_a, "127.0.0.1", remote_port_a).unwrap();
    let forward_b =
        LocalForwardSpec::new("127.0.0.1", local_port_b, "127.0.0.1", remote_port_b).unwrap();

    let connect = ssh.connect("integration-target", &control_path).unwrap();
    assert!(connect.success, "{}", connect.stderr);
    assert!(
        ssh.check("integration-target", &control_path)
            .unwrap()
            .success
    );

    let add_a = ssh
        .add_local_forward("integration-target", &control_path, &forward_a)
        .unwrap();
    assert!(add_a.success, "{}", add_a.stderr);
    let add_b = ssh
        .add_local_forward("integration-target", &control_path, &forward_b)
        .unwrap();
    assert!(add_b.success, "{}", add_b.stderr);
    exchange(local_port_a, b"one!", b"first");
    echo_server_a.join().unwrap();

    let cancel_a = ssh
        .cancel_local_forward("integration-target", &control_path, &forward_a)
        .unwrap();
    assert!(cancel_a.success, "{}", cancel_a.stderr);
    thread::sleep(Duration::from_millis(50));
    assert!(TcpStream::connect(("127.0.0.1", local_port_a)).is_err());
    exchange(local_port_b, b"two!", b"second");
    echo_server_b.join().unwrap();
    let cancel_b = ssh
        .cancel_local_forward("integration-target", &control_path, &forward_b)
        .unwrap();
    assert!(cancel_b.success, "{}", cancel_b.stderr);
    thread::sleep(Duration::from_millis(50));
    assert!(TcpStream::connect(("127.0.0.1", local_port_b)).is_err());

    let disconnect = ssh.disconnect("integration-target", &control_path).unwrap();
    assert!(disconnect.success, "{}", disconnect.stderr);
    assert!(
        !ssh.check("integration-target", &control_path)
            .unwrap()
            .success
    );
}

#[test]
#[ignore = "requires local /usr/sbin/sshd and isolated key generation"]
fn proxyjump_controlmaster_returns_checks_and_exits() {
    let fixture = SshdFixture::start();
    let ssh = OpenSsh::new(&fixture.client_executable);
    let runtime = RuntimeDirectory::prepare(fixture.directory.join("runtime")).unwrap();
    let control_path = runtime
        .control_path(&TargetId::new("integration-target-via-jump"))
        .unwrap();

    let started = Instant::now();
    let connect = ssh
        .connect("integration-target-via-jump", &control_path)
        .unwrap();
    let connect_elapsed = started.elapsed();
    let check = ssh
        .check("integration-target-via-jump", &control_path)
        .unwrap();
    let disconnect = ssh
        .disconnect("integration-target-via-jump", &control_path)
        .unwrap();

    assert!(connect.success, "{}", connect.stderr);
    assert!(
        connect_elapsed < Duration::from_secs(3),
        "ProxyJump connect waited {connect_elapsed:?} for inherited stderr to close"
    );
    assert!(check.success, "{}", check.stderr);
    assert!(disconnect.success, "{}", disconnect.stderr);
    assert!(
        !ssh.check("integration-target-via-jump", &control_path)
            .unwrap()
            .success
    );
}
