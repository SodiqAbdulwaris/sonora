//! The mini player overlay window: opening it, placing it, keeping it on top, letting clicks pass
//! through it, and the global shortcuts that summon it from any program.
//!
//! Which choices are on lives in settings. This watches them and makes the window match, so a
//! change in Settings, a shortcut or the tray all go through the same path.

#[cfg(windows)]
mod keys;

use std::time::Duration;

use gpui::{
    App, AppContext as _, Bounds, Context, Entity, Global, Pixels, Task, Window,
    WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, point, px,
};
use state::{Corner, Outcome, Shape, Shortcut, Sonora, Toasts};
use tokio::sync::mpsc::unbounded_channel;
use ui::ActiveTheme as _;
use views::{OVERLAY_DISC, OVERLAY_PILL, OverlayView};

/// How far the overlay stands from the edge of the screen.
const MARGIN: Pixels = px(12.);
/// How often the main window is looked at for being minimised, while the overlay may show itself
/// then.
const WATCH: Duration = Duration::from_millis(500);

/// What the open window was made to look like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Look {
    shape: Shape,
    corner: Corner,
    through: bool,
    on_top: bool,
}

/// What a global shortcut asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Press {
    Toggle,
    ClickThrough,
    /// A shortcut another program already holds.
    Taken(Shortcut),
}

struct Installed(Entity<Overlay>);

impl Global for Installed {}

/// Does what a global shortcut or the tray asks of the overlay.
pub(crate) fn press(press: Press, cx: &mut App) {
    let overlay = cx.global::<Installed>().0.clone();
    overlay.update(cx, |overlay, cx| overlay.pressed(press, cx));
}

/// Starts keeping the overlay window in step with settings.
pub fn install(cx: &mut App) {
    let overlay = cx.new(Overlay::new);
    cx.set_global(Installed(overlay));
}

struct Overlay {
    window: Option<(WindowHandle<OverlayView>, Look)>,
    /// Whether the window is up only because the main window is out of sight.
    auto_shown: bool,
    #[cfg(windows)]
    keys: Option<keys::Keys>,
    /// The shortcuts `keys` was started with, so a change restarts it and nothing else does.
    #[cfg(windows)]
    held: Option<(Shortcut, Shortcut)>,
    #[cfg(windows)]
    presses: tokio::sync::mpsc::UnboundedSender<Press>,
    watch: Option<Task<()>>,
    _presses: Task<()>,
}

impl Overlay {
    fn new(cx: &mut Context<Self>) -> Self {
        let (presses, mut received) = unbounded_channel::<Press>();
        let heard = cx.spawn(async move |this, cx| {
            while let Some(press) = received.recv().await {
                if this.update(cx, |this, cx| this.pressed(press, cx)).is_err() {
                    break;
                }
            }
        });

        let settings = Sonora::global(cx).settings.clone();
        cx.observe(&settings, |this, _, cx| this.sync(cx)).detach();

        let mut overlay = Self {
            window: None,
            auto_shown: false,
            #[cfg(windows)]
            keys: None,
            #[cfg(windows)]
            held: None,
            #[cfg(windows)]
            presses,
            watch: None,
            _presses: heard,
        };
        #[cfg(not(windows))]
        drop(presses);
        overlay.sync(cx);
        overlay
    }

    fn pressed(&mut self, press: Press, cx: &mut Context<Self>) {
        let settings = Sonora::global(cx).settings.clone();
        match press {
            Press::Toggle => {
                let on = self.window.is_none();
                self.auto_shown = false;
                settings.update(cx, |settings, cx| settings.set_overlay(on, cx));
                self.sync(cx);
            }
            Press::ClickThrough => settings.update(cx, |settings, cx| {
                let through = settings.overlay_click_through();
                settings.set_overlay_click_through(!through, cx);
            }),
            Press::Taken(shortcut) => {
                log::warn!("overlay: another program holds {shortcut}");
                Toasts::show(Outcome::Failed, "toast-overlay-shortcut-taken", cx);
            }
        }
    }

    /// Makes the window, the shortcuts and the watch for a minimised main window match settings.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let settings = Sonora::global(cx).settings.clone();
        let settings = settings.read(cx);
        let auto = settings.overlay_auto_show();
        let wanted = (settings.overlay() || self.auto_shown && auto).then(|| Look {
            shape: settings.overlay_shape(),
            corner: settings.overlay_corner(),
            through: settings.overlay_click_through(),
            on_top: settings.overlay_on_top(),
        });
        #[cfg(windows)]
        let shortcuts = (settings.overlay_toggle(), settings.overlay_click_shortcut());

        if !auto {
            self.auto_shown = false;
            self.watch = None;
        } else if self.watch.is_none() {
            self.watch = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(WATCH).await;
                    if this.update(cx, |this, cx| this.look(cx)).is_err() {
                        break;
                    }
                }
            }));
        }

        #[cfg(windows)]
        if self.held != Some(shortcuts) {
            self.keys = keys::Keys::start(shortcuts.0, shortcuts.1, self.presses.clone());
            self.held = Some(shortcuts);
        }

        match (self.window.take(), wanted) {
            (None, None) => {}
            (Some((handle, _)), None) => close(handle, cx),
            (None, Some(look)) => self.window = open(look, cx).map(|handle| (handle, look)),
            (Some((handle, have)), Some(look)) => {
                // a different size or place needs a different window
                if (have.shape, have.corner) != (look.shape, look.corner) {
                    close(handle, cx);
                    self.window = open(look, cx).map(|handle| (handle, look));
                } else {
                    if have != look {
                        handle.update(cx, |_, window, _| apply(window, look)).ok();
                    }
                    self.window = Some((handle, look));
                }
            }
        }
    }

    /// Shows the overlay while the main window is minimised or gone, and puts it away once the
    /// main window is back in front. An overlay the listener turned on stays whatever the window
    /// does.
    fn look(&mut self, cx: &mut Context<Self>) {
        let own = self.window.as_ref().map(|(handle, _)| handle.window_id());
        let main = cx
            .windows()
            .into_iter()
            .find(|window| Some(window.window_id()) != own);
        let settings = Sonora::global(cx).settings.clone();
        if settings.read(cx).overlay() {
            return;
        }
        let (hidden, active) = match main {
            None => (true, false),
            Some(main) => main
                .update(cx, |_, window, _| {
                    (minimised(window), window.is_window_active())
                })
                .unwrap_or((true, false)),
        };
        let shown = self.auto_shown;
        match (hidden, active) {
            (true, _) if !shown => self.auto_shown = true,
            (false, true) if shown => self.auto_shown = false,
            _ => return,
        }
        self.sync(cx);
    }
}

fn close(handle: WindowHandle<OverlayView>, cx: &mut App) {
    handle
        .update(cx, |_, window, _| window.remove_window())
        .ok();
}

/// Opens the overlay as a frameless, transparent window above the others that takes no focus.
fn open(look: Look, cx: &mut App) -> Option<WindowHandle<OverlayView>> {
    let size = match look.shape {
        Shape::Pill => OVERLAY_PILL,
        Shape::Disc => OVERLAY_DISC,
    };
    let visible = cx.primary_display()?.visible_bounds();
    let x = match look.corner {
        Corner::TopLeft => visible.origin.x + MARGIN,
        Corner::TopCenter => visible.origin.x + (visible.size.width - size.width) / 2.,
        Corner::TopRight => visible.origin.x + visible.size.width - size.width - MARGIN,
    };
    let bounds = Bounds {
        origin: point(x, visible.origin.y + MARGIN),
        size,
    };

    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: true,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            app_id: Some(crate::assets::app_id().into()),
            ..Default::default()
        },
        |window, cx| {
            window.set_rem_size(cx.theme().font_size);
            apply(window, look);
            cx.new(OverlayView::new)
        },
    );
    match opened {
        Ok(handle) => Some(handle),
        Err(error) => {
            log::warn!("overlay: cannot open the window: {error:#}");
            None
        }
    }
}

/// Keeps the window above the others, or lets them cover it, and lets clicks pass through it or
/// takes them. It never takes the focus, so playing with it leaves the program being used alone.
#[cfg(windows)]
fn apply(window: &Window, look: Look) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, HWND_NOTOPMOST, HWND_TOPMOST, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowLongPtrW, SetWindowPos, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
    };

    let Ok(RawWindowHandle::Win32(handle)) =
        HasWindowHandle::window_handle(window).map(|handle| handle.as_raw())
    else {
        return;
    };
    let handle = handle.hwnd.get() as *mut std::ffi::c_void;
    let through = (WS_EX_LAYERED | WS_EX_TRANSPARENT) as isize;
    unsafe {
        let style = GetWindowLongPtrW(handle, GWL_EXSTYLE) | WS_EX_NOACTIVATE as isize;
        let style = match look.through {
            true => style | through,
            false => style & !through,
        };
        SetWindowLongPtrW(handle, GWL_EXSTYLE, style);
        SetWindowPos(
            handle,
            match look.on_top {
                true => HWND_TOPMOST,
                false => HWND_NOTOPMOST,
            },
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

#[cfg(not(windows))]
fn apply(_window: &Window, _look: Look) {}

/// Whether the window is minimised to the taskbar.
#[cfg(windows)]
fn minimised(window: &Window) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::IsIconic;

    let Ok(RawWindowHandle::Win32(handle)) =
        HasWindowHandle::window_handle(window).map(|handle| handle.as_raw())
    else {
        return false;
    };
    unsafe { IsIconic(handle.hwnd.get() as *mut std::ffi::c_void) != 0 }
}

#[cfg(not(windows))]
fn minimised(_window: &Window) -> bool {
    false
}
