//! The shortcuts a user can change. Each [`Command`] names an action, where its keys work and its
//! factory keys; a saved map from command id to keystroke replaces them, and an empty text leaves
//! the command without a shortcut. Everything else in `bindings()` stays fixed.

use std::collections::BTreeMap;

use gpui::{KeyBinding, Keystroke};

use crate::{
    INPUT_CONTEXT, OpenFilter, OpenSearch, OpenSettings, Refresh, SongNext, SongPrevious,
    ToggleFullscreen, TogglePlayback, WORKSPACE_CONTEXT,
};

/// The saved changes, command id to keystroke text. A missing id keeps the factory keys.
pub type Overrides = BTreeMap<String, String>;

/// Where a command's keys work.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// In every context, so a key needs a modifier or it would type into a text field.
    Anywhere,
    /// Anywhere in the workspace except while a text field has the focus.
    AwayFromText,
}

/// One rebindable command.
pub struct Command {
    /// The stable id the saved map uses.
    pub id: &'static str,
    /// The Fluent key of the command's name.
    pub label: &'static str,
    scope: Scope,
    defaults: &'static [&'static str],
    bind: fn(&str, Option<&str>) -> KeyBinding,
}

/// Why a typed shortcut cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    /// Not a single keystroke such as `ctrl-shift-p`.
    Keystroke,
    /// A key with no modifier for a command that works inside text fields.
    NeedsModifier,
}

/// What to do when a wanted shortcut already belongs to another command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stance {
    /// Keep both as they are and tell the user who holds the key.
    Refuse,
    /// Give the key to the new command and leave the other one without a shortcut.
    Take,
}

macro_rules! command {
    ($id:literal, $scope:expr, [$($key:literal),+], $action:expr) => {
        Command {
            id: $id,
            label: concat!("shortcut-", $id),
            scope: $scope,
            defaults: &[$($key),+],
            bind: |keys, context| KeyBinding::new(keys, $action, context),
        }
    };
}

/// Every command whose shortcut can be changed, in the order the editor lists them.
pub const COMMANDS: &[Command] = &[
    command!(
        "toggle-playback",
        Scope::AwayFromText,
        ["space"],
        TogglePlayback
    ),
    command!("previous", Scope::AwayFromText, ["ctrl-left"], SongPrevious),
    command!("next", Scope::AwayFromText, ["ctrl-right"], SongNext),
    command!("fullscreen", Scope::AwayFromText, ["f"], ToggleFullscreen),
    command!("filter", Scope::Anywhere, ["cmd-f", "ctrl-f"], OpenFilter),
    command!(
        "search",
        Scope::Anywhere,
        ["shift-cmd-f", "shift-ctrl-f"],
        OpenSearch
    ),
    command!("refresh", Scope::Anywhere, ["cmd-r", "ctrl-r"], Refresh),
    command!(
        "settings",
        Scope::Anywhere,
        ["ctrl-,", "cmd-,"],
        OpenSettings
    ),
];

impl Command {
    /// The keystrokes this command answers to now.
    pub fn keys(&self, overrides: &Overrides) -> Vec<String> {
        match overrides.get(self.id).map(|text| text.trim()) {
            Some("") => Vec::new(),
            Some(text) => match normalise(text) {
                Ok(keys) if self.allows(&keys) => vec![keys],
                _ => self.factory(),
            },
            None => self.factory(),
        }
    }

    /// The text the editor shows for this command: its keystroke, or nothing when unbound.
    /// A command on its factory keys shows the one for this platform.
    pub fn shown(&self, overrides: &Overrides) -> String {
        match overrides.contains_key(self.id) {
            true => self.keys(overrides).join(" "),
            // the raw text is read for `cmd`, since parsing turns it into `win` off macOS
            false => self
                .defaults
                .iter()
                .find(|keys| keys.contains("cmd") == cfg!(target_os = "macos"))
                .or(self.defaults.first())
                .and_then(|keys| normalise(keys).ok())
                .unwrap_or_default(),
        }
    }

    fn factory(&self) -> Vec<String> {
        self.defaults
            .iter()
            .filter_map(|keys| normalise(keys).ok())
            .collect()
    }

    fn allows(&self, keys: &str) -> bool {
        self.scope == Scope::AwayFromText || has_modifier(keys)
    }

    fn context(&self) -> Option<String> {
        match self.scope {
            Scope::Anywhere => None,
            Scope::AwayFromText => Some(format!("{WORKSPACE_CONTEXT} && !{INPUT_CONTEXT}")),
        }
    }
}

/// The key bindings of every rebindable command, which `bindings()` adds to the fixed ones.
pub(crate) fn bindings(overrides: &Overrides) -> Vec<KeyBinding> {
    let mut all = Vec::new();
    for command in COMMANDS {
        let context = command.context();
        for keys in command.keys(overrides) {
            all.push((command.bind)(&keys, context.as_deref()));
        }
    }
    all
}

/// Reads typed text as one keystroke written the way bindings are, or says why it is not.
/// `command` decides whether a bare key is acceptable.
pub fn check(command: &Command, text: &str) -> Result<String, Invalid> {
    let keys = normalise(text.trim()).map_err(|_| Invalid::Keystroke)?;
    match command.allows(&keys) {
        true => Ok(keys),
        false => Err(Invalid::NeedsModifier),
    }
}

/// The other command that already answers to `keys`, if there is one.
pub fn holder(of: &Command, keys: &str, overrides: &Overrides) -> Option<&'static Command> {
    COMMANDS
        .iter()
        .filter(|other| other.id != of.id)
        .find(|other| other.keys(overrides).iter().any(|held| held == keys))
}

/// How to settle a clash between `wanted`, which is being rebound, and `holder`, which has the key.
pub fn stance(wanted: &Command, holder: &Command) -> Stance {
    // TODO(human): choose how a clash is settled. `Refuse` keeps things as they are and makes the
    // user free the key first; `Take` moves the key at once and leaves `holder` unbound. You may
    // also decide by scope, for example never taking a key from a command that works in text fields.
    let _ = (wanted, holder);
    Stance::Refuse
}

fn normalise(text: &str) -> Result<String, ()> {
    if text.is_empty() || text.split_whitespace().count() != 1 {
        return Err(());
    }
    Keystroke::parse(text)
        .map(|keystroke| keystroke.unparse())
        .map_err(|_| ())
}

fn has_modifier(keys: &str) -> bool {
    Keystroke::parse(keys).is_ok_and(|keystroke| keystroke.modifiers.modified())
}
