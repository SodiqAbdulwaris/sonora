use gpui::prelude::*;
use gpui::{AnyElement, App, Context, SharedString, div};
use i18n::t;
use music::backend::{Backend, BackendChoice, Problem};
use state::{Outcome, Toasts};
use ui::{Button, MenuItem, Picker};

use super::rows::Row;
use super::{Group, SettingsView, Slot};

const ROUTE: &str = "spotify-route";
/// Days before a Soloist build runs out that the status starts to say so.
const EXPIRY_WARNING: u32 = 14;

impl SettingsView {
    /// The group for where Spotify's audio comes from, left out while another provider is the
    /// signed-in one. The key row only stands when Soloist can be used at all.
    pub(super) fn spotify_groups(&self, cx: &App) -> Vec<Group> {
        if self.session.read(cx).provider_slug() != Some("spotify") {
            return Vec::new();
        }

        let mut slots = vec![Slot::SpotifyRoute, Slot::SpotifyRouteStatus];
        if self.playback.read(cx).backend() != BackendChoice::Librespot {
            slots.push(Slot::SoloistKey);
        }
        vec![Group::new("settings-group-spotify-route", "icons/speaker.svg").rows(slots)]
    }

    /// The detail of the status row, which a search reads.
    pub(super) fn spotify_route_detail(&self, cx: &App) -> SharedString {
        self.route_problems(cx)
    }

    pub(super) fn spotify_route_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let chosen = self.playback.read(cx).backend();

        let picker = Picker::new(
            ROUTE,
            &self.popovers,
            i18n::lookup(choice_key(chosen), None),
        )
        .width(Picker::NARROW)
        .items(BackendChoice::ALL.map(|choice| {
            MenuItem::new(choice.id(), i18n::lookup(choice_key(choice), None))
                .selected(choice == chosen)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.playback
                        .update(cx, |playback, cx| playback.set_backend(choice, cx));
                    cx.notify();
                }))
        }));

        self.row(
            t!("settings-spotify-route"),
            picker.name(t!("settings-spotify-route")).into_any_element(),
        )
    }

    /// Which route plays now, and under it why the others do not.
    pub(super) fn spotify_route_status_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let report = self.playback.read(cx).backend_report();
        let value = match report.active {
            Some(Backend::Soloist) => t!("settings-spotify-route-soloist"),
            Some(Backend::Librespot) => t!("settings-spotify-route-librespot"),
            None => t!("settings-spotify-route-none"),
        };
        let problems = self.route_problems(cx);

        Row::new(
            t!("settings-spotify-route-status"),
            ui::spoken("settings-spotify-route-active", value).into_any_element(),
        )
        .detail(problems)
        .into_any_element()
    }

    /// The reasons a route is passed over, one line each, and the days a Soloist build has left
    /// once that is few.
    fn route_problems(&self, cx: &App) -> SharedString {
        let report = self.playback.read(cx).backend_report();
        let mut lines: Vec<String> = report
            .skipped
            .iter()
            .map(|(backend, problem)| {
                let route = match backend {
                    Backend::Soloist => i18n::lookup("settings-spotify-route-soloist", None),
                    Backend::Librespot => i18n::lookup("settings-spotify-route-librespot", None),
                };
                format!("{route}: {}", problem_text(problem))
            })
            .collect();
        if let Some(days) = report.days_left.filter(|days| *days <= EXPIRY_WARNING) {
            lines.push(t!("settings-spotify-soloist-expiry", count = days).to_string());
        }
        match lines.is_empty() {
            true => t!("settings-spotify-route-status-detail"),
            false => lines.join("\n").into(),
        }
    }

    /// The API key Soloist starts with, kept in the system's credential store and never shown
    /// again once saved.
    pub(super) fn soloist_key_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let field = self.soloist_key.clone();
        let set = music::backend::soloist_key_set();
        let detail = match set {
            true => t!("settings-soloist-key-set"),
            false => t!("settings-soloist-key-unset"),
        };

        Row::new(
            t!("settings-soloist-key"),
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_end()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .max_w(Picker::WIDE)
                        .child(self.soloist_key.clone()),
                )
                .child(
                    Button::new("soloist-key-save")
                        .flex_none()
                        .secondary()
                        .small()
                        .label(t!("settings-soloist-key-save"))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            let typed = field.read(cx).text().trim().to_owned();
                            if typed.is_empty() {
                                return;
                            }
                            let stored = music::backend::store_soloist_key(&typed);
                            field.update(cx, |input, cx| input.set_text("", cx));
                            match stored {
                                Ok(()) => {
                                    Toasts::show(Outcome::Done, "toast-soloist-key-saved", cx)
                                }
                                Err(error) => {
                                    log::warn!("settings: cannot store the Soloist key: {error:#}");
                                    Toasts::show(Outcome::Failed, "toast-soloist-key-failed", cx);
                                }
                            }
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
        .yielding()
        .detail(detail)
        .into_any_element()
    }
}

fn choice_key(choice: BackendChoice) -> &'static str {
    match choice {
        BackendChoice::Auto => "settings-spotify-route-auto",
        BackendChoice::Soloist => "settings-spotify-route-soloist",
        BackendChoice::Librespot => "settings-spotify-route-librespot",
    }
}

/// Why a route was passed over, in the listener's language.
fn problem_text(problem: &Problem) -> SharedString {
    match problem {
        Problem::NoSoloist => t!("settings-spotify-problem-no-soloist"),
        Problem::NoKey => t!("settings-spotify-problem-no-key"),
        Problem::NeedsPairing => t!("settings-spotify-problem-pairing"),
        Problem::Expired => t!("settings-spotify-problem-expired"),
        Problem::Stopped(why) => t!("settings-spotify-problem-stopped", why = why.as_str()),
        Problem::Refused => t!("settings-spotify-problem-refused"),
    }
}
