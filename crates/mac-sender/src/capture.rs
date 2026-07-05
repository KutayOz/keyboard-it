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

use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventType, CallbackResult, EventField,
};

use protocol::{KeyEvent, MsgType};

use crate::keymap::mac_keycode_to_hid;
use crate::net::connect_retry;

const FN_KEYCODE: i64 = 0x3F; // kVK_Function (Fn / 🌐 Globe)
const DOUBLE_TAP: Duration = Duration::from_millis(400);

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

pub fn run(addr: String) -> io::Result<()> {
    // Anahtarı ağa dokunmadan ÖNCE türet (eksikse hemen dur).
    let psk = protocol::secure::psk_from_env()?;

    println!("bağlanılıyor: {addr}");
    let mut stream = connect_retry(&addr)?;
    println!("bağlandı.");

    // Noise (NNpsk0) el sıkışması — sender thread spawn'ından ÖNCE, ana thread stream'e sahipken.
    let mut transport = protocol::secure::handshake_initiator(&mut stream, &psk)?;
    println!("şifreli kanal kuruldu (Noise NNpsk0).");

    println!("Durum: PASİF. Aç/kapa için Fn'e çift bas.");
    println!("(İzin: Giriş İzleme + Erişilebilirlik. Ön koşul: fn tuşu 'Hiçbir şey yapma'.)");
    println!("(Çıkış: Ctrl-C — ya da kilitlenirsen fareyle  > Force Quit.)");

    // Callback hafif kalsın: olayları kanala koy; ayrı thread TCP'ye framed yazar.
    let (tx, rx) = mpsc::channel::<KeyEvent>();
    thread::spawn(move || {
        // stream VE transport bu thread'e taşınır (TransportState: Send — doğrulandı).
        for ev in rx {
            if protocol::secure::send_event(&mut transport, &mut stream, &ev).is_err() {
                eprintln!("bağlantı koptu — gönderici thread duruyor.");
                break;
            }
        }
    });

    let state = RefCell::new(State {
        active: false,
        fn_down: false,
        last_fn_press: None,
        held: HashSet::new(),
    });

    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        // AKTİF tap: callback'ten Drop dönerek tuşu yutabiliriz (Accessibility gerekir).
        CGEventTapOptions::Default,
        vec![
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
        ],
        move |_proxy, event_type, event: &CGEvent| -> CallbackResult {
            // Sistem tap'i devre dışı bıraktıysa (timeout/user-input) sadece geçir.
            if matches!(
                event_type,
                CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
            ) {
                eprintln!("uyarı: event tap devre dışı ({event_type:?}) — gerekirse yeniden başlat.");
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
                            println!(">>> AKTİF — klavye Windows'a gidiyor (Mac'te bastırılıyor).");
                        } else {
                            // PASİF'e dönüş: Windows'ta basılı kalan tuşları serbest bırak.
                            let held: Vec<u16> = st.held.drain().collect();
                            for hid in held {
                                let _ = tx.send(KeyEvent { msg: MsgType::Up, hid_usage: hid, modifiers: 0 });
                            }
                            println!("<<< PASİF — klavye tekrar Mac'te.");
                        }
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
                            let _ = tx.send(KeyEvent { msg: MsgType::Down, hid_usage: hid, modifiers: 0 });
                        }
                    }
                }
                CGEventType::KeyUp => {
                    if let Some(hid) = mac_keycode_to_hid(kc) {
                        st.held.remove(&hid);
                        let _ = tx.send(KeyEvent { msg: MsgType::Up, hid_usage: hid, modifiers: 0 });
                    }
                }
                CGEventType::FlagsChanged => {
                    if let (Some(hid), Some(mask)) = (mac_keycode_to_hid(kc), modifier_mask(kc)) {
                        let down = event.get_flags().contains(mask);
                        if down {
                            st.held.insert(hid);
                        } else {
                            st.held.remove(&hid);
                        }
                        let msg = if down { MsgType::Down } else { MsgType::Up };
                        let _ = tx.send(KeyEvent { msg, hid_usage: hid, modifiers: 0 });
                    }
                }
                _ => {}
            }
            CallbackResult::Drop // AKTİF iken tüm tuşları Mac'ten bastır
        },
    )
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::Other,
            "CGEventTap oluşturulamadı (Giriş İzleme + Erişilebilirlik izni verildi mi?)",
        )
    })?;

    let source = tap
        .mach_port()
        .create_runloop_source(0)
        .map_err(|_| io::Error::new(io::ErrorKind::Other, "run loop source oluşturulamadı"))?;

    unsafe {
        CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes);
    }
    tap.enable();
    println!("hazır. Fn'e çift bas → AKTİF; tekrar çift bas → PASİF.");
    CFRunLoop::run_current();
    Ok(())
}
