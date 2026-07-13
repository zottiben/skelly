//! The macOS application menu so the standard system keyboard shortcuts work. winit installs no
//! menu on its own, so without this the usual `⌘H` (hide), `⌘⌥H` (hide others), `⌘M` (minimize),
//! `⌘Q` (quit) and the like do nothing. We build a minimal native menu (App + Window) wired to
//! the standard `AppKit` selectors.
//!
//! It deliberately omits an Edit menu: an "Edit > Copy ⌘C" item would let macOS intercept `⌘C`
//! (and `⌘V`/`⌘X`/`⌘A`) before the key ever reaches winit, which would break Skelly's own
//! terminal-aware copy/paste + `⌘K`/`⌘F`/`⌘T`/… handling. Those chords stay owned by the app.
#![cfg(target_os = "macos")]
// This module is the binary's one bit of FFI: building native AppKit menu objects. Every `unsafe`
// call is a plain AppKit message send with a `// SAFETY:` note; nothing escapes this module.
#![allow(unsafe_code)]

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::sel;
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSString};

/// Install Skelly's application menu on the shared `NSApplication`. Must run on the main thread
/// (the caller holds a `MainThreadMarker`) after the app has initialized.
pub(crate) fn install(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    let main_menu = NSMenu::new(mtm);

    // --- App menu (macOS renders its title as the app name) ---
    let app_menu = NSMenu::new(mtm);
    add_item(
        mtm,
        &app_menu,
        "About skelly",
        Some(sel!(orderFrontStandardAboutPanel:)),
        "",
        None,
    );
    add_separator(mtm, &app_menu);
    add_item(mtm, &app_menu, "Hide skelly", Some(sel!(hide:)), "h", None);
    add_item(
        mtm,
        &app_menu,
        "Hide Others",
        Some(sel!(hideOtherApplications:)),
        "h",
        Some(
            NSEventModifierFlags::NSEventModifierFlagCommand
                | NSEventModifierFlags::NSEventModifierFlagOption,
        ),
    );
    add_item(
        mtm,
        &app_menu,
        "Show All",
        Some(sel!(unhideAllApplications:)),
        "",
        None,
    );
    add_separator(mtm, &app_menu);
    add_item(
        mtm,
        &app_menu,
        "Quit skelly",
        Some(sel!(terminate:)),
        "q",
        None,
    );
    let app_item = NSMenuItem::new(mtm);
    app_item.setSubmenu(Some(&app_menu));
    main_menu.addItem(&app_item);

    // --- Window menu (macOS wires it up for window management) ---
    let window_menu = NSMenu::new(mtm);
    // SAFETY: setting a menu's title to an owned NSString; no aliasing/lifetime obligations.
    unsafe { window_menu.setTitle(&NSString::from_str("Window")) };
    add_item(
        mtm,
        &window_menu,
        "Minimize",
        Some(sel!(performMiniaturize:)),
        "m",
        None,
    );
    add_item(
        mtm,
        &window_menu,
        "Zoom",
        Some(sel!(performZoom:)),
        "",
        None,
    );
    let window_item = NSMenuItem::new(mtm);
    window_item.setSubmenu(Some(&window_menu));
    main_menu.addItem(&window_item);

    app.setMainMenu(Some(&main_menu));
    // SAFETY: registering the Window menu so AppKit manages the window list; the menu outlives the
    // app (it is retained by the main menu). No aliasing obligations.
    unsafe { app.setWindowsMenu(Some(&window_menu)) };
}

/// Append a titled item bound to an `AppKit` `action` selector + a `⌘key` equivalent (empty `key`
/// = no shortcut), optionally overriding the modifier `mask` (default `⌘`).
fn add_item(
    mtm: MainThreadMarker,
    menu: &NSMenu,
    title: &str,
    action: Option<Sel>,
    key: &str,
    mask: Option<NSEventModifierFlags>,
) {
    // SAFETY: a standard NSMenuItem designated initializer; the title/key are owned NSStrings and
    // the action is a valid AppKit selector (or None). No aliasing or lifetime obligations.
    let item: Retained<NSMenuItem> = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    };
    if let Some(mask) = mask {
        item.setKeyEquivalentModifierMask(mask);
    }
    menu.addItem(&item);
}

/// Append a separator line to `menu`.
fn add_separator(mtm: MainThreadMarker, menu: &NSMenu) {
    menu.addItem(&NSMenuItem::separatorItem(mtm));
}
