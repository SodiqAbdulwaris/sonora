use gpui::prelude::*;
use gpui::{AnyElement, App, Context, Entity, SharedString};
use i18n::t;
use input::{COMMANDS, Invalid, Stance};
use state::{AppSettings, Outcome, Toasts};
use ui::{Button, Input};

use super::{Group, SettingsView, Slot};

impl SettingsView {
    /// The group for changing the keyboard shortcuts: a field for each command in
    /// `input::COMMANDS` and a button that puts them all back.
    pub(super) fn shortcut_groups(&self) -> Vec<Group> {
        let slots = (0..COMMANDS.len()).map(Slot::Shortcut);
        vec![
            Group::new("settings-group-shortcuts", "icons/list.svg")
                .rows(slots.chain([Slot::ShortcutsReset])),
        ]
    }

    /// The name and the hint of the command at `index`, or `None` when there is no such command.
    pub(super) fn shortcut_meta(index: usize) -> Option<(SharedString, SharedString)> {
        let command = COMMANDS.get(index)?;
        Some((
            i18n::lookup(command.label, None),
            t!("settings-shortcut-detail"),
        ))
    }

    /// One field per command, holding the keystroke it answers to now.
    pub(super) fn shortcut_inputs(
        settings: &Entity<AppSettings>,
        cx: &mut Context<Self>,
    ) -> Vec<Entity<Input>> {
        COMMANDS
            .iter()
            .map(|command| {
                let field = cx.new(|cx| Input::new("settings-shortcut-hint", cx).compact());
                let shown = command.shown(settings.read(cx).shortcuts());
                field.update(cx, |input, cx| input.set_text(shown, cx));
                field
            })
            .collect()
    }

    pub(super) fn command_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let (Some(command), Some(field)) = (COMMANDS.get(index), self.shortcuts.get(index)) else {
            return gpui::div().into_any_element();
        };
        let settings = self.settings.clone();
        let field = field.clone();
        let shown = field.clone();

        self.shortcut_fields(
            i18n::lookup(command.label, None),
            COMMANDS[index].id,
            field,
            move |text, cx| {
                if let Some(keys) = save(index, text, &settings, cx) {
                    shown.update(cx, |input, cx| input.set_text(keys, cx));
                }
            },
            cx,
        )
    }

    pub(super) fn shortcuts_reset_row(&self, cx: &mut Context<Self>) -> AnyElement {
        self.row(
            t!("settings-shortcuts-reset"),
            Button::new("shortcuts-reset")
                .secondary()
                .small()
                .label(t!("settings-shortcuts-reset-button"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.settings
                        .update(cx, |settings, cx| settings.reset_shortcuts(cx));
                    for (command, field) in COMMANDS.iter().zip(&this.shortcuts) {
                        let shown = command.shown(this.settings.read(cx).shortcuts());
                        field.update(cx, |input, cx| input.set_text(shown, cx));
                    }
                }))
                .into_any_element(),
        )
    }
}

/// Applies `text` as the shortcut of the command at `index`: empty text unbinds it. Says why on
/// a toast and returns `None` when the text is not usable or the key belongs to another command;
/// otherwise returns the keystroke as it is now written, for the field to show.
fn save(index: usize, text: &str, settings: &Entity<AppSettings>, cx: &mut App) -> Option<String> {
    let command = &COMMANDS[index];
    if text.trim().is_empty() {
        settings.update(cx, |settings, cx| {
            settings.set_shortcut(command.id, None, cx)
        });
        return Some(String::new());
    }
    let keys = match input::check(command, text) {
        Ok(keys) => keys,
        Err(Invalid::Keystroke) => {
            Toasts::show(Outcome::Failed, "toast-shortcut-keystroke", cx);
            return None;
        }
        Err(Invalid::NeedsModifier) => {
            Toasts::show(Outcome::Failed, "toast-shortcut-needs-modifier", cx);
            return None;
        }
    };
    let changes = settings.read(cx).shortcuts().clone();
    if let Some(holder) = input::holder(command, &keys, &changes) {
        match input::stance(command, holder) {
            Stance::Refuse => {
                Toasts::about(
                    Outcome::Failed,
                    "toast-shortcut-taken",
                    i18n::lookup(holder.label, None),
                    cx,
                );
                return None;
            }
            Stance::Take => settings.update(cx, |settings, cx| {
                settings.set_shortcut(holder.id, None, cx)
            }),
        }
    }
    settings.update(cx, |settings, cx| {
        settings.set_shortcut(command.id, Some(keys.clone()), cx)
    });
    Some(keys)
}
