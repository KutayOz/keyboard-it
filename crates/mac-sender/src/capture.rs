//! macOS klavye yakalama + çift-tıklama-Fn toggle (M3).
//!
//! Durum makinesi:
//!   - PASİF (başlangıç): tuşlar Mac'te normal çalışır, Windows'a GÖNDERİLMEZ.
//!   - AKTİF: her tuş HID'e çevrilip Windows'a gider VE Mac'te BASTIRILIR (Drop).
//! Aç/kapa: Fn'e ~400 ms içinde iki kez basmak (çift-tıklama).
//!
//! İZİNLER (ikisi de gerekli):
//!   - Giriş İzleme (Input Monitoring): olayları görmek için.
//!   - Erişilebilirlik (Accessibility): AKTİF iken tuşları bastırmak (Drop) için.
//! İlk çalıştırmada ikisi de SİSTEMİN RESMİ istemleriyle istenir
//! (request_permissions_official — preflight sayesinde izin varsa istem çıkmaz).
//! ÖN KOŞUL: Sistem Ayarları > Klavye > "🌐/fn tuşuna basınca: Hiçbir şey yapma"
//!   (yoksa macOS çift-Fn'i Dikte için yer ve toggle'ı yiyebilir).
//!
//! GÜVENLİK: AKTİF iken Mac klavyesi bastırıldığından, kilitlenirsen fare hâlâ
//! çalışır — menüden  > Force Quit ile terminali kapatabilirsin. Çift-Fn ile de
//! her zaman PASİF'e dönersin.

use std::cell::RefCell;
use std::collections::HashSet;
use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use core_foundation::base::TCFType;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventType, CallbackResult, EventField,
};

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;

use objc2_app_kit::NSApplication;
use objc2_foundation::MainThreadMarker;

use protocol::{mousebtn, InputEvent, KeyEvent, MsgType};

use crate::keymap::mac_keycode_to_hid;
use crate::menubar::{self, ConnStatus};
use crate::net::connect_retry;

const FN_KEYCODE: i64 = 0x3F; // kVK_Function (Fn / 🌐 Globe)
const CAPSLOCK_KEYCODE: i64 = 0x39; // kVK_CapsLock — flagsChanged'de TOGGLE davranır
const DOUBLE_TAP: Duration = Duration::from_millis(400);

// Sistem tap'i kapattığında (TapDisabledByTimeout/ByUserInput) callback içinden
// yeniden açmak için ham FFI. core-graphics sarmalayıcısının enable()'ı tap
// handle'ından çağrılır ama handle callback'e taşınamaz (CFMachPort: !Send,
// CGEventTap::new ise Send closure ister); bu yüzden CFMachPortRef'i AtomicUsize
// içinde taşıyıp burada elle çağırıyoruz. Callback zaten ana thread'de koşar.
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapEnable(tap: *mut std::ffi::c_void, enable: bool);
}

// İlk çalıştırma izin akışı (macOS 10.15+): Preflight, Giriş İzleme iznini
// İSTEMSİZ yoklar (varsa true). Request, Apple'ın RESMİ izin diyaloğunu gösterir
// VE uygulamayı Sistem Ayarları > Gizlilik ve Güvenlik > Giriş İzleme listesine
// otomatik ekler — kullanıcı uygulamayı elle aramak zorunda kalmaz.
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

/// Erişilebilirlik (Accessibility) izni: kAXTrustedCheckOptionPrompt=true ile
/// çağrıldığında izin YOKSA Apple'ın resmi sistem diyaloğu çıkar ve uygulama
/// Erişilebilirlik listesine otomatik eklenir; izin ZATEN varsa hiçbir diyalog
/// çıkmadan true döner.
fn ax_trusted_with_prompt() -> bool {
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::{CFString, CFStringRef};

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        #[allow(non_upper_case_globals)]
        static kAXTrustedCheckOptionPrompt: CFStringRef;
        fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    }
    unsafe {
        // Get-rule: sistem sabitinin sahipliği bizde değil, retain edilir.
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let opts = CFDictionary::from_CFType_pairs(&[(
            key.as_CFType(),
            CFBoolean::true_value().as_CFType(),
        )]);
        AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef())
    }
}

/// İzinleri sistemin RESMİ istemleriyle iste (tap kurulmadan ÖNCE çağrılır).
/// İki istemi AYNI ANDA patlatmamak için sıralama:
///   - Giriş İzleme yoksa: yalnız onu iste (Apple diyaloğu). Erişilebilirlik
///     istemi bir SONRAKİ açılışa kalır — izin sonrası uygulamayı yeniden açmak
///     zaten gerekiyor (CGEventTap izni süreç başında değerlendirir).
///   - Giriş İzleme varsa: Erişilebilirlik eksikse onun resmi istemini göster.
/// İki izin de ZATEN verilmişse hiçbir diyalog çıkmaz (preflight bunun için).
fn request_permissions_official() {
    let listen_ok = unsafe { CGPreflightListenEventAccess() };
    if !listen_ok {
        let _ = unsafe { CGRequestListenEventAccess() };
        return;
    }
    let _ = ax_trusted_with_prompt();
}

/// Sistem Ayarları > Gizlilik ve Güvenlik > Giriş İzleme bölmesini doğrudan aç
/// ('İzin gerekli' diyaloğundaki 'Ayarları Aç' butonu için).
fn open_input_monitoring_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
        .spawn();
}

/// Kanal kapasitesi: bağlantı kopukken callback'ten gelen olaylar bu sınırın
/// üstünde ATILIR (try_send). Reconnect sonrası bayat tuş seli olmasın diye
/// gönderim thread'i bağlantı kurulunca kuyruğu ayrıca boşaltır (bulgu düzeltmesi).
const EVENT_QUEUE_CAP: usize = 128;

/// Bir modifier keycode'un down/up durumunu belirleyen CGEventFlags maskesi.
fn modifier_mask(kc: i64) -> Option<CGEventFlags> {
    let m = match kc {
        0x37 | 0x36 => CGEventFlags::CGEventFlagCommand,
        0x38 | 0x3C => CGEventFlags::CGEventFlagShift,
        0x3A | 0x3D => CGEventFlags::CGEventFlagAlternate,
        0x3B | 0x3E => CGEventFlags::CGEventFlagControl,
        0x39 => CGEventFlags::CGEventFlagAlphaShift,
        _ => return None,
    };
    Some(m)
}

struct State {
    active: bool,
    fn_down: bool,
    last_fn_press: Option<Instant>,
    held: HashSet<u16>, // AKTİF iken Windows'a Down gönderilmiş HID usage'lar
}

/// Mac imlecini fiziksel fareden AYIR/BAĞLA. `captured=true` iken imleç DONAR
/// (WindowServer artık fiziksel fareyle imleci hareket ettirmez) ama CGEventTap
/// hâlâ delta'ları görür — event Drop'lamak imleci durdurmadığından bu gerekli.
/// Kenar-clamp de kalkar, yani ekran kenarında bile delta akar. PASİF'te geri bağlanır.
#[cfg(target_os = "macos")]
fn set_mouse_captured(captured: bool) {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        // boolean_t (= int): connected=1 normal (bağlı), 0 = ayrık (imleç donar).
        fn CGAssociateMouseAndMouseCursorPosition(connected: i32) -> i32;
    }
    unsafe {
        let _ = CGAssociateMouseAndMouseCursorPosition(if captured { 0 } else { 1 });
    }
}

/// Sender-tarafı ilk kurulum tamam mı? (Anahtar config'te ya da env'de VE peer_host dolu.)
fn config_ready(cfg: &protocol::config::Config) -> bool {
    protocol::secure::psk_from_config_or_env(cfg).is_ok() && !cfg.peer_host.is_empty()
}

pub fn run(cfg: protocol::config::Config) -> io::Result<()> {
    // ÖNCE menü çubuğu + event tap (ana thread), bağlantı ARKA planda: Windows
    // kapalıyken/config boşken uygulama artık sessizce ölmez (bulgu düzeltmesi).
    let mtm = MainThreadMarker::new()
        .expect("run() ana thread'de çağrılmalı (AppKit ana thread ister)");
    let initial_conn =
        if config_ready(&cfg) { ConnStatus::Connecting } else { ConnStatus::ConfigNeeded };
    let menu_bar = menubar::setup(mtm, false, initial_conn);

    // Durum bayrakları: tap callback + bağlantı thread'i (Send olmalı) buraya yazar;
    // ana-thread timer okuyup menü çubuğu başlığını günceller (objc2 nesneleri !Send).
    let active_flag = Arc::new(AtomicBool::new(false));
    let conn_status = Arc::new(AtomicU8::new(initial_conn as u8));
    let permission_needed = Arc::new(AtomicBool::new(false));
    menubar::install_status_updater(
        mtm,
        menu_bar.status_item.clone(),
        active_flag.clone(),
        conn_status.clone(),
        permission_needed.clone(),
    );
    let flag_cb = active_flag.clone();

    println!("Durum: PASİF. Aç/kapa için Fn'e çift bas.");
    println!("(İzin: Giriş İzleme + Erişilebilirlik. Ön koşul: fn tuşu 'Hiçbir şey yapma'.)");
    println!("(Çıkış: Ctrl-C — ya da kilitlenirsen fareyle  > Force Quit.)");

    // Callback hafif kalsın: olayları SINIRLI kanala koy (try_send — dolarsa at);
    // ayrı thread TCP'ye framed yazar. Sınırsız kuyruk, kopuklukta biriken bayat
    // olayların reconnect'te sel gibi boşalmasına yol açıyordu (bulgu düzeltmesi).
    let (tx, rx) = mpsc::sync_channel::<InputEvent>(EVENT_QUEUE_CAP);
    let conn_bg = conn_status.clone();
    thread::spawn(move || loop {
        // Config'i HER denemede taze oku: 'Ayarlar...' ile değişen adres/anahtar
        // yeniden başlatma gerektirmeden bir sonraki denemede geçerli olur
        // (bulgu düzeltmesi). Bozuk/eksik dosya = 'Ayar gerekli'.
        let cfg = protocol::config::Config::load().ok().flatten().unwrap_or_default();
        let psk = match protocol::secure::psk_from_config_or_env(&cfg) {
            Ok(p) if !cfg.peer_host.is_empty() => p,
            _ => {
                conn_bg.store(ConnStatus::ConfigNeeded as u8, Ordering::Relaxed);
                for _ in rx.try_iter() {} // bağlantı yokken olayları at
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        let addr = cfg.peer_addr();
        conn_bg.store(ConnStatus::Connecting as u8, Ordering::Relaxed);
        let mut stream = match connect_retry(&addr) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("bağlanılamadı ({addr}): {e} — tekrar denenecek.");
                conn_bg.store(ConnStatus::Disconnected as u8, Ordering::Relaxed);
                for _ in rx.try_iter() {}
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        let mut transport = match protocol::secure::handshake_initiator(&mut stream, &psk) {
            Ok(t) => t,
            Err(e) => {
                // secure.rs yanlış anahtarı 'karşı taraf el sıkışmayı reddetti —
                // eşleşme anahtarları iki tarafta AYNI mı?' metniyle ayırt eder.
                eprintln!("el sıkışma başarısız: {e}");
                conn_bg.store(ConnStatus::HandshakeFailed as u8, Ordering::Relaxed);
                for _ in rx.try_iter() {}
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        println!("bağlandı (şifreli, Noise NNpsk0): {addr}");
        conn_bg.store(ConnStatus::Connected as u8, Ordering::Relaxed);
        // Kopukluk boyunca birikenler bayat — göndermeden önce kuyruğu boşalt.
        for _ in rx.try_iter() {}
        // Gönderim döngüsü — bağlantı kopana kadar.
        loop {
            match rx.recv() {
                Ok(ev) => {
                    if protocol::secure::send_event(&mut transport, &mut stream, &ev).is_err() {
                        eprintln!("bağlantı koptu — yeniden bağlanılıyor...");
                        conn_bg.store(ConnStatus::Disconnected as u8, Ordering::Relaxed);
                        break; // dış döngü yeniden bağlanır
                    }
                }
                Err(_) => return, // ana thread gitti (kanal kapandı)
            }
        }
    });

    // İlk açılış: config boşsa kullanıcıya GÖRÜNÜR yönerge (LSUIElement .app'te
    // stderr görünmez; eskiden sessizce çıkılıyordu — bulgu düzeltmesi).
    if initial_conn == ConnStatus::ConfigNeeded {
        let open = menubar::show_setup_alert(
            mtm,
            "keyboard-it — ilk kurulum",
            "Ayar dosyası henüz doldurulmamış.\n\n\
             config.toml içinde şunları doldur:\n\
             \u{2022} shared_secret: eşleşme anahtarı (Windows'takiyle AYNI)\n\
             \u{2022} peer_host: Windows PC'nin IP adresi\n\n\
             Dosyaya menü çubuğundaki keyboard-it simgesi \u{2192} 'Ayarlar...' ile de ulaşabilirsin.\n\
             Kaydettikten sonra uygulama kendiliğinden bağlanır (yeniden başlatma gerekmez).",
        );
        if open {
            let _ = protocol::config::Config::edit();
        }
    }

    let state = RefCell::new(State {
        active: false,
        fn_down: false,
        last_fn_press: None,
        held: HashSet::new(),
    });

    // İlk çalıştırma: tap kurulmadan ÖNCE izinleri sistemin resmi istemleriyle
    // yokla/iste — izin zaten varsa hiçbir diyalog çıkmaz, yoksa Apple'ın kendi
    // diyaloğu çıkar ve uygulama ilgili izin listesine otomatik eklenir.
    request_permissions_official();

    // Tap'in mach portu: callback TapDisabled* görünce CGEventTapEnable ile tap'i
    // YENİDEN açar (eskiden sadece stderr'e yazılıyor, toggle ölü kalıyordu —
    // bulgu düzeltmesi). Tap oluştuktan SONRA doldurulur (0 = henüz yok).
    let tap_port = Arc::new(AtomicUsize::new(0));
    let tap_port_cb = tap_port.clone();

    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        // AKTİF tap: callback'ten Drop dönerek tuşu yutabiliriz (Accessibility gerekir).
        CGEventTapOptions::Default,
        vec![
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
            // fare: hareket (düz + buton basılıyken drag)
            CGEventType::MouseMoved,
            CGEventType::LeftMouseDragged,
            CGEventType::RightMouseDragged,
            CGEventType::OtherMouseDragged,
            // fare: butonlar
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            // fare: scroll
            CGEventType::ScrollWheel,
        ],
        move |_proxy, event_type, event: &CGEvent| -> CallbackResult {
            // Sistem tap'i devre dışı bıraktıysa (timeout/user-input) hemen geri aç:
            // yoksa çift-Fn toggle sessizce ölür ve uygulama işlevsiz kalır.
            if matches!(
                event_type,
                CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
            ) {
                let port = tap_port_cb.load(Ordering::Relaxed);
                if port != 0 {
                    unsafe { CGEventTapEnable(port as *mut std::ffi::c_void, true) };
                    eprintln!("uyarı: event tap devre dışı kalmıştı ({event_type:?}) — yeniden etkinleştirildi.");
                } else {
                    eprintln!("uyarı: event tap devre dışı ({event_type:?}).");
                }
                return CallbackResult::Keep;
            }

            let kc = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            let mut st = state.borrow_mut();

            // --- Fn tuşu: çift-tıklama toggle algılama (durumdan bağımsız her zaman) ---
            if kc == FN_KEYCODE {
                let now_down = event.get_flags().contains(CGEventFlags::CGEventFlagSecondaryFn);
                if now_down && !st.fn_down {
                    // rising edge = bir "tık"
                    let is_double = matches!(st.last_fn_press, Some(t) if t.elapsed() <= DOUBLE_TAP);
                    if is_double {
                        st.last_fn_press = None;
                        st.active = !st.active;
                        if st.active {
                            set_mouse_captured(true); // Mac imlecini dondur
                            println!(">>> AKTİF — klavye+fare Windows'a gidiyor (Mac'te bastırılıyor).");
                        } else {
                            set_mouse_captured(false); // Mac imlecini geri bağla
                            // PASİF'e dönüş: Windows'ta basılı kalan tuşları serbest bırak.
                            let held: Vec<u16> = st.held.drain().collect();
                            for hid in held {
                                let _ = tx.try_send(InputEvent::Key(KeyEvent {
                                    msg: MsgType::Up,
                                    hid_usage: hid,
                                    modifiers: 0,
                                }));
                            }
                            println!("<<< PASİF — klavye+fare tekrar Mac'te.");
                        }
                        // Durum bayrağını güncelle; menü çubuğunu ana-thread timer yansıtır.
                        flag_cb.store(st.active, Ordering::Relaxed);
                    } else {
                        st.last_fn_press = Some(Instant::now());
                    }
                }
                st.fn_down = now_down;
                // Fn'in kendisi Windows'a gitmez (HID yok). AKTİF iken tüket, PASİF iken geçir.
                return if st.active { CallbackResult::Drop } else { CallbackResult::Keep };
            }

            // --- PASİF: Mac normal çalışsın, gönderme, bastırma ---
            if !st.active {
                return CallbackResult::Keep;
            }

            // --- AKTİF: çevir + gönder + bastır ---
            match event_type {
                CGEventType::KeyDown => {
                    let repeat = event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT);
                    if repeat == 0 {
                        if let Some(hid) = mac_keycode_to_hid(kc) {
                            st.held.insert(hid);
                            let _ = tx.try_send(InputEvent::Key(KeyEvent { msg: MsgType::Down, hid_usage: hid, modifiers: 0 }));
                        }
                    }
                }
                CGEventType::KeyUp => {
                    if let Some(hid) = mac_keycode_to_hid(kc) {
                        st.held.remove(&hid);
                        let _ = tx.try_send(InputEvent::Key(KeyEvent { msg: MsgType::Up, hid_usage: hid, modifiers: 0 }));
                    }
                }
                CGEventType::FlagsChanged => {
                    if kc == CAPSLOCK_KEYCODE {
                        // CapsLock: macOS flagsChanged'de AlphaShift bayrağı fiziksel
                        // down/up değil TOGGLE'dır — bayrağı Down/Up'a çevirmek iki
                        // tarafı desenkronize ediyordu. Her değişimde tek Down+Up
                        // çifti gönder: Windows'ta da tam bir kez toggle olur
                        // (bulgu düzeltmesi). held'e girmez (anında bırakılıyor).
                        if let Some(hid) = mac_keycode_to_hid(kc) {
                            let _ = tx.try_send(InputEvent::Key(KeyEvent { msg: MsgType::Down, hid_usage: hid, modifiers: 0 }));
                            let _ = tx.try_send(InputEvent::Key(KeyEvent { msg: MsgType::Up, hid_usage: hid, modifiers: 0 }));
                        }
                    } else if let (Some(hid), Some(mask)) = (mac_keycode_to_hid(kc), modifier_mask(kc)) {
                        let down = event.get_flags().contains(mask);
                        if down {
                            st.held.insert(hid);
                        } else {
                            st.held.remove(&hid);
                        }
                        let msg = if down { MsgType::Down } else { MsgType::Up };
                        let _ = tx.try_send(InputEvent::Key(KeyEvent { msg, hid_usage: hid, modifiers: 0 }));
                    }
                }

                // --- fare: RELATİF hareket (delta yalnızca move/drag'de anlamlı) ---
                CGEventType::MouseMoved
                | CGEventType::LeftMouseDragged
                | CGEventType::RightMouseDragged
                | CGEventType::OtherMouseDragged => {
                    let dx = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_X);
                    let dy = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y);
                    let dx = dx.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
                    let dy = dy.clamp(i16::MIN as i64, i16::MAX as i64) as i16;
                    if dx != 0 || dy != 0 {
                        let _ = tx.try_send(InputEvent::MouseMove { dx, dy });
                    }
                }

                // --- fare: sol/sağ butonlar (kendi olay tipleri) ---
                CGEventType::LeftMouseDown => {
                    let _ = tx.try_send(InputEvent::MouseButton { button: mousebtn::LEFT, down: true });
                }
                CGEventType::LeftMouseUp => {
                    let _ = tx.try_send(InputEvent::MouseButton { button: mousebtn::LEFT, down: false });
                }
                CGEventType::RightMouseDown => {
                    let _ = tx.try_send(InputEvent::MouseButton { button: mousebtn::RIGHT, down: true });
                }
                CGEventType::RightMouseUp => {
                    let _ = tx.try_send(InputEvent::MouseButton { button: mousebtn::RIGHT, down: false });
                }

                // --- fare: diğer butonlar (orta = numara 2; ekstralar şimdilik atlanır) ---
                CGEventType::OtherMouseDown | CGEventType::OtherMouseUp => {
                    let num = event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER);
                    let down = matches!(event_type, CGEventType::OtherMouseDown);
                    if num == 2 {
                        let _ = tx.try_send(InputEvent::MouseButton { button: mousebtn::MIDDLE, down });
                    }
                }

                // --- fare: scroll. Axis1=dikey, Axis2=yatay (tam-sayı tick). ---
                CGEventType::ScrollWheel => {
                    let v = event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1);
                    let h = event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2);
                    let dy = v.clamp(i8::MIN as i64, i8::MAX as i64) as i8;
                    let dx = h.clamp(i8::MIN as i64, i8::MAX as i64) as i8;
                    if dx != 0 || dy != 0 {
                        let _ = tx.try_send(InputEvent::Scroll { dx, dy });
                    }
                }

                _ => {}
            }
            CallbackResult::Drop // AKTİF iken tüm klavye+fare olaylarını Mac'ten bastır
        },
    );

    // Tap kurulamazsa (tipik neden: Giriş İzleme/Erişilebilirlik izni yok) ÇIKMA:
    // .app LSUIElement olduğundan stderr görünmezdi ve uygulama sessizce ölüyordu.
    // Bunun yerine görünür bir diyalog göster; uygulama menü çubuğunda 'İzin
    // gerekli' başlığıyla açık kalır (Ayarlar/Çıkış çalışmaya devam eder) —
    // bulgu düzeltmesi.
    let _tap = match tap {
        Ok(tap) => match tap.mach_port().create_runloop_source(0) {
            Ok(source) => {
                unsafe {
                    CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes);
                }
                // Portu callback'in erişeceği yuvaya koy (TapDisabled* kurtarması).
                tap_port.store(tap.mach_port().as_concrete_TypeRef() as usize, Ordering::Relaxed);
                tap.enable();
                println!("hazır. Fn'e çift bas → AKTİF; tekrar çift bas → PASİF. (Menü çubuğu: Cikis ile çık)");
                Some(tap)
            }
            Err(_) => {
                permission_needed.store(true, Ordering::Relaxed);
                menubar::show_alert(
                    mtm,
                    "keyboard-it — hata",
                    "Klavye yakalama başlatılamadı (run loop source oluşturulamadı).\n\
                     Uygulamayı kapatıp yeniden açmayı dene.",
                );
                None
            }
        },
        Err(_) => {
            permission_needed.store(true, Ordering::Relaxed);
            // İzin az önce resmi istemle verilmiş olsa bile CGEventTap için
            // uygulamanın YENİDEN AÇILMASI gerekir — metin bunu söylüyor.
            // 'Ayarları Aç' doğru bölmeyi (Giriş İzleme) doğrudan açar.
            let open = menubar::show_setup_alert(
                mtm,
                "keyboard-it — izin gerekli",
                "Klavye yakalama başlatılamadı — izin eksik.\n\n\
                 Az önce çıkan sistem isteminde izin verdiysen: macOS bu iznin\n\
                 etkinleşmesi için uygulamanın kapatılıp YENİDEN AÇILMASINI ister.\n\n\
                 İstem çıkmadıysa: Sistem Ayarları \u{2192} Gizlilik ve Güvenlik \u{2192}\n\
                 Giriş İzleme (ve Erişilebilirlik) bölümünde keyboard-it'i AÇIK\n\
                 konuma getir, sonra uygulamayı yeniden başlat.\n\n\
                 Uygulama menü çubuğunda 'İzin gerekli' olarak açık kalacak.",
            );
            if open {
                open_input_monitoring_settings();
            }
            None
        }
    };

    // Tap source'u ana run-loop'a EKLEDİKTEN sonra AppKit'i çalıştır. app.run()
    // aynı ana-thread CFRunLoop'unu sürer; source kCFRunLoopCommonModes'ta olduğundan
    // tap NSApp altında da tetiklenir.
    let app = NSApplication::sharedApplication(mtm);
    app.run();

    drop(menu_bar); // tutamaç canlılığı için buraya kadar taşı
    Ok(())
}
