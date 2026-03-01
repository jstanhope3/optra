//! Browsing a remote machine over SFTP.
//!
//! Everything network-facing lives on one worker thread running a tokio
//! runtime. The UI thread never blocks on it: requests go over a channel, and
//! results land in shared maps that the UI reads each frame. A round trip that
//! is microseconds locally can be tens of milliseconds remotely, which would be
//! a visible freeze on every folder expansion if it ran inline.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use russh::client::{self, AuthResult};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::AgentClient;
use russh::keys::check_known_hosts;
use russh_sftp::client::SftpSession;

/// One entry in a remote directory.
#[derive(Clone)]
pub struct RemoteEntry {
    pub path: PathBuf,
    pub is_dir: bool,
}

/// State of a directory listing the UI has asked for.
#[derive(Clone)]
pub enum ListState {
    Loading,
    Ready(Vec<RemoteEntry>),
    Failed(String),
}

/// State of a file the UI has asked for.
#[derive(Clone)]
pub enum FileState {
    Loading,
    Ready(Arc<Vec<u8>>),
    Failed(String),
}

#[derive(Clone, PartialEq)]
pub enum Status {
    Connecting,
    Connected { home: PathBuf },
    Failed(String),
}

enum Request {
    List(PathBuf),
    Fetch(PathBuf),
}

/// A live (or failed) connection to a remote machine.
pub struct RemoteFs {
    tx: Sender<Request>,
    listings: Arc<Mutex<HashMap<PathBuf, ListState>>>,
    files: Arc<Mutex<HashMap<PathBuf, FileState>>>,
    status: Arc<Mutex<Status>>,
    shutdown: Arc<AtomicBool>,
    pub label: String,
}

impl RemoteFs {
    /// Start connecting. Returns immediately; watch [`RemoteFs::status`].
    pub fn connect(host: String, port: u16, user: String, ctx: egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let listings = Arc::new(Mutex::new(HashMap::new()));
        let files = Arc::new(Mutex::new(HashMap::new()));
        let status = Arc::new(Mutex::new(Status::Connecting));
        let shutdown = Arc::new(AtomicBool::new(false));

        let label = format!("{user}@{host}");

        {
            let listings = Arc::clone(&listings);
            let files = Arc::clone(&files);
            let status = Arc::clone(&status);
            let shutdown = Arc::clone(&shutdown);

            std::thread::spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        *status.lock().unwrap() = Status::Failed(format!("runtime: {e}"));
                        ctx.request_repaint();
                        return;
                    }
                };

                runtime.block_on(worker(
                    host, port, user, rx, listings, files, status, shutdown, ctx,
                ));
            });
        }

        Self {
            tx,
            listings,
            files,
            status,
            shutdown,
            label,
        }
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// The listing for `dir`, requesting it if this is the first time it is asked for.
    pub fn list(&self, dir: &Path) -> ListState {
        let mut listings = self.listings.lock().unwrap();
        if let Some(state) = listings.get(dir) {
            return state.clone();
        }
        listings.insert(dir.to_path_buf(), ListState::Loading);
        drop(listings);

        let _ = self.tx.send(Request::List(dir.to_path_buf()));
        ListState::Loading
    }

    /// The bytes of `path`, requesting them if this is the first time.
    pub fn file(&self, path: &Path) -> FileState {
        let mut files = self.files.lock().unwrap();
        if let Some(state) = files.get(path) {
            return state.clone();
        }
        files.insert(path.to_path_buf(), FileState::Loading);
        drop(files);

        let _ = self.tx.send(Request::Fetch(path.to_path_buf()));
        FileState::Loading
    }

    /// Forget fetched file bytes, keeping directory listings. Remote files are
    /// decoded into the normal image cache, so holding the raw bytes too would
    /// double the memory for no gain.
    pub fn forget_file(&self, path: &Path) {
        self.files.lock().unwrap().remove(path);
    }
}

impl Drop for RemoteFs {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

/// Accepts a host key only if `~/.ssh/known_hosts` already vouches for it, the
/// same rule the `ssh` command applies.
struct Verifier {
    host: String,
    port: u16,
}

impl client::Handler for Verifier {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let public_key = match key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            russh::keys::PublicKeyOrCertificate::Certificate(cert) => {
                russh::keys::PublicKey::new(cert.public_key().clone(), "")
            }
        };
        Ok(check_known_hosts(&self.host, self.port, &public_key).unwrap_or(false))
    }
}

#[allow(clippy::too_many_arguments)]
async fn worker(
    host: String,
    port: u16,
    user: String,
    rx: Receiver<Request>,
    listings: Arc<Mutex<HashMap<PathBuf, ListState>>>,
    files: Arc<Mutex<HashMap<PathBuf, FileState>>>,
    status: Arc<Mutex<Status>>,
    shutdown: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    let sftp = match establish(&host, port, &user).await {
        Ok(sftp) => sftp,
        Err(e) => {
            *status.lock().unwrap() = Status::Failed(e);
            ctx.request_repaint();
            return;
        }
    };

    // Start where ssh would: the user's home directory.
    let home = sftp
        .canonicalize(".")
        .await
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"));

    *status.lock().unwrap() = Status::Connected { home };
    ctx.request_repaint();

    // `recv` blocks this thread, so poll instead: the runtime is
    // current-thread and must stay free to drive the SFTP futures.
    loop {
        if shutdown.load(Ordering::Relaxed) {
            return;
        }

        let request = match rx.try_recv() {
            Ok(request) => request,
            Err(mpsc::TryRecvError::Empty) => {
                tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                continue;
            }
            Err(mpsc::TryRecvError::Disconnected) => return,
        };

        match request {
            Request::List(dir) => {
                let state = match sftp.read_dir(dir.to_string_lossy().to_string()).await {
                    Ok(entries) => ListState::Ready(
                        entries
                            .map(|e| RemoteEntry {
                                path: dir.join(e.file_name()),
                                is_dir: e.file_type().is_dir(),
                            })
                            .collect(),
                    ),
                    Err(e) => ListState::Failed(e.to_string()),
                };
                listings.lock().unwrap().insert(dir, state);
            }
            Request::Fetch(path) => {
                let state = match sftp.read(path.to_string_lossy().to_string()).await {
                    Ok(bytes) => FileState::Ready(Arc::new(bytes)),
                    Err(e) => FileState::Failed(e.to_string()),
                };
                files.lock().unwrap().insert(path, state);
            }
        }

        ctx.request_repaint();
    }
}

/// Connect, authenticate via the SSH agent, and open an SFTP subsystem.
async fn establish(host: &str, port: u16, user: &str) -> Result<SftpSession, String> {
    let config = Arc::new(client::Config::default());
    let verifier = Verifier {
        host: host.to_string(),
        port,
    };

    let mut session = client::connect(config, (host, port), verifier)
        .await
        .map_err(|e| match e {
            russh::Error::UnknownKey => format!(
                "Host key for {host} is not in ~/.ssh/known_hosts. \
                 Run `ssh {user}@{host}` once to record it."
            ),
            other => format!("Could not connect to {host}:{port}: {other}"),
        })?;

    let mut agent = AgentClient::connect_env()
        .await
        .map_err(|e| format!("No SSH agent available ({e}). Try `ssh-add`."))?;

    let identities = agent
        .request_identities()
        .await
        .map_err(|e| format!("Could not list agent keys: {e}"))?;

    if identities.is_empty() {
        return Err("The SSH agent has no keys loaded. Run `ssh-add` first.".into());
    }

    let mut authenticated = false;
    for identity in identities {
        let AgentIdentity::PublicKey { key, .. } = &identity else {
            continue; // certificates are not handled here
        };

        let key = key.clone();
        match session
            .authenticate_publickey_with(user, key, None, &mut agent)
            .await
        {
            Ok(AuthResult::Success) => {
                authenticated = true;
                break;
            }
            Ok(AuthResult::Failure { .. }) => continue,
            Err(e) => return Err(format!("Authentication error: {e}")),
        }
    }

    if !authenticated {
        return Err(format!(
            "No key in the SSH agent was accepted by {user}@{host}."
        ));
    }

    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| format!("Could not open channel: {e}"))?;

    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("Remote host refused the SFTP subsystem: {e}"))?;

    SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| format!("SFTP handshake failed: {e}"))
}
