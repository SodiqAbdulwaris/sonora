//! Global shortcuts on Windows: a thread of their own that registers the hotkeys and waits for
//! them, since a hotkey is posted to the thread that registered it.

use std::ptr::null_mut;
use std::sync::mpsc::channel;

use state::{Key, Shortcut};
use tokio::sync::mpsc::UnboundedSender;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
    VK_F1, VK_SPACE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_HOTKEY, WM_QUIT,
};

use super::Press;

/// The ids the two hotkeys are registered under.
const TOGGLE: i32 = 1;
const CLICK_THROUGH: i32 = 2;

/// The registered hotkeys. Dropping them gives the keys back.
pub(super) struct Keys {
    thread: u32,
}

impl Keys {
    /// Registers the shortcuts and starts listening, or `None` when the thread cannot start. A
    /// shortcut another program already holds is reported as `Press::Taken`.
    pub(super) fn start(
        toggle: Shortcut,
        click_through: Shortcut,
        presses: UnboundedSender<Press>,
    ) -> Option<Self> {
        let (ready, thread) = channel();
        let spawned = std::thread::Builder::new()
            .name("overlay-keys".into())
            .spawn(move || unsafe {
                let mut message: MSG = std::mem::zeroed();
                // asking for a message makes the thread's queue, which the hotkeys post to
                PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
                ready.send(GetCurrentThreadId()).ok();

                let mut held = Vec::new();
                for (id, shortcut) in [(TOGGLE, toggle), (CLICK_THROUGH, click_through)] {
                    match RegisterHotKey(null_mut(), id, modifiers(shortcut), code(shortcut)) {
                        0 => {
                            presses.send(Press::Taken(shortcut)).ok();
                        }
                        _ => held.push(id),
                    }
                }
                while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                    if message.message != WM_HOTKEY {
                        continue;
                    }
                    let press = match message.wParam as i32 {
                        TOGGLE => Press::Toggle,
                        _ => Press::ClickThrough,
                    };
                    if presses.send(press).is_err() {
                        break;
                    }
                }
                for id in held {
                    UnregisterHotKey(null_mut(), id);
                }
            });
        if let Err(error) = spawned {
            log::warn!("overlay: cannot start the shortcut thread: {error}");
            return None;
        }
        Some(Self {
            thread: thread.recv().ok()?,
        })
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        unsafe { PostThreadMessageW(self.thread, WM_QUIT, 0, 0) };
    }
}

fn modifiers(shortcut: Shortcut) -> u32 {
    [
        (shortcut.ctrl, MOD_CONTROL),
        (shortcut.alt, MOD_ALT),
        (shortcut.shift, MOD_SHIFT),
        (shortcut.win, MOD_WIN),
    ]
    .into_iter()
    .filter(|(held, _)| *held)
    .fold(MOD_NOREPEAT, |all, (_, bit)| all | bit)
}

/// The virtual-key code of the shortcut's key: a letter or digit is its own character.
fn code(shortcut: Shortcut) -> u32 {
    match shortcut.key {
        Key::Letter(character) | Key::Digit(character) => character as u32,
        Key::Function(number) => VK_F1 as u32 + u32::from(number) - 1,
        Key::Space => VK_SPACE as u32,
    }
}
