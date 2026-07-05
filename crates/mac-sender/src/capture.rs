//! REFERANS — HENÜZ DERLENMİYOR.
//!
//! Bu dosya `main.rs`'te `mod capture;` ile bağlanmadığı için Cargo onu derlemez.
//! M2 aşamasında etkinleştirip bu Mac üzerinde compile-check edeceğiz; core-graphics
//! 0.25 API imzaları o an doğrulanacak (özellikle callback ve run loop source kısmı).
//!
//! Akış: CGEventTap yakalar -> macOS keycode'unu HID usage'a çevirir ->
//! `protocol::KeyEvent::encode()` ile 5 bayta paketler -> TCP'ye yazar.
//! Çift-tıklama-Fn toggle'ı da BURADA, `FlagsChanged` + `CGEventFlagSecondaryFn`
//! biti ile yakalanır ve tele GİTMEZ.

use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventType, CallbackResult, EventField,
};

pub fn run() {
    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        // AKTİF tap -> callback'ten Drop dönerek tuşu tüketebiliriz.
        // ListenOnly olsaydı Mac'in de yazmasını engelleyemezdik.
        CGEventTapOptions::Default,
        vec![
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
        ],
        |_proxy, event_type, event: &CGEvent| -> CallbackResult {
            match event_type {
                CGEventType::KeyDown => {
                    let keycode =
                        event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
                    println!("keyDown keycode = {}", keycode);
                    // TODO(M2): keycode -> HID usage çevir, protocol::KeyEvent::encode() ile
                    // 5 bayta paketle, TCP'ye yaz. Toggle açıkken CallbackResult::Drop dön
                    // (Mac yazmasın); kapalıyken Keep.
                    CallbackResult::Keep
                }
                CGEventType::FlagsChanged => {
                    let flags = event.get_flags();
                    let fn_down = flags.contains(CGEventFlags::CGEventFlagSecondaryFn);
                    println!("flagsChanged: Fn/Globe basılı = {}", fn_down);
                    // TODO(M3): rising-edge + ~300-400ms zamanlayıcı ile çift-tıklama-Fn
                    // toggle'ını burada yakala. Fn olaylarını tele GÖNDERME.
                    CallbackResult::Keep
                }
                _ => CallbackResult::Keep,
            }
        },
    )
    .expect("event tap oluşturulamadı (Accessibility + Input Monitoring izni gerekli)");

    let source = tap
        .mach_port()
        .create_runloop_source(0)
        .expect("run loop source oluşturulamadı");

    let run_loop = CFRunLoop::get_current();
    unsafe {
        run_loop.add_source(&source, kCFRunLoopCommonModes);
    }
    tap.enable();

    println!("event tap çalışıyor; tuşlara basın (çıkış: Ctrl-C)");
    CFRunLoop::run_current(); // bu thread'i bloklar; gerçek uygulamada ağ I/O ayrı thread'de olmalı
}
