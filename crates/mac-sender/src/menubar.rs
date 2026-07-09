//! macOS menü çubuğu (status bar) göstergesi: PASİF/AKTİF + Çıkış.
//!
//! - Accessory aktivasyon politikası → Dock ikonu YOK, sadece menü çubuğu.
//! - NSStatusItem başlığı bir emoji + metinle durumu gösterir.
//! - Dropdown NSMenu içinde "Cikis" (Quit) → çıkmadan önce imleci yeniden bağlar.
//!
//! Her şey ANA THREAD'de çalışır (run() ana thread'den çağrılıyor, tap callback
//! de ana thread'de tetikleniyor), bu yüzden başlığı callback'ten DOĞRUDAN
//! güncellemek güvenli.

#![allow(non_snake_case)]

use std::cell::Cell;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSMenu, NSMenuItem,
    NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSString, NSTimer,
};

/// PASİF/AKTİF durumuna göre menü çubuğu başlığı (emoji + metin).
pub fn title_for(active: bool) -> &'static NSString {
    if active {
        ns_string!("\u{1F7E2} AKTIF") // 🟢 AKTIF
    } else {
        ns_string!("\u{1F512} PASIF") // 🔒 PASIF
    }
}

/// AKTİF iken CGAssociateMouseAndMouseCursorPosition(0) imleci donduruyor.
/// Çıkışta yeniden bağlamazsak imleç sistem genelinde donuk kalır.
fn reassociate_cursor() {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGAssociateMouseAndMouseCursorPosition(connected: i32) -> i32;
    }
    unsafe {
        let _ = CGAssociateMouseAndMouseCursorPosition(1);
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KbItQuitDelegate"]
    struct QuitDelegate;

    unsafe impl NSObjectProtocol for QuitDelegate {}

    unsafe impl NSApplicationDelegate for QuitDelegate {
        // terminate: (menüden Çıkış) çıkıştan hemen ÖNCE ana thread'de tetiklenir.
        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _n: &NSNotification) {
            reassociate_cursor();
        }
    }

    impl QuitDelegate {
        // Menü "Cikis" -> NSApp.terminate (applicationWillTerminate temizliği tetiklenir).
        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            let mtm = self.mtm();
            NSApplication::sharedApplication(mtm).terminate(None);
        }

        // Menü "Ayarlar..." -> config.toml'u editörde aç.
        #[unsafe(method(settings:))]
        fn settings(&self, _sender: Option<&AnyObject>) {
            let _ = protocol::config::Config::edit();
        }

        // Menü "Girişte Başlat" -> LaunchAgent'ı aç/kapat + tik işaretini güncelle.
        // sender = tıklanan NSMenuItem; setState ile tiki yansıtırız.
        #[unsafe(method(toggleStartup:))]
        fn toggle_startup(&self, sender: Option<&AnyObject>) {
            let now = crate::autostart::is_enabled();
            let _ = crate::autostart::set_enabled(!now);
            let enabled = crate::autostart::is_enabled();
            if let Some(item) = sender {
                // NSControlStateValue: On=1, Off=0.
                let state: isize = if enabled { 1 } else { 0 };
                unsafe {
                    let _: () = msg_send![item, setState: state];
                }
            }
        }
    }
);

impl QuitDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

/// Menü çubuğu kurulumunun sonucu. Status item + delegate'i CANLI tutmak için
/// çağıran bunları düşürmeden saklamalı — düşerse öğe menü çubuğundan kaybolur.
pub struct MenuBar {
    pub status_item: Retained<NSStatusItem>,
    _delegate: Retained<QuitDelegate>,
}

/// NSApplication'ı Accessory yap, status item + Çıkış menüsü kur, başlangıç
/// başlığını yaz. app.run() ÇAĞIRMAZ — çağıran tap kurulumundan sonra çağırır.
pub fn setup(mtm: MainThreadMarker, initial_active: bool) -> MenuBar {
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let bar = NSStatusBar::systemStatusBar();
    let status_item = bar.statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = status_item.button(mtm) {
        button.setTitle(title_for(initial_active));
    }

    let delegate = QuitDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    let menu = NSMenu::new(mtm);
    let settings = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("Ayarlar..."),
            Some(sel!(settings:)),
            ns_string!(","),
        )
    };
    unsafe { settings.setTarget(Some(&delegate)) };
    menu.addItem(&settings);

    let startup = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("Girişte Başlat"),
            Some(sel!(toggleStartup:)),
            ns_string!(""),
        )
    };
    unsafe { startup.setTarget(Some(&delegate)) };
    unsafe {
        // Açılışta gerçek durumu (LaunchAgent var mı) tik olarak yansıt.
        let state: isize = if crate::autostart::is_enabled() { 1 } else { 0 };
        let _: () = msg_send![&*startup, setState: state];
    }
    menu.addItem(&startup);

    let quit = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("Cikis"),
            Some(sel!(quit:)),
            ns_string!("q"),
        )
    };
    unsafe { quit.setTarget(Some(&delegate)) };
    menu.addItem(&quit);
    status_item.setMenu(Some(&menu));

    MenuBar {
        status_item,
        _delegate: delegate,
    }
}

/// Menü çubuğu başlığını periyodik olarak `active` bayrağına göre günceller.
/// Tap callback Send olmak zorunda (objc2 nesneleri !Send, callback'e taşınamaz),
/// bu yüzden başlığı callback yerine bu ANA-THREAD timer'ından güncelliyoruz.
pub fn install_status_updater(
    mtm: MainThreadMarker,
    status_item: Retained<NSStatusItem>,
    active: Arc<AtomicBool>,
) {
    let last = Cell::new(false);
    let block = RcBlock::new(move |_t: NonNull<NSTimer>| {
        let a = active.load(Ordering::Relaxed);
        if a != last.get() {
            last.set(a);
            if let Some(button) = status_item.button(mtm) {
                button.setTitle(title_for(a));
            }
        }
    });
    unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.15, true, &block);
    }
}
