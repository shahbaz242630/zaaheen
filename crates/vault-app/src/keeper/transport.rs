//! Local IPC between relays and the keeper: named pipes on Windows, unix
//! sockets elsewhere.
//!
//! Chosen over loopback HTTP after adversarial review (ADR-SEC-019): a named
//! pipe never enters the network stack, so HTTP proxy settings cannot divert
//! it and loopback packet capture cannot read it; remote clients are refused;
//! other local users get read-only access at most (they can connect but never
//! send a byte, and the keeper drops silent connections at the frame-1
//! deadline). And a pipe connection is ONE persistent stream, which is what
//! lets the handshake bind to it.
//!
//! **Security QoS:** client connections keep tokio's default
//! `SECURITY_IDENTIFICATION`, so a pipe server squatting a dead keeper's name
//! cannot impersonate the relay's user. Never override it.

use std::io;

/// Why a relay could not connect. The relay acts differently on each.
#[derive(Debug)]
pub enum ConnectError {
    /// The keeper is alive but every pipe instance is taken. Retry shortly.
    Busy,
    /// Nothing is listening at that endpoint: no keeper, or a dead one.
    NotFound,
    /// The endpoint exists but refuses us — most likely a squatter holding a
    /// dead keeper's name. Start a keeper; it will choose a fresh name.
    Denied,
    Other(io::Error),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => write!(f, "keeper busy"),
            Self::NotFound => write!(f, "no keeper at endpoint"),
            Self::Denied => write!(f, "endpoint refused access"),
            Self::Other(e) => write!(f, "connect: {e}"),
        }
    }
}

impl std::error::Error for ConnectError {}

#[cfg(windows)]
pub use windows_impl::{connect, ClientStream, Listener, ServerStream};

#[cfg(not(windows))]
pub use unix_impl::{connect, ClientStream, Listener, ServerStream};

#[cfg(windows)]
mod windows_impl {
    use std::io;
    use std::time::Duration;

    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };

    use super::ConnectError;

    pub type ServerStream = NamedPipeServer;
    pub type ClientStream = NamedPipeClient;

    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_PIPE_BUSY: i32 = 231;

    /// Attempts at creating the next pipe instance before declaring the
    /// listener broken (the keeper then rotates to a fresh name).
    const INSTANCE_ATTEMPTS: u32 = 3;

    /// A keeper's listening endpoint. There is always one instance waiting
    /// for a client; if that can no longer be guaranteed, `accept` fails and
    /// the keeper moves to a fresh endpoint rather than advertising a pipe
    /// nobody can reach.
    pub struct Listener {
        name: String,
        next: Option<NamedPipeServer>,
    }

    impl Listener {
        /// Create the FIRST instance. `first_pipe_instance` makes this fail if
        /// anything already owns the name — a squatter is detected here, at
        /// creation, rather than discovered by relays later.
        ///
        /// # Errors
        ///
        /// The name is taken or the pipe could not be created.
        pub fn bind(endpoint: &str) -> io::Result<Self> {
            let first = ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(endpoint)?;
            Ok(Self {
                name: endpoint.to_string(),
                next: Some(first),
            })
        }

        /// Wait for the next client. Creates the following instance BEFORE
        /// returning the connected one, so a client arriving meanwhile never
        /// finds zero instances.
        ///
        /// **Cancel-safe.** The keeper's serve loop drops this future every
        /// time another branch wins (its idle tick fires every few seconds).
        /// The waiting instance therefore stays in `self` throughout: an
        /// earlier version took it out first, so each cancellation closed the
        /// listening pipe — a relay connecting at that moment found no keeper
        /// at all, and one that had just connected was cut off.
        ///
        /// # Errors
        ///
        /// The listener can no longer keep an instance available.
        pub async fn accept(&mut self) -> io::Result<NamedPipeServer> {
            loop {
                if self.next.is_none() {
                    self.next = Some(self.create_instance().await?);
                }
                // A client that connected before a cancellation is picked up
                // here: connecting an already-connected instance succeeds at
                // once.
                let connected = match self.next.as_ref() {
                    Some(instance) => instance.connect().await,
                    None => continue,
                };
                // The replacement is created while the current instance still
                // exists, so the name never has zero instances.
                let replacement = self.create_instance().await?;
                let current = self.next.replace(replacement);
                if let (Ok(()), Some(current)) = (connected, current) {
                    return Ok(current);
                }
                // The client vanished mid-connect (Claude kills relays
                // abruptly): that instance was dropped just above; keep
                // listening on its replacement.
            }
        }

        async fn create_instance(&self) -> io::Result<NamedPipeServer> {
            let mut delay = Duration::from_millis(50);
            let mut attempt = 1;
            loop {
                match ServerOptions::new()
                    .reject_remote_clients(true)
                    .create(&self.name)
                {
                    Ok(instance) => return Ok(instance),
                    Err(e) if attempt >= INSTANCE_ATTEMPTS => return Err(e),
                    Err(_) => {
                        attempt += 1;
                        tokio::time::sleep(delay).await;
                        delay *= 4;
                    }
                }
            }
        }
    }

    /// How long a client keeps retrying a busy pipe before reporting Busy.
    ///
    /// A pipe is momentarily "busy" between the keeper accepting one
    /// connection and creating the next instance, so relays connecting at the
    /// same instant — Claude Desktop's chat and its shared pool do exactly
    /// that — routinely see it. Retrying is the client's job (tokio's named
    /// pipe docs). Found by `two_clients_are_served_concurrently`, which failed
    /// with `Busy` before this existed.
    const BUSY_RETRY_FOR: Duration = Duration::from_millis(500);
    const BUSY_RETRY_EVERY: Duration = Duration::from_millis(10);

    /// Connect to a keeper endpoint.
    ///
    /// # Errors
    ///
    /// A [`ConnectError`] the relay branches on. `Busy` only after
    /// [`BUSY_RETRY_FOR`] of retrying.
    pub async fn connect(endpoint: &str) -> Result<NamedPipeClient, ConnectError> {
        let deadline = tokio::time::Instant::now() + BUSY_RETRY_FOR;
        loop {
            match ClientOptions::new().open(endpoint) {
                Ok(client) => return Ok(client),
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(ConnectError::Busy);
                    }
                    tokio::time::sleep(BUSY_RETRY_EVERY).await;
                }
                Err(e) => {
                    return Err(match e.raw_os_error() {
                        Some(ERROR_FILE_NOT_FOUND) => ConnectError::NotFound,
                        Some(ERROR_ACCESS_DENIED) => ConnectError::Denied,
                        _ => ConnectError::Other(e),
                    })
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod unix_impl {
    use std::io;
    use std::path::{Path, PathBuf};

    use tokio::net::{UnixListener, UnixStream};

    use super::ConnectError;

    pub type ServerStream = UnixStream;
    pub type ClientStream = UnixStream;

    /// A keeper's listening socket, inside the vault's own 0700 directory.
    /// The socket file is removed when the listener is dropped.
    pub struct Listener {
        path: PathBuf,
        inner: UnixListener,
    }

    impl Listener {
        /// # Errors
        ///
        /// The directory could not be created or the path is taken.
        pub fn bind(endpoint: &str) -> io::Result<Self> {
            use std::os::unix::fs::PermissionsExt;
            let path = PathBuf::from(endpoint);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
            let inner = UnixListener::bind(&path)?;
            Ok(Self { path, inner })
        }

        /// # Errors
        ///
        /// Accept failed.
        pub async fn accept(&mut self) -> io::Result<UnixStream> {
            let (stream, _addr) = self.inner.accept().await?;
            Ok(stream)
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// # Errors
    ///
    /// A [`ConnectError`] the relay branches on.
    pub async fn connect(endpoint: &str) -> Result<UnixStream, ConnectError> {
        match UnixStream::connect(Path::new(endpoint)).await {
            Ok(stream) => Ok(stream),
            Err(e) => Err(match e.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                    ConnectError::NotFound
                }
                io::ErrorKind::PermissionDenied => ConnectError::Denied,
                _ => ConnectError::Other(e),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keeper::discovery::new_endpoint;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn a_client_and_the_keeper_exchange_bytes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let endpoint = new_endpoint(tmp.path()).unwrap();
        let mut listener = Listener::bind(&endpoint).unwrap();

        let server = tokio::spawn(async move {
            let mut s = listener.accept().await.unwrap();
            let mut buf = [0u8; 4];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(b"pong").await.unwrap();
            buf
        });

        let mut c = connect(&endpoint).await.unwrap();
        c.write_all(b"ping").await.unwrap();
        let mut reply = [0u8; 4];
        c.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"pong");
        assert_eq!(&server.await.unwrap(), b"ping");
    }

    /// Two relays at once — Claude Desktop alone starts several.
    #[tokio::test]
    async fn two_clients_are_served_concurrently() {
        let tmp = tempfile::TempDir::new().unwrap();
        let endpoint = new_endpoint(tmp.path()).unwrap();
        let mut listener = Listener::bind(&endpoint).unwrap();

        let server = tokio::spawn(async move {
            let mut handles = Vec::new();
            for _ in 0..2 {
                let mut s = listener.accept().await.unwrap();
                handles.push(tokio::spawn(async move {
                    let mut buf = [0u8; 1];
                    s.read_exact(&mut buf).await.unwrap();
                    s.write_all(&buf).await.unwrap();
                }));
            }
            for h in handles {
                h.await.unwrap();
            }
        });

        let mut a = connect(&endpoint).await.unwrap();
        let mut b = connect(&endpoint).await.unwrap();
        a.write_all(b"a").await.unwrap();
        b.write_all(b"b").await.unwrap();
        let (mut ra, mut rb) = ([0u8; 1], [0u8; 1]);
        a.read_exact(&mut ra).await.unwrap();
        b.read_exact(&mut rb).await.unwrap();
        assert_eq!((&ra, &rb), (b"a", b"b"));
        server.await.unwrap();
    }

    /// A burst: Claude Desktop, Cursor and a Code session all connecting in
    /// the same moment. Every one must be served, none reported busy.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_burst_of_clients_connecting_at_once_is_all_served() {
        const CLIENTS: usize = 6;
        let tmp = tempfile::TempDir::new().unwrap();
        let endpoint = new_endpoint(tmp.path()).unwrap();
        let mut listener = Listener::bind(&endpoint).unwrap();

        tokio::spawn(async move {
            loop {
                let Ok(mut s) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1];
                    if s.read_exact(&mut buf).await.is_ok() {
                        let _ = s.write_all(&buf).await;
                    }
                });
            }
        });

        let clients: Vec<_> = (0..CLIENTS)
            .map(|i| {
                let endpoint = endpoint.clone();
                tokio::spawn(async move {
                    let mut c = connect(&endpoint).await.expect("served, not busy");
                    let byte = [u8::try_from(i).unwrap()];
                    c.write_all(&byte).await.unwrap();
                    let mut echo = [0u8; 1];
                    c.read_exact(&mut echo).await.unwrap();
                    assert_eq!(echo, byte);
                })
            })
            .collect();
        for c in clients {
            c.await.unwrap();
        }
    }

    /// The serve loop cancels `accept` whenever its idle tick wins. The pipe
    /// must still be there for the next relay, and that relay must be served
    /// by the next `accept`.
    #[tokio::test]
    async fn a_cancelled_accept_leaves_the_endpoint_listening() {
        let tmp = tempfile::TempDir::new().unwrap();
        let endpoint = new_endpoint(tmp.path()).unwrap();
        let mut listener = Listener::bind(&endpoint).unwrap();

        // Cancelled while waiting for a client, exactly as `select!` does.
        let cancelled =
            tokio::time::timeout(std::time::Duration::from_millis(20), listener.accept()).await;
        assert!(cancelled.is_err(), "no client yet, so accept was pending");

        let mut client = connect(&endpoint)
            .await
            .expect("a cancelled accept must not take the endpoint down");
        let mut served = listener
            .accept()
            .await
            .expect("the waiting relay is served");
        client.write_all(b"x").await.unwrap();
        let mut buf = [0u8; 1];
        served.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"x");
    }

    #[tokio::test]
    async fn nothing_listening_is_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let endpoint = new_endpoint(tmp.path()).unwrap();
        assert!(matches!(
            connect(&endpoint).await,
            Err(ConnectError::NotFound)
        ));
    }

    /// A name already owned is refused at bind — how a squatter is caught.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_taken_name_cannot_be_bound_again() {
        let tmp = tempfile::TempDir::new().unwrap();
        let endpoint = new_endpoint(tmp.path()).unwrap();
        let _owner = Listener::bind(&endpoint).unwrap();
        assert!(Listener::bind(&endpoint).is_err());
    }
}
