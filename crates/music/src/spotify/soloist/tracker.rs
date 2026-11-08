//! What Soloist's messages mean for Sonora's playback contract, with no sockets or threads.
//!
//! Sonora's player contract asks for things Soloist does not say outright: `Playing` means audio
//! from `at` is audible, `Seeked` means audio from there has arrived, and `Ended` means the track
//! is over. Soloist has no event for the last of these and only reports positions for the others.
//! `Tracker` turns the one into the other, taking commands and messages in and handing back the
//! events to send up and the frames to send down.

use std::time::Duration;

use super::wire::{self, Anchor, Entity, Incoming, Status};
use crate::{Normalisation, PlaybackEvent, TrackSignal};

/// How far from the place a load or seek asked for the position may be and still count as there.
const LANDED: Duration = Duration::from_secs(1);
/// How long a load or seek may take to produce audio before it is given up on. Soloist took
/// 9.35 s once to buffer a seek far into a track it had not downloaded.
const PATIENCE_MS: u64 = 12_000;
/// How close to the end of a track a stop must come to be the track ending.
const NEAR_END: Duration = Duration::from_millis(1500);
/// How long a started Soloist has to log in before it is called unpaired. It takes a couple of
/// seconds to come up, and says it is not logged in until it has.
const LOGIN_PATIENCE_MS: u64 = 12_000;

/// What the player asks for. Each is the Soloist side of one `Player` method.
#[derive(Debug)]
pub(super) enum Cmd {
    /// Plays a track from `at`. `seamless` marks the queue's own step to the next track.
    Load {
        id: String,
        at: Duration,
        seamless: bool,
    },
    /// Readies a track paused at `at`. Nothing is sent to Soloist until `Play`, so restoring the
    /// last session at launch makes no sound.
    Hold {
        id: String,
        at: Duration,
    },
    /// Lines a track up behind the one playing.
    Preload {
        id: String,
    },
    Play,
    Pause,
    Seek(Duration),
    /// A volume from 0 to 100.
    Volume(u8),
}

/// What stops Soloist playing, which may pass once the listener has put it right.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Fault {
    /// No Spotify app has paired with it, or the pairing was lost.
    NotPaired,
}

/// What one input produced.
#[derive(Debug, Default)]
pub(super) struct Step {
    pub events: Vec<PlaybackEvent>,
    pub wire: Vec<String>,
    pub fault: Option<Fault>,
}

/// Whether a landing waits for the track to start or for a seek to arrive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Play,
    Seek,
}

/// Audio that has been asked for and not yet heard, and where.
#[derive(Debug)]
struct Landing {
    id: String,
    at: Duration,
    kind: Kind,
    deadline: u64,
}

#[derive(Debug, Default)]
pub(super) struct Tracker {
    /// The track Sonora asked for and has not been told has ended.
    want: Option<String>,
    /// The track Soloist is on, which runs ahead of `want` when it moves on by itself.
    item: Option<String>,
    /// The track lined up behind the current one.
    queued: Option<String>,
    status: Status,
    anchor: Option<Anchor>,
    duration: Duration,
    /// Whether this Soloist is the Connect device that plays.
    active: bool,
    /// A track readied paused, with where it resumes.
    held: Option<(String, Duration)>,
    landing: Option<Landing>,
    /// The seek a load from the middle of a track sends once the track is up.
    start_at: Option<Duration>,
    /// Whether `Ended` has gone up for `want`.
    ended: bool,
    /// The track `Length` was last sent for.
    length: Option<String>,
    /// Whether Soloist's own repeat has been switched off.
    repeat_off: bool,
    /// Whether the last thing said up about `want` was that it plays, so a repeated state from
    /// Soloist is not said twice.
    playing: bool,
    /// Whether Soloist has logged in, which every control command needs.
    signed_in: bool,
    /// When this tracker first heard from anyone, which the wait for a login counts from.
    since: Option<u64>,
    /// Whether the fault for a missing login has been raised.
    unpaired_said: bool,
    /// The load asked for before Soloist logged in, sent once it has.
    pending: Option<Cmd>,
    /// The volume asked for before Soloist logged in.
    volume: Option<u8>,
}

impl Tracker {
    pub(super) fn command(&mut self, cmd: Cmd, now: u64) -> Step {
        let mut out = Step::default();
        self.since.get_or_insert(now);
        if !self.signed_in && !matches!(cmd, Cmd::Hold { .. }) {
            self.unsigned(cmd);
            return out;
        }
        match cmd {
            Cmd::Load { id, at, seamless } => self.load(id, at, seamless, now, &mut out),
            Cmd::Hold { id, at } => {
                self.want = None;
                self.landing = None;
                self.ended = false;
                out.events.push(PlaybackEvent::Paused {
                    id: Some(id.clone()),
                    at,
                });
                self.held = Some((id, at));
            }
            Cmd::Preload { id } => {
                if self.item.as_deref() != Some(id.as_str())
                    && self.queued.as_deref() != Some(id.as_str())
                {
                    out.wire.push(wire::enqueue(&id));
                    self.queued = Some(id);
                }
            }
            Cmd::Play => match self.held.take() {
                Some((id, at)) => self.load(id, at, false, now, &mut out),
                None => out.wire.push(wire::play(None)),
            },
            Cmd::Pause => out.wire.push(wire::pause()),
            Cmd::Seek(to) => match (&mut self.held, &self.want) {
                (Some((id, at)), _) => {
                    *at = to;
                    out.events.push(PlaybackEvent::Seeked {
                        id: Some(id.clone()),
                        at: to,
                    });
                }
                (None, Some(id)) => {
                    self.landing = Some(Landing {
                        id: id.clone(),
                        at: to,
                        kind: Kind::Seek,
                        deadline: now + PATIENCE_MS,
                    });
                    out.wire.push(wire::seek(to));
                }
                (None, None) => {}
            },
            Cmd::Volume(percent) => out.wire.push(wire::volume(percent)),
        }
        out
    }

    pub(super) fn message(&mut self, message: Incoming, now: u64) -> Step {
        let mut out = Step::default();
        self.since.get_or_insert(now);
        match message {
            Incoming::AuthState {
                logged_in,
                is_active,
            } => {
                self.active = is_active;
                match (logged_in, self.signed_in) {
                    (true, false) => {
                        self.signed_in = true;
                        self.unpaired_said = false;
                        if let Some(percent) = self.volume.take() {
                            out.wire.push(wire::volume(percent));
                        }
                        if let Some(cmd) = self.pending.take() {
                            let sent = self.command(cmd, now);
                            out.events.extend(sent.events);
                            out.wire.extend(sent.wire);
                        }
                    }
                    (false, true) => {
                        self.signed_in = false;
                        self.unpaired_said = true;
                        out.fault = Some(Fault::NotPaired);
                    }
                    _ => {}
                }
            }
            Incoming::DeviceChanged { is_active } => self.active = is_active,
            Incoming::PlaybackState {
                status,
                item,
                position,
                is_active,
            } => {
                self.active = is_active;
                self.status = status;
                self.anchor = position.or(self.anchor);
                self.change_item(&item, &mut out);
                self.on_status(now, &mut out);
            }
            Incoming::TrackChanged { item } => self.change_item(&item, &mut out),
            Incoming::PlaybackChanged { status } => {
                self.status = status;
                self.on_status(now, &mut out);
            }
            Incoming::PositionSync { position } => self.anchor = Some(position),
            Incoming::Error { message } => self.rejected(&message, &mut out),
            Incoming::Other => {}
        }
        self.settle(now, &mut out);
        out
    }

    /// The clock's turn: position reports while playing, and the give-up for a landing that
    /// never came.
    pub(super) fn tick(&mut self, now: u64) -> Step {
        let mut out = Step::default();
        let since = *self.since.get_or_insert(now);
        if !self.signed_in && !self.unpaired_said && now.saturating_sub(since) > LOGIN_PATIENCE_MS {
            self.unpaired_said = true;
            // a track asked for then must not start by itself once the pairing is done
            self.pending = None;
            out.fault = Some(Fault::NotPaired);
        }
        if let Some(landing) = self.landing.take_if(|landing| now >= landing.deadline) {
            log::warn!("soloist: audio from {:?} never arrived", landing.at);
            out.events.push(match landing.kind {
                Kind::Play => PlaybackEvent::Unavailable {
                    id: Some(landing.id),
                },
                Kind::Seek => PlaybackEvent::Seeked {
                    id: Some(landing.id),
                    at: landing.at,
                },
            });
        }
        if self.landing.is_none()
            && self.status == Status::Playing
            && let Some(id) = self.live()
        {
            out.events.push(PlaybackEvent::Position {
                id: Some(id),
                at: self.position(now),
            });
        }
        out
    }

    /// A command that arrives before Soloist has logged in. A load waits for the login, as does
    /// the volume, and nothing else can be done yet.
    fn unsigned(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Load { id, at, .. } => {
                self.pending = Some(Cmd::Load {
                    id,
                    at,
                    seamless: false,
                });
            }
            Cmd::Play => {
                if let Some((id, at)) = self.held.take() {
                    self.pending = Some(Cmd::Load {
                        id,
                        at,
                        seamless: false,
                    });
                }
            }
            Cmd::Volume(percent) => self.volume = Some(percent),
            Cmd::Hold { .. } | Cmd::Preload { .. } | Cmd::Pause | Cmd::Seek(_) => {}
        }
    }

    fn load(&mut self, id: String, at: Duration, seamless: bool, now: u64, out: &mut Step) {
        self.held = None;
        self.ended = false;
        self.playing = false;
        self.want = Some(id.clone());
        out.events.push(PlaybackEvent::Signal {
            id: id.clone(),
            track: TrackSignal {
                encoding: None,
                rate: 44_100,
                channels: 2,
                // Soloist normalises loudness on its own, and says by how much to no one.
                normalisation: Normalisation::Limited,
            },
        });
        out.events.push(PlaybackEvent::Loading {
            id: Some(id.clone()),
            at,
        });

        // Soloist moved onto this track itself, the join a queued track was lined up for.
        if seamless && at.is_zero() && self.item.as_deref() == Some(id.as_str()) {
            match self.status {
                Status::Playing => {
                    self.playing = true;
                    out.events.push(PlaybackEvent::Playing {
                        id: Some(id),
                        at: self.position(now),
                    });
                }
                _ => {
                    self.landing = Some(Landing {
                        id,
                        at: self.position(now),
                        kind: Kind::Play,
                        deadline: now + PATIENCE_MS,
                    });
                    out.wire.push(wire::play(None));
                }
            }
            return;
        }

        if !self.active {
            out.wire.push(wire::activate());
        }
        if !self.repeat_off {
            out.wire.extend(wire::no_repeat());
            self.repeat_off = true;
        }
        out.wire.push(wire::play(Some(&id)));
        self.queued = None;
        self.start_at = (!at.is_zero()).then_some(at);
        self.landing = Some(Landing {
            id,
            at,
            kind: Kind::Play,
            deadline: now + PATIENCE_MS,
        });
    }

    /// Soloist is on `item` now. When it left the track Sonora was playing without being told to,
    /// that track ended, and if the new one is not what was lined up it is Spotify's own autoplay,
    /// which is stopped before it is heard.
    fn change_item(&mut self, item: &Entity, out: &mut Step) {
        let id = item.track_id().map(str::to_owned);
        if id == self.item {
            self.send_length(out);
            return;
        }
        let previous = std::mem::replace(&mut self.item, id.clone());
        self.duration = item.duration().unwrap_or_default();
        if previous.is_some() && previous == self.want && id != self.want && !self.ended {
            out.events.push(PlaybackEvent::Ended {
                id: previous.clone(),
            });
            self.ended = true;
            self.playing = false;
            match id.is_some() && id == self.queued {
                true => self.queued = None,
                false => {
                    log::debug!("soloist: stopping a track Sonora did not queue");
                    out.wire.push(wire::pause());
                }
            }
        }
        self.send_length(out);
    }

    fn on_status(&mut self, now: u64, out: &mut Step) {
        let Some(id) = self.live() else { return };
        if self.landing.is_some() {
            return;
        }
        let at = self.position(now);
        let finished = !self.duration.is_zero() && at + NEAR_END >= self.duration;
        match self.status {
            Status::Idle | Status::Paused if finished => {
                self.ended = true;
                self.playing = false;
                out.events.push(PlaybackEvent::Ended { id: Some(id) });
            }
            Status::Paused if self.playing => {
                self.playing = false;
                out.events.push(PlaybackEvent::Paused { id: Some(id), at });
            }
            // Playback resumed after a pause, which no load or seek is waiting on.
            Status::Playing if !self.playing => {
                self.playing = true;
                out.events.push(PlaybackEvent::Playing { id: Some(id), at });
            }
            _ => {}
        }
    }

    /// Announces `Playing` or `Seeked` once audio is where a load or seek asked for, and sends the
    /// seek a load from the middle of a track was waiting to send.
    fn settle(&mut self, now: u64, out: &mut Step) {
        let Some(id) = self.live() else { return };
        self.send_length(out);
        let audible = self.status == Status::Playing && self.anchor.is_some_and(|a| a.running());
        if audible && let Some(at) = self.start_at.take() {
            out.wire.push(wire::seek(at));
            return;
        }
        let Some(landing) = &self.landing else { return };
        if landing.id != id {
            return;
        }
        let here = self.position(now);
        let ready = match landing.kind {
            Kind::Play => audible,
            Kind::Seek => audible || self.status == Status::Paused,
        };
        if ready && here.abs_diff(landing.at) <= LANDED {
            self.playing = audible;
            out.events.push(match landing.kind {
                Kind::Play => PlaybackEvent::Playing {
                    id: Some(id),
                    at: here,
                },
                Kind::Seek => PlaybackEvent::Seeked {
                    id: Some(id),
                    at: here,
                },
            });
            self.landing = None;
        }
    }

    /// Soloist refused a command. While a track is loading that is the track failing.
    fn rejected(&mut self, message: &str, out: &mut Step) {
        log::warn!("soloist: {message}");
        if let Some(landing) = self.landing.take_if(|landing| landing.kind == Kind::Play) {
            out.events.push(PlaybackEvent::Unavailable {
                id: Some(landing.id),
            });
        }
    }

    fn send_length(&mut self, out: &mut Step) {
        let Some(id) = self.live() else { return };
        if self.length.as_deref() != Some(id.as_str()) && !self.duration.is_zero() {
            out.events.push(PlaybackEvent::Length {
                id: Some(id.clone()),
                duration: self.duration,
            });
            self.length = Some(id);
        }
    }

    /// The track both Sonora and Soloist are on, while it has not ended.
    fn live(&self) -> Option<String> {
        match (&self.want, &self.item) {
            (Some(want), Some(item)) if want == item && !self.ended => Some(want.clone()),
            _ => None,
        }
    }

    fn position(&self, now: u64) -> Duration {
        self.anchor.map(|anchor| anchor.at(now)).unwrap_or_default()
    }
}
