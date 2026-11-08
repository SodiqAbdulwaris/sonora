//! Which route a provider's audio takes, and what the app can show about it.
//!
//! A provider whose sound can come from more than one place, as Spotify's does, plays through the
//! first route that works. The player reports which one is in use and what stopped the ones before
//! it through a [`BackendStatus`] shared with the views, the way `Meter` is shared.

use std::sync::{Arc, Mutex};

/// A way of producing a provider's audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// Spotify's own headless Connect client, driven over a loopback WebSocket.
    Soloist,
    /// librespot decoding inside Sonora, which is the route that feeds the equalizer.
    Librespot,
}

impl Backend {
    /// The name settings and logs know the route by.
    pub fn id(self) -> &'static str {
        match self {
            Self::Soloist => "soloist",
            Self::Librespot => "librespot",
        }
    }
}

/// Which routes the listener lets a provider use, as settings store it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BackendChoice {
    /// Soloist when it is set up, then librespot, each handing over to the next when it fails.
    #[default]
    Auto,
    Soloist,
    Librespot,
}

impl BackendChoice {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Soloist, Self::Librespot];

    pub fn id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Soloist => "soloist",
            Self::Librespot => "librespot",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|choice| choice.id() == id)
    }
}

/// Why a route is not the one playing, so the listener is told what to fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// No Soloist executable was found where Sonora looks for it.
    NoSoloist,
    /// Soloist has no API key to start with.
    NoKey,
    /// Soloist is running but no Spotify app has paired with it yet.
    NeedsPairing,
    /// This Soloist build is past its 90 days.
    Expired,
    /// Soloist stopped or could not be reached.
    Stopped(String),
    /// Spotify refused the account the keys to its audio.
    Refused,
}

/// What the player knows about its routes, written by the player and read by the views.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The route playing now, or none before the first track.
    pub active: Option<Backend>,
    /// The routes passed over, each with why.
    pub skipped: Vec<(Backend, Problem)>,
    /// Days until the installed Soloist build expires, once it has said.
    pub days_left: Option<u32>,
}

/// A [`Report`] shared between the player that writes it and the views that read it.
#[derive(Clone, Debug, Default)]
pub struct BackendStatus(Arc<Mutex<Report>>);

impl BackendStatus {
    /// A copy of the report as it stands.
    pub fn get(&self) -> Report {
        self.0
            .lock()
            .map(|report| report.clone())
            .unwrap_or_default()
    }

    /// Changes the report in place.
    pub fn update(&self, change: impl FnOnce(&mut Report)) {
        if let Ok(mut report) = self.0.lock() {
            change(&mut report);
        }
    }
}

impl std::fmt::Display for Problem {
    /// The problem in English for logs and errors. The views word it from the variant instead.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSoloist => f.write_str("Soloist is not installed"),
            Self::NoKey => f.write_str("Soloist has no API key"),
            Self::NeedsPairing => f.write_str("Soloist is not paired with a Spotify app"),
            Self::Expired => f.write_str("this Soloist build has expired"),
            Self::Stopped(why) => f.write_str(why),
            Self::Refused => f.write_str("Spotify refused this account the audio keys"),
        }
    }
}

/// Keeps Soloist's API key in the operating system's credential store, where nothing else reads
/// it. It is never written to settings or to a log.
pub fn store_soloist_key(key: &str) -> anyhow::Result<()> {
    crate::spotify::store_key(key)
}

/// Whether Soloist has an API key to start with.
pub fn soloist_key_set() -> bool {
    crate::spotify::has_key()
}
