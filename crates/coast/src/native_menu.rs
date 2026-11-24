//! A real AppKit context menu for the terminal.
//!
//! gpui can draw a popup, but it cannot reproduce AppKit's menu material,
//! vibrancy, animation or keyboard navigation. This module uses `NSMenu`
//! directly instead, and reports the chosen item back after AppKit's modal
//! menu-tracking loop returns, so the action can run through gpui with the
//! view's `Context`.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, sel, ClassType, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::NSString;

/// Which menu item was chosen.
#[derive(Debug)]
pub enum Choice {
    Copy,
    Paste,
}

const COPY_TAG: i32 = 1;
const PASTE_TAG: i32 = 2;
static LAST_ACTION: AtomicI32 = AtomicI32::new(0);
/// Choice handed back to gpui after the modal menu loop has unwound.
static PENDING: Mutex<Option<(u64, Choice)>> = Mutex::new(None);

define_class!(
    /// Target for both items. AppKit invokes this while tracking the menu;
    /// all it does is record the tag so the caller can act on it afterwards.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(coastMenuSelected:))]
        fn coast_menu_selected(&self, sender: &NSMenuItem) {
            LAST_ACTION.store(sender.tag() as i32, Ordering::SeqCst);
        }
    }

    unsafe impl NSObjectProtocol for MenuTarget {}
);

/// Show a native context menu for the current right-click event.
///
/// Returns `None` if the menu was dismissed without a choice, or if the
/// click did not come from an AppKit window (e.g. in gpui's test harness).
pub fn show(can_copy: bool, can_paste: bool) -> Option<Choice> {
    let mtm = MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(mtm);
    // gpui dispatches mouse-down synchronously from AppKit, so the current
    // NSEvent is the right-click that brought us here.
    let event = app.currentEvent()?;
    let view = event.window(mtm)?.contentView()?;

    LAST_ACTION.store(0, Ordering::SeqCst);
    let menu = NSMenu::new(mtm);
    let target: Retained<MenuTarget> = unsafe { msg_send![MenuTarget::class(), new] };
    let action = sel!(coastMenuSelected:);

    let add_item = |title: &str, tag: i32| {
        let item = NSMenuItem::new(mtm);
        item.setTitle(&NSString::from_str(title));
        item.setTag(tag as _);
        unsafe {
            item.setTarget(Some(&*target));
            item.setAction(Some(action));
        }
        menu.addItem(&item);
    };

    if can_copy {
        add_item("Copy", COPY_TAG);
    }
    if can_copy && can_paste {
        menu.addItem(&NSMenuItem::separatorItem(mtm));
    }
    if can_paste {
        add_item("Paste", PASTE_TAG);
    }

    // Blocks in AppKit's menu-tracking loop until the user picks or dismisses.
    NSMenu::popUpContextMenu_withEvent_forView(&menu, &event, &view);

    match LAST_ACTION.swap(0, Ordering::SeqCst) {
        COPY_TAG => Some(Choice::Copy),
        PASTE_TAG => Some(Choice::Paste),
        _ => None,
    }
}

/// Queue the menu for a later main-thread turn.
///
/// `popUpContextMenu` runs AppKit's own nested event loop. Calling it from
/// inside gpui's mouse-down handler re-enters gpui while its `App` is
/// mutably borrowed, which panics. GCD lets the current handler unwind
/// first; the chosen item is collected by `TerminalView::poll`.
pub fn schedule(entity_id: u64, can_copy: bool, can_paste: bool) {
    dispatch::Queue::main().exec_async(move || {
        if let Some(choice) = show(can_copy, can_paste) {
            let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
            *pending = Some((entity_id, choice));
        }
    });
}

/// Take the pending choice, if it belongs to `entity_id`.
pub fn take(entity_id: u64) -> Option<Choice> {
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    match *pending {
        Some((id, _)) if id == entity_id => pending.take().map(|(_, choice)| choice),
        _ => None,
    }
}
