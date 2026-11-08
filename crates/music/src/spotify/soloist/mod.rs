//! Spotify played by Soloist, Spotify's own headless Connect client.
//!
//! librespot cannot get the audio keys some accounts need, and Soloist, being Spotify's own, can.
//! It plays through the system's default output and gives no samples, so the equalizer, the
//! spectrum and Sonora's loudness never touch this audio. Sonora starts it as a child process and
//! controls it over a loopback WebSocket. See `process` for what Sonora does and does not ship.
//!
//! The player here runs on its own thread, with its own small runtime, so it can be started from
//! anywhere. `Tracker` holds every decision; this file only moves bytes.

mod process;
mod tracker;
mod wire;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::{SinkExt as _, StreamExt as _};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::Message;

use crate::backend::{BackendStatus, Problem};
use crate::{PlaybackConfig, PlaybackEvent, PlaybackEvents, Player};
use process::{Running, Settings};
use tracker::{Cmd, Fault, Step, Tracker};

pub(crate) use process::{has_key, store_key};

/// How long Soloist has to accept the WebSocket once it says where it listens.
const CONNECT: Duration = Duration::from_secs(5);
/// How often the child is checked for having exited.
const WATCH: Duration = Duration::from_millis(500);
/// The decibel span of the volume taper `state::Playback` turns the volume slider into a gain
/// with. Soloist takes a percentage, so the gain is turned back into the slider's position.
const TAPER_DB: f32 = 50.;

/// A started Soloist: the player, its events, and the reasons it may give up.
pub(crate) struct Started {
    pub player: Box<dyn Player>,
    pub events: Box<dyn PlaybackEvents>,
    /// Sends the reason Soloist can no longer be used, once.
    pub faults: UnboundedReceiver<Problem>,
}

/// Starts Soloist, or says what stops it. The process comes up on the thread this starts, so the
/// first track waits for it and a failure arrives through `faults`.
pub(crate) fn start(config: &PlaybackConfig, status: &BackendStatus) -> Result<Started, Problem> {
    let settings = Settings::find()?;
    let (commands, command_rx) = unbounded_channel();
    let (events_tx, events_rx) = unbounded_channel();
    let (fault_tx, faults) = unbounded_channel();
    let interval = config.position_interval;
    let volume = percent(config.gain);
    let status = status.clone();

    let spawned = std::thread::Builder::new()
        .name("soloist".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let result = match runtime {
                Ok(runtime) => runtime.block_on(run(
                    settings, status, interval, volume, command_rx, events_tx,
                )),
                Err(error) => Err(Problem::Stopped(format!("cannot start a runtime: {error}"))),
            };
            if let Err(problem) = result {
                fault_tx.send(problem).ok();
            }
        });
    spawned
        .map_err(|error| Problem::Stopped(format!("cannot start the Soloist thread: {error}")))?;

    Ok(Started {
        player: Box::new(SoloistPlayer { commands }),
        events: Box::new(Events(events_rx)),
        faults,
    })
}

/// The Soloist side of `Player`. Every call queues a command for the thread.
struct SoloistPlayer {
    commands: UnboundedSender<Cmd>,
}

impl SoloistPlayer {
    fn send(&self, cmd: Cmd) -> Result<()> {
        self.commands
            .send(cmd)
            .map_err(|_| anyhow!("Soloist has stopped"))
    }
}

impl Player for SoloistPlayer {
    fn load(&self, track_id: &str, at: Duration, seamless: bool) -> Result<()> {
        self.send(Cmd::Load {
            id: track_id.to_owned(),
            at,
            seamless,
        })
    }

    fn load_paused_at(&self, track_id: &str, at: Duration) -> Result<()> {
        self.send(Cmd::Hold {
            id: track_id.to_owned(),
            at,
        })
    }

    /// Only the next queue item is lined up, since Soloist has no way to take a track back out of
    /// its queue once it is there.
    fn preload(&self, track_id: &str, segue: bool) -> Result<()> {
        match segue {
            true => self.send(Cmd::Preload {
                id: track_id.to_owned(),
            }),
            false => Ok(()),
        }
    }

    fn play(&self) {
        self.send(Cmd::Play).ok();
    }

    fn pause(&self) {
        self.send(Cmd::Pause).ok();
    }

    fn seek(&self, position: Duration) {
        self.send(Cmd::Seek(position)).ok();
    }

    fn set_gain(&self, gain: f32) {
        self.send(Cmd::Volume(percent(gain))).ok();
    }
}

struct Events(UnboundedReceiver<PlaybackEvent>);

#[async_trait]
impl PlaybackEvents for Events {
    async fn next(&mut self) -> Option<PlaybackEvent> {
        self.0.recv().await
    }
}

/// The volume slider's position for a linear gain, from 0 to 100. It undoes the taper
/// `state::Playback` applies, so the slider means the same on every route.
fn percent(gain: f32) -> u8 {
    match gain <= 0. {
        true => 0,
        false => ((1. + 20. * gain.log10() / TAPER_DB).clamp(0., 1.) * 100.).round() as u8,
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

type Socket = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

/// Runs Soloist until the player is dropped or Soloist gives out, and says why in the latter case.
async fn run(
    settings: Settings,
    status: BackendStatus,
    interval: Duration,
    volume: u8,
    mut commands: UnboundedReceiver<Cmd>,
    events: UnboundedSender<PlaybackEvent>,
) -> Result<(), Problem> {
    let stopped = |error: &dyn std::fmt::Display| Problem::Stopped(error.to_string());
    let mut child =
        Running::start(&settings, &status).map_err(|error| stopped(&format!("{error:#}")))?;
    let url = child.endpoint(settings.data()).await?;
    let (socket, _) = tokio::time::timeout(CONNECT, tokio_tungstenite::connect_async(&url))
        .await
        .map_err(|_| stopped(&"Soloist did not accept the connection"))?
        .map_err(|error| stopped(&error))?;
    let (mut sink, mut stream) = socket.split();
    log::info!("playback: soloist listens on {url}");

    let mut tracker = Tracker::default();
    let mut ticks = tokio::time::interval(interval);
    let mut watch = tokio::time::interval(WATCH);
    let first = tracker.command(Cmd::Volume(volume), now());
    flush(first, &mut sink, &events).await?;

    loop {
        let step = tokio::select! {
            frame = stream.next() => match frame {
                Some(Ok(Message::Text(text))) => match serde_json::from_str(text.as_str()) {
                    Ok(message) => tracker.message(message, now()),
                    Err(error) => {
                        log::debug!("soloist: unreadable message: {error}");
                        continue;
                    }
                },
                Some(Ok(_)) => continue,
                Some(Err(error)) => return Err(stopped(&error)),
                None => return Err(stopped(&"Soloist closed the connection")),
            },
            command = commands.recv() => match command {
                Some(command) => tracker.command(command, now()),
                None => return Ok(()),
            },
            _ = ticks.tick() => tracker.tick(now()),
            _ = watch.tick() => match child.exited() {
                Some(code) => return Err(process::stopped(code)),
                None => continue,
            },
        };
        flush(step, &mut sink, &events).await?;
    }
}

/// Sends what a step produced: its frames to Soloist and its events up to the player.
async fn flush(
    step: Step,
    sink: &mut Socket,
    events: &UnboundedSender<PlaybackEvent>,
) -> Result<(), Problem> {
    for frame in step.wire {
        sink.send(Message::text(frame))
            .await
            .map_err(|error| Problem::Stopped(error.to_string()))?;
    }
    for event in step.events {
        events.send(event).ok();
    }
    match step.fault {
        Some(Fault::NotPaired) => Err(Problem::NeedsPairing),
        None => Ok(()),
    }
}
