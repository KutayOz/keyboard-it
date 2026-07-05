//! macOS klavye yakalama: CGEventTap ile keyDown/keyUp/flagsChanged dinler,
//! her tuşu HID usage'a çevirip TCP kanalına koyar. Ayrı bir thread bunları
//! win-receiver'a framed olarak gönderir.
//!
//! M2: yakala + çevir + ilet, SÜREKLİ AÇIK ve BASTIRMADAN (ListenOnly) — yani
//! yazdıkların hem Mac'te hem Windows'ta görünür. Bastırma + çift-tıklama-Fn
//! toggle M3'te gelecek.
//!
//! İzin: Sistem Ayarları > Gizlilik ve Güvenlik > "Giriş İzleme" (Input Monitoring)
//! altında bu programı çalıştıran uygulamaya (ör. Terminal) izin verilmeli.

use std::io;
use std::sync::mpsc;
use std::thread;

use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventType, CallbackResult, EventField,
};

use protocol::{KeyEvent, MsgType};

use crate::keymap::mac_keycode_to_hid;
use crate::net::connect_retry;

/// Bir modifier keycode'un down/up durumunu belirlemek için ilgili CGEventFlags maskesi.
/// (Modifierlar keyDown değil, flagsChanged olarak gelir.)
fn modifier_mask(kc: i64) -> Option<CGEventFlags> {
    let m = match kc {
        0x37 | 0x36 => CGEventFlags::CGEventFlagCommand,   // Left/Right Command
        0x38 | 0x3C => CGEventFlags::CGEventFlagShift,     // Left/Right Shift
        0x3A | 0x3D => CGEventFlags::CGEventFlagAlternate, // Left/Right Option
        0x3B | 0x3E => CGEventFlags::CGEventFlagControl,   // Left/Right Control
        0x39 => CGEventFlags::CGEventFlagAlphaShift,       // CapsLock
        _ => return None,
    };
    Some(m)
}

pub fn run(addr: String) -> io::Result<()> {
    println!("bağlanılıyor: {addr}");
    let mut stream = connect_retry(&addr)?;
    println!("bağlandı. Klavye yakalanıyor — yazdıkların Windows'a gidecek.");
    println!("(İZİN gerekli: Sistem Ayarları > Gizlilik ve Güvenlik > Giriş İzleme.)");
    println!("(Çıkış: Ctrl-C)");

    // Callback hafif kalsın diye olayları kanala koy; ayrı thread TCP'ye yazar.
    let (tx, rx) = mpsc::channel::<KeyEvent>();

    thread::spawn(move || {
        for ev in rx {
            if ev.write_framed(&mut stream).is_err() {
                eprintln!("bağlantı koptu — gönderici thread duruyor.");
                break;
            }
        }
    });

    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        // M2: bastırma YOK — sadece dinle. (M3'te aktif tap + bastırma.)
        CGEventTapOptions::ListenOnly,
        vec![
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
        ],
        move |_proxy, event_type, event: &CGEvent| {
            let kc = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            match event_type {
                CGEventType::KeyDown => {
                    // Otomatik tekrarları atla — Windows kendi tekrarını üretsin.
                    let repeat = event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT);
                    if repeat == 0 {
                        if let Some(hid) = mac_keycode_to_hid(kc) {
                            let _ = tx.send(KeyEvent { msg: MsgType::Down, hid_usage: hid, modifiers: 0 });
                        }
                    }
                }
                CGEventType::KeyUp => {
                    if let Some(hid) = mac_keycode_to_hid(kc) {
                        let _ = tx.send(KeyEvent { msg: MsgType::Up, hid_usage: hid, modifiers: 0 });
                    }
                }
                CGEventType::FlagsChanged => {
                    if let (Some(hid), Some(mask)) = (mac_keycode_to_hid(kc), modifier_mask(kc)) {
                        let down = event.get_flags().contains(mask);
                        let msg = if down { MsgType::Down } else { MsgType::Up };
                        let _ = tx.send(KeyEvent { msg, hid_usage: hid, modifiers: 0 });
                    }
                }
                _ => {}
            }
            CallbackResult::Keep // ListenOnly: dönüş yok sayılır; olayı değiştirmiyoruz.
        },
    )
    .map_err(|_| io::Error::new(io::ErrorKind::Other, "CGEventTap oluşturulamadı (Giriş İzleme izni verildi mi?)"))?;

    let source = tap
        .mach_port()
        .create_runloop_source(0)
        .map_err(|_| io::Error::new(io::ErrorKind::Other, "run loop source oluşturulamadı"))?;

    unsafe {
        CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes);
    }
    tap.enable();
    println!("hazır — yaz!");
    CFRunLoop::run_current(); // bloklar
    Ok(())
}
