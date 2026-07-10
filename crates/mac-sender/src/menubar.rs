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
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSMenu,
    NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSString, NSTimer,
};

/// Bağlantı durumu — win-receiver serve::ConnStatus ile parite
/// (Connected/Disconnected/HandshakeFailed) + göndericiye özgü Connecting ve
/// ConfigNeeded. AtomicU8 içinde taşınır: arka plan bağlantı thread'i yazar,
/// ana-thread timer okuyup menü çubuğu başlığına yansıtır (bulgu düzeltmesi).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ConnStatus {
    /// Eşleşme anahtarı ve/veya peer_host boş — 'Ayarlar...' ile doldurulmalı.
    ConfigNeeded = 0,
    /// Karşı tarafa TCP bağlantısı deneniyor.
    Connecting = 1,
    /// Şifreli kanal kuruldu.
    Connected = 2,
    /// Bağlantı kapandı/koptu; arka planda yeniden deneniyor.
    Disconnected = 3,
    /// El sıkışma reddedildi — büyük olasılıkla anahtarlar iki tarafta FARKLI
    /// (secure.rs 'karşı taraf el sıkışmayı reddetti' hatası).
    HandshakeFailed = 4,
}

impl ConnStatus {
    /// AtomicU8'den geri çevir (bilinmeyen değer = ConfigNeeded, en zararsızı).
    pub fn from_u8(v: u8) -> ConnStatus {
        match v {
            1 => ConnStatus::Connecting,
            2 => ConnStatus::Connected,
            3 => ConnStatus::Disconnected,
            4 => ConnStatus::HandshakeFailed,
            _ => ConnStatus::ConfigNeeded,
        }
    }
}

/// PASİF/AKTİF + bağlantı durumuna göre menü çubuğu başlığı (emoji + metin).
/// Bağlantı kopukken kullanıcı bunu menüden görebilsin (bulgu düzeltmesi).
pub fn title_for(active: bool, conn: ConnStatus) -> &'static NSString {
    match (conn, active) {
        (ConnStatus::ConfigNeeded, _) => ns_string!("\u{2699}\u{FE0F} Ayar gerekli"), // ⚙️
        (ConnStatus::HandshakeFailed, _) => ns_string!("\u{1F511} Anahtar uyuşmuyor"), // 🔑
        (ConnStatus::Connecting, false) => ns_string!("\u{23F3} Bağlanıyor…"), // ⏳
        (ConnStatus::Connecting, true) => ns_string!("\u{26A0}\u{FE0F} AKTIF (bağlanıyor…)"), // ⚠️
        (ConnStatus::Disconnected, false) => ns_string!("\u{1F50C} Bağlantı yok"), // 🔌
        (ConnStatus::Disconnected, true) => ns_string!("\u{26A0}\u{FE0F} AKTIF (bağlantı yok)"), // ⚠️
        (ConnStatus::Connected, true) => ns_string!("\u{1F7E2} AKTIF"), // 🟢
        (ConnStatus::Connected, false) => ns_string!("\u{1F512} PASIF"), // 🔒
    }
}

/// Kullanıcıya modal bilgi diyaloğu (NSAlert). LSUIElement .app'te stderr/stdout
/// GÖRÜNMEZ — ilk kurulum / izin / hata gibi durumları kullanıcıya söylemenin
/// görünür tek yolu bu (bulgu düzeltmesi).
pub fn show_alert(mtm: MainThreadMarker, title: &str, text: &str) {
    // Accessory (Dock'suz) uygulamada diyalog arka planda kalmasın: öne getir.
    let app = NSApplication::sharedApplication(mtm);
    unsafe {
        let _: () = msg_send![&*app, activateIgnoringOtherApps: true];
    }
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(text));
    let _ = alert.runModal();
}

/// İki butonlu diyalog: 'Ayarları Aç' / 'Daha Sonra'. true = kullanıcı ayarları
/// açmak istedi; NE açılacağına çağıran karar verir (ilk kurulumda config.toml,
/// izin diyaloğunda Sistem Ayarları > Giriş İzleme bölmesi).
pub fn show_setup_alert(mtm: MainThreadMarker, title: &str, text: &str) -> bool {
    let app = NSApplication::sharedApplication(mtm);
    unsafe {
        let _: () = msg_send![&*app, activateIgnoringOtherApps: true];
    }
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(text));
    let _ = alert.addButtonWithTitle(ns_string!("Ayarları Aç"));
    let _ = alert.addButtonWithTitle(ns_string!("Daha Sonra"));
    // İlk eklenen buton = NSAlertFirstButtonReturn.
    alert.runModal() == objc2_app_kit::NSAlertFirstButtonReturn
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
pub fn setup(mtm: MainThreadMarker, initial_active: bool, initial_conn: ConnStatus) -> MenuBar {
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let bar = NSStatusBar::systemStatusBar();
    let status_item = bar.statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = status_item.button(mtm) {
        // Başlangıç durumu: bağlantıyı arka plan thread'i kurar; config eksikse
        // çağıran ConfigNeeded verir ('Ayar gerekli' görünür).
        button.setTitle(title_for(initial_active, initial_conn));
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

/// Menü çubuğu başlığını periyodik olarak `active` + `conn` (+ izin) durumuna göre
/// günceller. Tap callback ve bağlantı thread'i Send olmak zorunda (objc2 nesneleri
/// !Send, oralara taşınamaz), bu yüzden başlığı bu ANA-THREAD timer'ından güncelliyoruz.
pub fn install_status_updater(
    mtm: MainThreadMarker,
    status_item: Retained<NSStatusItem>,
    active: Arc<AtomicBool>,
    conn: Arc<AtomicU8>,
    permission_needed: Arc<AtomicBool>,
) {
    // None = ilk tikte kesin bir kez yaz (setup'taki başlangıç başlığı ne olursa olsun).
    let last: Cell<Option<(bool, u8, bool)>> = Cell::new(None);
    let block = RcBlock::new(move |_t: NonNull<NSTimer>| {
        let now = (
            active.load(Ordering::Relaxed),
            conn.load(Ordering::Relaxed),
            permission_needed.load(Ordering::Relaxed),
        );
        if last.get() != Some(now) {
            last.set(Some(now));
            if let Some(button) = status_item.button(mtm) {
                // İzin eksikse hiçbir şey yakalanamaz — bu her durumun önüne geçer.
                let title = if now.2 {
                    ns_string!("\u{26D4} İzin gerekli") // ⛔
                } else {
                    title_for(now.0, ConnStatus::from_u8(now.1))
                };
                button.setTitle(title);
            }
        }
    });
    unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.15, true, &block);
    }
}
