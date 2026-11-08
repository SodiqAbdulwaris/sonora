use gpui::prelude::*;
use gpui::{AnyElement, Context, Entity, div};
use i18n::t;
use state::{Corner, InvalidShortcut, Outcome, Toasts};
use ui::{Button, Input, MenuItem, Picker, Switch};

use super::{Group, SettingsView, Slot};

const CORNER: &str = "overlay-corner";

impl SettingsView {
    /// The mini player's group. The shortcut rows stand only on Windows, the one platform
    /// where Sonora can listen for keys while another program has the focus.
    pub(super) fn overlay_groups(&self) -> Vec<Group> {
        let mut slots = vec![Slot::Overlay, Slot::OverlayCorner, Slot::OverlayThrough];
        slots.extend([Slot::OverlayAutoShow, Slot::OverlayOnTop]);
        if cfg!(windows) {
            slots.extend([Slot::OverlayToggle, Slot::OverlayClickShortcut]);
        }
        vec![Group::new("settings-group-overlay", "icons/pin.svg").rows(slots)]
    }

    pub(super) fn overlay_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.settings.read(cx).overlay();

        self.row(
            t!("settings-overlay"),
            Switch::new("overlay", on)
                .name(t!("settings-overlay"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.settings
                        .update(cx, |settings, cx| settings.set_overlay(!on, cx));
                }))
                .into_any_element(),
        )
    }

    pub(super) fn overlay_through_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.settings.read(cx).overlay_click_through();

        self.row(
            t!("settings-overlay-through"),
            Switch::new("overlay-through", on)
                .name(t!("settings-overlay-through"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.settings.update(cx, |settings, cx| {
                        settings.set_overlay_click_through(!on, cx)
                    });
                }))
                .into_any_element(),
        )
    }

    pub(super) fn overlay_auto_show_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.settings.read(cx).overlay_auto_show();

        self.row(
            t!("settings-overlay-auto-show"),
            Switch::new("overlay-auto-show", on)
                .name(t!("settings-overlay-auto-show"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.settings
                        .update(cx, |settings, cx| settings.set_overlay_auto_show(!on, cx));
                }))
                .into_any_element(),
        )
    }

    pub(super) fn overlay_on_top_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.settings.read(cx).overlay_on_top();

        self.row(
            t!("settings-overlay-on-top"),
            Switch::new("overlay-on-top", on)
                .name(t!("settings-overlay-on-top"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.settings
                        .update(cx, |settings, cx| settings.set_overlay_on_top(!on, cx));
                }))
                .into_any_element(),
        )
    }

    pub(super) fn overlay_corner_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let chosen = self.settings.read(cx).overlay_corner();

        let picker = Picker::new(CORNER, &self.popovers, i18n::lookup(chosen.key(), None))
            .width(Picker::NARROW)
            .items(Corner::ALL.map(|corner| {
                MenuItem::new(corner.id(), i18n::lookup(corner.key(), None))
                    .selected(corner == chosen)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings
                            .update(cx, |settings, cx| settings.set_overlay_corner(corner, cx));
                        cx.notify();
                    }))
            }));

        self.row(
            t!("settings-overlay-corner"),
            picker
                .name(t!("settings-overlay-corner"))
                .into_any_element(),
        )
    }

    pub(super) fn overlay_toggle_row(&self, cx: &mut Context<Self>) -> AnyElement {
        self.shortcut_row(
            t!("settings-overlay-toggle"),
            "overlay-toggle-save",
            self.overlay_toggle.clone(),
            |settings, text, cx| settings.set_overlay_toggle(text, cx),
            cx,
        )
    }

    pub(super) fn overlay_click_shortcut_row(&self, cx: &mut Context<Self>) -> AnyElement {
        self.shortcut_row(
            t!("settings-overlay-click-shortcut"),
            "overlay-click-shortcut-save",
            self.overlay_click_shortcut.clone(),
            |settings, text, cx| settings.set_overlay_click_shortcut(text, cx),
            cx,
        )
    }

    /// A text field for a shortcut and the button that applies it, which says why when the text
    /// is not a shortcut and leaves the old one in place.
    fn shortcut_row(
        &self,
        title: gpui::SharedString,
        button: &'static str,
        field: Entity<Input>,
        apply: fn(
            &mut state::AppSettings,
            &str,
            &mut Context<state::AppSettings>,
        ) -> Result<(), InvalidShortcut>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let typed = field.clone();
        let settings = self.settings.clone();

        self.yielding_row(
            title,
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_end()
                .gap_2()
                .child(div().flex_1().min_w_0().max_w(Picker::NARROW).child(field))
                .child(
                    Button::new(button)
                        .flex_none()
                        .secondary()
                        .small()
                        .label(t!("settings-overlay-shortcut-save"))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            let text = typed.read(cx).text().to_owned();
                            let applied =
                                settings.update(cx, |settings, cx| apply(settings, &text, cx));
                            match applied {
                                Ok(()) => {}
                                Err(InvalidShortcut::Empty | InvalidShortcut::Incomplete) => {
                                    Toasts::show(
                                        Outcome::Failed,
                                        "toast-overlay-shortcut-incomplete",
                                        cx,
                                    )
                                }
                                Err(InvalidShortcut::Key) => {
                                    Toasts::show(Outcome::Failed, "toast-overlay-shortcut-key", cx)
                                }
                            }
                        })),
                )
                .into_any_element(),
        )
    }
}
