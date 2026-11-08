//! Soloist's WebSocket messages: the ones it sends that matter here, and the commands we send.
//!
//! The protocol is Spotify's (developer.spotify.com/documentation/soloist/reference/websocket-api).
//! Every frame is a JSON text frame. Unknown message types and fields are ignored, so a newer
//! Soloist that says more does not stop the old reader.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

const TRACK: &str = "spotify:track:";

/// A message from Soloist.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum Incoming {
    AuthState {
        #[serde(default)]
        logged_in: bool,
        #[serde(default)]
        is_active: bool,
    },
    PlaybackState {
        #[serde(default)]
        status: Status,
        #[serde(default)]
        item: Entity,
        position: Option<Anchor>,
        #[serde(default)]
        is_active: bool,
    },
    TrackChanged {
        #[serde(default)]
        item: Entity,
    },
    PlaybackChanged {
        #[serde(default)]
        status: Status,
    },
    DeviceChanged {
        #[serde(default)]
        is_active: bool,
    },
    PositionSync {
        position: Anchor,
    },
    Error {
        #[serde(default)]
        message: String,
    },
    #[serde(other)]
    Other,
}

/// What Soloist's player is doing.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    #[default]
    Idle,
    Playing,
    Paused,
    Buffering,
    #[serde(other)]
    Unknown,
}

/// The thing playing: a track, an episode, an ad. Only the parts Sonora reads are kept.
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct Entity {
    #[serde(default)]
    uri: String,
    #[serde(default)]
    decorations: Decorations,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Decorations {
    #[serde(default)]
    playback: Playback,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Playback {
    duration_ms: Option<f64>,
}

impl Entity {
    /// The base62 id when this is a track, which is what Sonora calls a track.
    pub(super) fn track_id(&self) -> Option<&str> {
        self.uri.strip_prefix(TRACK)
    }

    pub(super) fn duration(&self) -> Option<Duration> {
        let millis = self.decorations.playback.duration_ms?;
        (millis > 0.).then(|| Duration::from_millis(millis as u64))
    }
}

/// A position and the moment it was true, so it can be carried forward without more messages.
#[derive(Clone, Copy, Debug, Deserialize)]
pub(super) struct Anchor {
    position_ms: f64,
    timestamp_ms: f64,
    #[serde(default)]
    speed: f64,
}

impl Anchor {
    /// Where the track is at `now_ms` on the wall clock, which Soloist's timestamps share because
    /// it runs on this machine.
    pub(super) fn at(&self, now_ms: u64) -> Duration {
        let elapsed = (now_ms as f64 - self.timestamp_ms).max(0.);
        let millis = self.position_ms + elapsed * self.speed.max(0.);
        Duration::from_millis(millis.max(0.) as u64)
    }

    /// Whether the position is moving.
    pub(super) fn running(&self) -> bool {
        self.speed > 0.
    }
}

/// The `spotify:track:` uri of a track id.
pub(super) fn uri(id: &str) -> String {
    format!("{TRACK}{id}")
}

fn command(name: &str, fields: Value) -> String {
    let mut message = json!({ "type": "command", "command": name });
    if let (Some(message), Value::Object(fields)) = (message.as_object_mut(), fields) {
        message.extend(fields);
    }
    message.to_string()
}

/// Resumes, or starts `track` when one is named.
pub(super) fn play(track: Option<&str>) -> String {
    match track {
        Some(id) => command("play", json!({ "uri": uri(id) })),
        None => command("play", json!({})),
    }
}

pub(super) fn pause() -> String {
    command("pause", json!({}))
}

pub(super) fn seek(to: Duration) -> String {
    command("seek", json!({ "position_ms": to.as_millis() as u64 }))
}

pub(super) fn volume(percent: u8) -> String {
    command("set_volume", json!({ "volume": percent }))
}

/// Queues a track behind the one playing, which is how a gapless join is lined up.
pub(super) fn enqueue(id: &str) -> String {
    command("add_to_queue", json!({ "uri": uri(id) }))
}

/// Makes this Soloist the Spotify Connect device that plays.
pub(super) fn activate() -> String {
    command("activate", json!({}))
}

/// Turns off Soloist's own repeat so a track that ends does not start over behind Sonora's queue.
pub(super) fn no_repeat() -> [String; 2] {
    [
        command("set_repeat_track", json!({ "enabled": false })),
        command("set_repeat_context", json!({ "enabled": false })),
    ]
}
