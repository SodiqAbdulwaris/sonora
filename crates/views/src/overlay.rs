//! What the mini player overlay window shows: a pill with the cover, the song and its controls, or
//! the small spinning disc the pill folds into.
//!
//! The window is opened, placed and made to ignore the pointer by the app binary. This view only
//! draws, reading the player and the settings, and changes the shape or the click-through setting.

use std::time::Duration;

use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, Entity, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, Size, Subscription, Transformation, Window, div, percentage, px, size,
    svg,
};
use i18n::t;
use state::{AppSettings, Playback, Shape, Sonora};
use ui::{ActiveTheme as _, Artwork, Button, Text, motion};

use crate::shared::transport::{like, next, previous, toggle};

/// The overlay's window size as a pill and as a disc, in logical pixels.
pub const PILL: Size<Pixels> = size(px(400.), px(64.));
pub const DISC: Size<Pixels> = size(px(72.), px(72.));
/// How long the vinyl takes to turn once.
const TURN: Duration = Duration::from_secs(4);
/// The share of the disc the cover label takes.
const LABEL: f32 = 0.42;

/// The overlay window's one view.
pub struct OverlayView {
    playback: Entity<Playback>,
    settings: Entity<AppSettings>,
    _observing: [Subscription; 3],
}

impl OverlayView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let sonora = Sonora::global(cx);
        let (playback, settings, library) = (
            sonora.playback.clone(),
            sonora.settings.clone(),
            sonora.library.clone(),
        );
        Self {
            _observing: [
                cx.observe(&playback, |_, _, cx| cx.notify()),
                cx.observe(&settings, |_, _, cx| cx.notify()),
                // the heart follows the library
                cx.observe(&library, |_, _, cx| cx.notify()),
            ],
            playback,
            settings,
        }
    }

    fn pill(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let shown = self.playback.read(cx).shown_track(cx);
        let track = shown.as_ref();
        let cover = track.and_then(|track| track.cover.clone());
        let title = track.map(|track| track.name.clone());
        let artists = track.map(|track| track.artists.clone());
        let through = self.settings.read(cx).overlay_click_through();

        let handle = div()
            .id("overlay-handle")
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_3()
            .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, window, _| {
                window.start_window_move();
            })
            .child(Artwork::new(cover).size(theme.metrics.row).flex_none())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        ui::spoken(
                            "overlay-title",
                            title.map_or_else(|| t!("overlay-nothing"), gpui::SharedString::from),
                        )
                        .truncate()
                        .text_size(theme.text(Text::Label)),
                    )
                    .child(
                        ui::spoken("overlay-artists", artists.unwrap_or_default())
                            .truncate()
                            .text_color(theme.muted_foreground)
                            .text_size(theme.text(Text::Small)),
                    ),
            );

        let controls = div()
            .flex()
            .flex_none()
            .items_center()
            .child(like(track, &self.playback, cx))
            .child(previous(&self.playback, false, cx))
            .child(toggle(&self.playback, false, false, cx))
            .child(next(&self.playback, false, cx))
            .child(
                Button::new("overlay-click-through")
                    .ghost()
                    .small()
                    .icon("icons/pin.svg")
                    .tooltip_above("overlay-click-through")
                    .tint(theme.muted_foreground)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.settings.update(cx, |settings, cx| {
                            settings.set_overlay_click_through(true, cx)
                        });
                    })),
            )
            .child(
                Button::new("overlay-fold")
                    .ghost()
                    .small()
                    .icon("icons/minimize.svg")
                    .tooltip_above("overlay-fold")
                    .tint(theme.muted_foreground)
                    .on_click(cx.listener(|this, _, _, cx| this.fold(Shape::Disc, cx))),
            );

        div()
            .id("overlay")
            .role(Role::Group)
            .aria_label(t!("overlay-label"))
            .size_full()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .rounded_full()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            // a pill that lets clicks through is dimmed, so it reads as not taking them
            .when(through, |this| this.opacity(0.55))
            .child(handle)
            .child(controls)
            .into_any_element()
    }

    fn disc(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let shown = self.playback.read(cx).shown_track(cx);
        let cover = shown.as_ref().and_then(|track| track.cover.clone());
        let playing = self.playback.read(cx).wants_playing();
        let label = DISC.width * LABEL;

        let record = svg()
            .path(icons::path("icons/vinyl.svg"))
            .size_full()
            .text_color(theme.popover_foreground);
        let record = match playing && motion::animates(cx) {
            true => record
                .with_animation(
                    "overlay-vinyl",
                    Animation::new(TURN).repeat(),
                    |record, turned| {
                        record.with_transformation(Transformation::rotate(percentage(turned)))
                    },
                )
                .into_any_element(),
            false => record.into_any_element(),
        };

        div()
            .id("overlay-disc")
            .role(Role::Button)
            .aria_label(t!("overlay-disc"))
            .size_full()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(theme.popover)
            .child(div().absolute().size_full().child(record))
            .child(Artwork::new(cover).size(label).circle())
            // a press that moves drags the disc along the edge; one that stays is a click
            .on_mouse_move(|event: &MouseMoveEvent, window, _| {
                if event.dragging() {
                    window.start_window_move();
                }
            })
            .on_click(cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                this.playback
                    .update(cx, |playback, cx| playback.toggle_play(cx));
                // the first click of a double click played or paused, so the second puts it back
                if event.click_count() >= 2 {
                    this.fold(Shape::Pill, cx);
                }
            }))
            .into_any_element()
    }

    fn fold(&mut self, shape: Shape, cx: &mut Context<Self>) {
        self.settings
            .update(cx, |settings, cx| settings.set_overlay_shape(shape, cx));
    }
}

impl Render for OverlayView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.settings.read(cx).overlay_shape() {
            Shape::Pill => self.pill(cx),
            Shape::Disc => self.disc(cx),
        }
    }
}
