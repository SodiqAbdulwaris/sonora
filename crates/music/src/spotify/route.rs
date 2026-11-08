//! Spotify's audio from the first route that works.
//!
//! The routes are tried in order: Soloist, when it is installed and set up, then librespot. All of
//! them that open are kept running side by side, so when the one in use gives out the next takes
//! over with the track it was playing, and so downloads, which only librespot can make, always have
//! a player to ask. `BackendStatus` tells the views which route is in use and why the others
//! are not.

use std::future::pending;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::future::BoxFuture;
use librespot_core::Session;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::backend::{Backend, BackendChoice, BackendStatus, Problem};
use crate::spectrum::Spectrum;
use crate::spotify::playback::Factory;
use crate::spotify::soloist;
use crate::{Fraction, PlaybackConfig, PlaybackEvent, PlaybackEvents, PlaybackFactory, Player};

/// The routes a choice allows, in the order they are tried.
fn order(choice: BackendChoice) -> &'static [Backend] {
    match choice {
        BackendChoice::Auto => &[Backend::Soloist, Backend::Librespot],
        BackendChoice::Soloist => &[Backend::Soloist],
        BackendChoice::Librespot => &[Backend::Librespot],
    }
}

/// Builds Spotify's players on a connected session.
pub struct Routes(Session);

impl Routes {
    pub fn new(session: Session) -> Self {
        Self(session)
    }
}

impl PlaybackFactory for Routes {
    fn start(&self, config: PlaybackConfig) -> (Box<dyn Player>, Box<dyn PlaybackEvents>) {
        let mut players: Vec<Box<dyn Player>> = Vec::new();
        let mut events: Vec<Box<dyn PlaybackEvents>> = Vec::new();
        let mut faults: Vec<Option<UnboundedReceiver<Problem>>> = Vec::new();
        let mut backends = Vec::new();
        let mut skipped = Vec::new();

        for backend in order(config.backend) {
            match backend {
                Backend::Soloist => match soloist::start(&config, &config.status) {
                    Ok(started) => {
                        players.push(started.player);
                        events.push(started.events);
                        faults.push(Some(started.faults));
                        backends.push(*backend);
                    }
                    Err(problem) => {
                        log::info!("playback: soloist is not used: {problem}");
                        skipped.push((*backend, problem));
                    }
                },
                Backend::Librespot => {
                    let (player, stream) = Factory::new(self.0.clone()).start(config.clone());
                    players.push(player);
                    events.push(stream);
                    faults.push(None);
                    backends.push(*backend);
                }
            }
        }

        if players.is_empty() {
            let problem = skipped
                .last()
                .map_or(Problem::NoSoloist, |(_, problem)| problem.clone());
            config.status.update(|report| {
                report.active = None;
                report.skipped = skipped;
            });
            return (Box::new(Dead(problem)), Box::new(Silent));
        }

        config.status.update(|report| {
            report.active = backends.first().copied();
            report.skipped = skipped;
        });
        log::info!("playback: spotify plays through {}", backends[0].id());

        let shared = Arc::new(Shared {
            players,
            active: AtomicUsize::new(0),
            last: Mutex::new(Last {
                gain: config.gain,
                ..Last::default()
            }),
        });
        (
            Box::new(RoutePlayer(shared.clone())),
            Box::new(RouteEvents {
                shared,
                events,
                faults,
                backends,
                status: config.status,
            }),
        )
    }
}

/// What a switch of route needs to carry the listener over: the track, where it was, and whether
/// it was playing.
#[derive(Default)]
struct Last {
    id: Option<String>,
    at: Duration,
    playing: bool,
    gain: f32,
}

/// The players of every route that opened, and which one is in use.
struct Shared {
    players: Vec<Box<dyn Player>>,
    active: AtomicUsize,
    last: Mutex<Last>,
}

impl Shared {
    fn active(&self) -> &dyn Player {
        &*self.players[self.active.load(Ordering::Acquire)]
    }

    fn remember(&self, change: impl FnOnce(&mut Last)) {
        if let Ok(mut last) = self.last.lock() {
            change(&mut last);
        }
    }
}

/// A `Player` that forwards to whichever route is in use.
struct RoutePlayer(Arc<Shared>);

impl Player for RoutePlayer {
    fn load(&self, track_id: &str, at: Duration, seamless: bool) -> Result<()> {
        self.0.remember(|last| {
            last.id = Some(track_id.to_owned());
            last.at = at;
            last.playing = true;
        });
        self.0.active().load(track_id, at, seamless)
    }

    fn load_paused_at(&self, track_id: &str, at: Duration) -> Result<()> {
        self.0.remember(|last| {
            last.id = Some(track_id.to_owned());
            last.at = at;
            last.playing = false;
        });
        self.0.active().load_paused_at(track_id, at)
    }

    fn preload(&self, track_id: &str, segue: bool) -> Result<()> {
        self.0.active().preload(track_id, segue)
    }

    fn play(&self) {
        self.0.remember(|last| last.playing = true);
        self.0.active().play();
    }

    fn pause(&self) {
        self.0.remember(|last| last.playing = false);
        self.0.active().pause();
    }

    fn seek(&self, position: Duration) {
        self.0.remember(|last| last.at = position);
        self.0.active().seek(position);
    }

    fn set_gain(&self, gain: f32) {
        self.0.remember(|last| last.gain = gain);
        // Every route hears it, so the one that takes over is already at the right level.
        self.0
            .players
            .iter()
            .for_each(|player| player.set_gain(gain));
    }

    fn spectrum(&self) -> Option<Spectrum> {
        self.0.active().spectrum()
    }

    /// Downloads go to the last route, which is librespot whenever it is allowed, since nothing
    /// else makes them.
    fn download(&self, track_id: &str, progress: Fraction) -> BoxFuture<'static, Result<()>> {
        self.0.players.last().map_or_else(
            || Box::pin(async { Err(anyhow!("no player can download")) }) as BoxFuture<_>,
            |player| player.download(track_id, progress),
        )
    }

    fn downloaded(&self, track_id: &str) -> bool {
        self.0
            .players
            .last()
            .is_some_and(|player| player.downloaded(track_id))
    }

    fn plays_offline(&self, track_id: &str) -> bool {
        self.0
            .players
            .last()
            .is_some_and(|player| player.plays_offline(track_id))
    }

    fn discard(&self, track_id: &str) {
        if let Some(player) = self.0.players.last() {
            player.discard(track_id);
        }
    }
}

/// The events of whichever route is in use, handing over to the next when it fails.
struct RouteEvents {
    shared: Arc<Shared>,
    events: Vec<Box<dyn PlaybackEvents>>,
    faults: Vec<Option<UnboundedReceiver<Problem>>>,
    backends: Vec<Backend>,
    status: BackendStatus,
}

#[async_trait]
impl PlaybackEvents for RouteEvents {
    async fn next(&mut self) -> Option<PlaybackEvent> {
        loop {
            let index = self.shared.active.load(Ordering::Acquire);
            let last = index + 1 >= self.events.len();
            tokio::select! {
                event = self.events[index].next() => match event {
                    // librespot being denied the audio keys is the cue to try another route.
                    Some(PlaybackEvent::Refused) if !last => self.hand_over(Problem::Refused),
                    Some(event) => {
                        self.follow(&event);
                        return Some(event);
                    }
                    None if !last => {
                        self.hand_over(Problem::Stopped("the player ended".to_owned()));
                    }
                    None => return None,
                },
                problem = fault(&mut self.faults[index]) => match last {
                    true => {
                        log::warn!("playback: {} stopped: {problem}", self.backends[index].id());
                        self.status.update(|report| {
                            report.skipped.push((self.backends[index], problem));
                        });
                        // Nothing is left to hand over to, so the track in hand cannot play.
                        return Some(PlaybackEvent::Unavailable { id: self.shared.id() });
                    }
                    false => self.hand_over(problem),
                },
            }
        }
    }
}

impl RouteEvents {
    /// Moves to the next route and starts the track the last one was on, from where it was.
    fn hand_over(&mut self, problem: Problem) {
        let from = self.shared.active.load(Ordering::Acquire);
        let to = from + 1;
        log::warn!(
            "playback: {} gave out ({problem}), moving to {}",
            self.backends[from].id(),
            self.backends[to].id()
        );
        self.status.update(|report| {
            report.skipped.push((self.backends[from], problem));
            report.active = Some(self.backends[to]);
        });
        self.shared.active.store(to, Ordering::Release);
        let (id, at, playing) = self
            .shared
            .last
            .lock()
            .map(|last| (last.id.clone(), last.at, last.playing))
            .unwrap_or_default();
        let player = &self.shared.players[to];
        if let Some(id) = id {
            let loaded = match playing {
                true => player.load(&id, at, false),
                false => player.load_paused_at(&id, at),
            };
            if let Err(error) = loaded {
                log::warn!("playback: cannot carry {id} over: {error:#}");
            }
        }
    }

    /// Keeps the track and place a later hand-over would resume from.
    fn follow(&self, event: &PlaybackEvent) {
        self.shared.remember(|last| match event {
            PlaybackEvent::Position { id, at }
            | PlaybackEvent::Playing { id, at }
            | PlaybackEvent::Seeked { id, at }
            | PlaybackEvent::Paused { id, at }
                if *id == last.id =>
            {
                last.at = *at;
            }
            PlaybackEvent::Ended { .. } | PlaybackEvent::Unavailable { .. } => last.id = None,
            _ => {}
        });
    }
}

impl Shared {
    fn id(&self) -> Option<String> {
        self.last.lock().ok().and_then(|last| last.id.clone())
    }
}

/// The next reason a route gives for giving out, or never when it has none to give.
async fn fault(faults: &mut Option<UnboundedReceiver<Problem>>) -> Problem {
    match faults {
        Some(faults) => match faults.recv().await {
            Some(problem) => problem,
            None => pending().await,
        },
        None => pending().await,
    }
}

/// What plays when no route opened: every load says why not.
struct Dead(Problem);

impl Player for Dead {
    fn load(&self, _track_id: &str, _at: Duration, _seamless: bool) -> Result<()> {
        Err(anyhow!("{}", self.0))
    }

    fn load_paused_at(&self, _track_id: &str, _at: Duration) -> Result<()> {
        Ok(())
    }

    fn preload(&self, _track_id: &str, _segue: bool) -> Result<()> {
        Ok(())
    }

    fn play(&self) {}

    fn pause(&self) {}

    fn seek(&self, _position: Duration) {}

    fn set_gain(&self, _gain: f32) {}
}

/// The events of [`Dead`]: none, ever.
struct Silent;

#[async_trait]
impl PlaybackEvents for Silent {
    async fn next(&mut self) -> Option<PlaybackEvent> {
        pending().await
    }
}
