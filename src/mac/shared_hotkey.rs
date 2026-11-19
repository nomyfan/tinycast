//! Global shortcuts another app may hold too: Carbon's hot keys registered
//! without `kEventHotKeyExclusive`, which nsh's `register_global_shortcut`
//! always asks for. macOS 27's Spotlight (the Siri AI app) holds ⌘Space so
//! even with its shortcut off in System Settings, and refuses exclusive
//! registrations of it; Raycast shares it the same way.
//!
//! Carbon is used on the main thread: registering is sent there, and the
//! presses come back through a channel to the engine thread.

use std::{
    cell::RefCell,
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};

use dispatch2::DispatchQueue;
use nsh_core::desktop::{Key, Shortcut};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

type OsStatus = i32;
type EventRef = *mut c_void;
type EventTargetRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventHandler = unsafe extern "C" fn(*mut c_void, EventRef, *mut c_void) -> OsStatus;

const fn code(name: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*name)
}

const EVENT_CLASS_KEYBOARD: u32 = code(b"keyb");
const EVENT_HOT_KEY_PRESSED: u32 = 5;
const EVENT_PARAM_DIRECT_OBJECT: u32 = code(b"----");
const TYPE_EVENT_HOT_KEY_ID: u32 = code(b"hkid");
const EVENT_NOT_HANDLED: OsStatus = -9874;
/// The signature of these hot keys' ids; nsh's handler passes them on.
const SIGNATURE: u32 = code(b"TCsh");

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
#[derive(Default)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandler,
        count: usize,
        types: *const EventTypeSpec,
        user_data: *mut c_void,
        out: *mut *mut c_void,
    ) -> OsStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: EventTargetRef,
        options: u32,
        out: *mut EventHotKeyRef,
    ) -> OsStatus;
    fn UnregisterEventHotKey(hot_key: EventHotKeyRef) -> OsStatus;
    fn GetEventParameter(
        event: EventRef,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> OsStatus;
}

struct Registered {
    id: u32,
    hot_key: EventHotKeyRef,
    pressed: UnboundedSender<()>,
}

thread_local! {
    /// The main thread's registered hot keys; `None` until the handler is
    /// installed.
    static REGISTERED: RefCell<Option<Vec<Registered>>> = const { RefCell::new(None) };
}

static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// A shared hot key, given back when dropped.
pub struct SharedHotKey(u32);

impl SharedHotKey {
    /// Registers `shortcut`, sending on `pressed` at each press. The answer
    /// says whether the system took it; `None`: a key this cannot name.
    pub fn register(
        shortcut: &Shortcut,
        pressed: UnboundedSender<()>,
    ) -> Option<(Self, oneshot::Receiver<bool>)> {
        let key_code = key_code(shortcut.key)?;
        let modifiers = [
            (shortcut.meta, 1 << 8),
            (shortcut.shift, 1 << 9),
            (shortcut.alt, 1 << 11),
            (shortcut.ctrl, 1 << 12),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .fold(0, |all, (_, flag)| all | flag);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = oneshot::channel();
        DispatchQueue::main().exec_async(move || {
            let _ = answer.send(register_on_main(id, key_code, modifiers, pressed));
        });
        Some((Self(id), answered))
    }
}

impl Drop for SharedHotKey {
    fn drop(&mut self) {
        let id = self.0;
        DispatchQueue::main().exec_async(move || {
            REGISTERED.with_borrow_mut(|registered| {
                let Some(registered) = registered else { return };
                registered.retain(|r| {
                    if r.id != id {
                        return true;
                    }
                    // SAFETY: a hot key `RegisterEventHotKey` gave,
                    // unregistered once.
                    unsafe { UnregisterEventHotKey(r.hot_key) };
                    false
                });
            });
        });
    }
}

fn register_on_main(id: u32, key_code: u16, modifiers: u32, pressed: UnboundedSender<()>) -> bool {
    if !install_handler() {
        return false;
    }
    let mut hot_key = ptr::null_mut();
    // SAFETY: the target is the application's; `hot_key` receives the ref.
    let status = unsafe {
        RegisterEventHotKey(
            u32::from(key_code),
            modifiers,
            EventHotKeyId { signature: SIGNATURE, id },
            GetApplicationEventTarget(),
            0,
            &mut hot_key,
        )
    };
    if status != 0 {
        log::warn!("RegisterEventHotKey (shared): {status}");
        return false;
    }
    REGISTERED.with_borrow_mut(|registered| {
        registered.get_or_insert_default().push(Registered { id, hot_key, pressed });
    });
    true
}

/// Installs the handler of these hot keys once; whether it is.
fn install_handler() -> bool {
    if REGISTERED.with_borrow(Option::is_some) {
        return true;
    }
    let types = EventTypeSpec {
        event_class: EVENT_CLASS_KEYBOARD,
        event_kind: EVENT_HOT_KEY_PRESSED,
    };
    // SAFETY: a function for the process's life, one type, no user data.
    let status = unsafe {
        InstallEventHandler(
            GetApplicationEventTarget(),
            pressed,
            1,
            &types,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if status != 0 {
        log::warn!("InstallEventHandler: {status}");
        return false;
    }
    REGISTERED.with_borrow_mut(|registered| *registered = Some(Vec::new()));
    true
}

/// A hot key was pressed, on the main thread.
unsafe extern "C" fn pressed(_call: *mut c_void, event: EventRef, _data: *mut c_void) -> OsStatus {
    let mut id = EventHotKeyId::default();
    // SAFETY: `id` is the parameter's type and size.
    let status = unsafe {
        GetEventParameter(
            event,
            EVENT_PARAM_DIRECT_OBJECT,
            TYPE_EVENT_HOT_KEY_ID,
            ptr::null_mut(),
            size_of::<EventHotKeyId>(),
            ptr::null_mut(),
            (&raw mut id).cast(),
        )
    };
    if status != 0 || id.signature != SIGNATURE {
        // nsh's hot keys, which its own handler takes.
        return EVENT_NOT_HANDLED;
    }
    REGISTERED.with_borrow(|registered| {
        let found = registered.iter().flatten().find(|r| r.id == id.id);
        if let Some(r) = found {
            let _ = r.pressed.send(());
        }
    });
    0
}

/// The virtual key code (`kVK_*`) of `key` on the US layout.
fn key_code(key: Key) -> Option<u16> {
    // kVK_ANSI_* in key code order; `\0` where the key is no character.
    const ANSI: &str = "asdfhgzxcv\0bqweryt123465=97-80]ou[ip\0lj'k;\\,/nm.";
    match key {
        Key::Space => Some(49),
        Key::Char(c) => ANSI.find(c.to_ascii_lowercase()).and_then(|i| u16::try_from(i).ok()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_have_their_us_codes() {
        assert_eq!(key_code(Key::Space), Some(49));
        assert_eq!(key_code(Key::Char('a')), Some(0));
        assert_eq!(key_code(Key::Char('v')), Some(9));
        assert_eq!(key_code(Key::Char('1')), Some(18));
        assert_eq!(key_code(Key::Char('.')), Some(47));
        assert_eq!(key_code(Key::Enter), None);
    }
}
