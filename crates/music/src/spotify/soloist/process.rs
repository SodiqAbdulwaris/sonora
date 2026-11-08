//! Finding Soloist, keeping its API key, and running it as a child process.
//!
//! Soloist is Spotify's own closed binary. Sonora never ships it: the listener downloads it from
//! Spotify, accepts Spotify's terms, makes an API key in their own developer dashboard and pairs it
//! once from a Spotify app. Sonora only finds the copy they installed and supervises it.

use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use anyhow::{Context as _, Result};

use crate::backend::{BackendStatus, Problem};
use crate::credentials;

/// The environment variable that names the Soloist executable, ahead of anywhere Sonora looks.
const EXE_VARIABLE: &str = "SONORA_SOLOIST";
/// The environment variable that carries the API key when the credential store has none, for a
/// platform without one.
const KEY_VARIABLE: &str = "SOLOIST_API_KEY";
/// Where the API key lives in the operating system's credential store.
#[cfg(any(windows, target_os = "macos"))]
const KEY_SERVICE: &str = "sonora";
#[cfg(any(windows, target_os = "macos"))]
const KEY_USER: &str = "soloist-api-key";
/// The exit code of a build that is past its 90 days.
const EXPIRED: i32 = 10;
/// How long a freshly started Soloist has to say where it listens.
const START: Duration = Duration::from_secs(20);
/// Windows: start the child without a console window.
#[cfg(windows)]
const NO_WINDOW: u32 = 0x0800_0000;

/// Everything needed to start Soloist.
pub(super) struct Settings {
    exe: PathBuf,
    key: String,
    /// Soloist's own folder for its device identity and Connect login. The files naming its
    /// WebSocket address appear here too.
    data: PathBuf,
    cache: PathBuf,
    name: String,
}

impl Settings {
    /// The installed Soloist and the API key, or what is missing.
    pub(super) fn find() -> Result<Self, Problem> {
        let exe = executable().ok_or(Problem::NoSoloist)?;
        let key = api_key().ok_or(Problem::NoKey)?;
        Ok(Self {
            exe,
            key,
            data: credentials::dir("soloist"),
            cache: credentials::root().join("soloist"),
            name: format!(
                "Sonora Soloist ({})",
                gethostname::gethostname().to_string_lossy()
            ),
        })
    }

    pub(super) fn data(&self) -> &Path {
        &self.data
    }
}

/// The Soloist executable: the one `SONORA_SOLOIST` names, one in Sonora's own `soloist` folder,
/// or one on the path.
fn executable() -> Option<PathBuf> {
    let name = format!("soloist{}", std::env::consts::EXE_SUFFIX);
    let named = std::env::var_os(EXE_VARIABLE).map(PathBuf::from);
    let installed =
        dirs::data_local_dir().map(|dir| dir.join("sonora").join("soloist").join(&name));
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join(&name));
    named
        .into_iter()
        .chain(installed)
        .chain(on_path)
        .find(|path| path.is_file())
}

/// The API key from the operating system's credential store, or from `SOLOIST_API_KEY` where the
/// platform has no store. It is never written to a file or a log.
fn api_key() -> Option<String> {
    #[cfg(any(windows, target_os = "macos"))]
    if let Some(key) = keyring::Entry::new(KEY_SERVICE, KEY_USER)
        .and_then(|entry| entry.get_password())
        .ok()
        .filter(|key| !key.is_empty())
    {
        return Some(key);
    }
    std::env::var(KEY_VARIABLE)
        .ok()
        .filter(|key| !key.is_empty())
}

/// Keeps the API key in the operating system's credential store.
#[cfg(any(windows, target_os = "macos"))]
pub fn store_key(key: &str) -> Result<()> {
    keyring::Entry::new(KEY_SERVICE, KEY_USER)
        .and_then(|entry| entry.set_password(key))
        .context("cannot store the Soloist API key")
}

/// Soloist has no builds for this platform, so there is nowhere to keep its key.
#[cfg(not(any(windows, target_os = "macos")))]
pub fn store_key(_key: &str) -> Result<()> {
    anyhow::bail!("there is no credential store to keep the Soloist API key in")
}

/// Whether an API key is set, so settings can say so without reading it back to the screen.
pub fn has_key() -> bool {
    api_key().is_some()
}

/// A running Soloist. Dropping it stops the process.
pub(super) struct Running {
    child: Child,
}

impl Running {
    /// Starts Soloist listening for its WebSocket on a port the system picks, and follows what it
    /// prints for the days its build has left.
    pub(super) fn start(settings: &Settings, status: &BackendStatus) -> Result<Self> {
        std::fs::create_dir_all(&settings.data).context("cannot create Soloist's data folder")?;
        std::fs::create_dir_all(&settings.cache).context("cannot create Soloist's cache folder")?;
        // A file left by a Soloist that was killed would point at a port nobody listens on.
        for stale in ["ws.addr", "ws.port"] {
            std::fs::remove_file(settings.data.join(stale)).ok();
        }

        let mut command = Command::new(&settings.exe);
        command
            .args(["--ws", "127.0.0.1:0", "--device-name", &settings.name])
            .arg("--data-dir")
            .arg(&settings.data)
            .arg("--cache-dir")
            .arg(&settings.cache)
            // ponytail: Soloist documents only `--api-key`, so the key is visible in its command
            // line to other processes of this user. Move it to the environment alone if Soloist
            // ever reads it from there.
            .args(["--api-key", &settings.key])
            .env(KEY_VARIABLE, &settings.key)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(NO_WINDOW);
        }
        let mut child = command.spawn().context("cannot start Soloist")?;

        if let Some(out) = child.stdout.take() {
            follow(out, settings.key.clone(), status.clone());
        }
        if let Some(err) = child.stderr.take() {
            follow(err, settings.key.clone(), status.clone());
        }
        Ok(Self { child })
    }

    /// The exit code of the process once it has stopped, which is none on a platform that signals
    /// the stop instead.
    pub(super) fn exited(&mut self) -> Option<Option<i32>> {
        self.child
            .try_wait()
            .ok()
            .flatten()
            .map(|status| status.code())
    }

    /// Waits for Soloist to say where it listens, and returns the WebSocket url.
    pub(super) async fn endpoint(&mut self, data: &Path) -> Result<String, Problem> {
        let started = std::time::Instant::now();
        loop {
            if let Some(code) = self.exited() {
                return Err(stopped(code));
            }
            let read = |name: &str| std::fs::read_to_string(data.join(name)).ok();
            if let (Some(addr), Some(port)) = (read("ws.addr"), read("ws.port"))
                && let Ok(port) = port.trim().parse::<u16>()
            {
                return Ok(format!("ws://{}:{port}", addr.trim()));
            }
            if started.elapsed() > START {
                return Err(Problem::Stopped(
                    "Soloist did not open its socket".to_owned(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

/// Why Soloist stopped, from its exit code.
pub(super) fn stopped(code: Option<i32>) -> Problem {
    match code {
        Some(EXPIRED) => Problem::Expired,
        Some(code) => Problem::Stopped(format!("Soloist exited with code {code}")),
        None => Problem::Stopped("Soloist was stopped".to_owned()),
    }
}

/// Logs what Soloist prints, without ever letting the API key into the log, and reads the days its
/// build has left from the line it starts with.
fn follow(pipe: impl std::io::Read + Send + 'static, key: String, status: BackendStatus) {
    let spawned = std::thread::Builder::new()
        .name("soloist-log".into())
        .spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                let line = line.replace(&key, "***");
                if let Some(days) = expiry(&line) {
                    status.update(|report| report.days_left = Some(days));
                }
                log::debug!("soloist: {line}");
            }
        });
    if let Err(error) = spawned {
        log::warn!("soloist: cannot follow its output: {error}");
    }
}

/// The days left in a `client expires in N days` line.
fn expiry(line: &str) -> Option<u32> {
    let rest = line.split("expires in ").nth(1)?;
    rest.split_whitespace().next()?.parse().ok()
}
