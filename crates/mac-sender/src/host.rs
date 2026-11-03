//! Explicit opt-in input host inside the existing Mac bundle. Consent is shown
//! on the main thread; network workers never create AppKit objects.
use device_host::{inject, serve};
use objc2::MainThreadOnly;
use objc2_app_kit::{NSAlert, NSFont, NSPopUpButton, NSTextField};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
use protocol::{
    access::AccessStore,
    config::{Config, Role},
    pairing::PairRequest,
};
use std::{
    fs, io,
    sync::{mpsc, Arc, Mutex, OnceLock},
};
struct Pending {
    request: PairRequest,
    reply: mpsc::SyncSender<bool>,
}
struct Host {
    _listener: serve::Handle,
    access: Arc<AccessStore>,
}
static HOST: OnceLock<Mutex<Option<Host>>> = OnceLock::new();
static PENDING: OnceLock<Arc<Mutex<Option<Pending>>>> = OnceLock::new();
fn state() -> &'static Mutex<Option<Host>> {
    HOST.get_or_init(Default::default)
}
fn pending() -> Arc<Mutex<Option<Pending>>> {
    PENDING.get_or_init(|| Arc::new(Mutex::new(None))).clone()
}
pub fn running() -> bool {
    state().lock().unwrap().is_some()
}
pub fn stop() {
    state().lock().unwrap().take();
    if let Some(request) = pending().lock().unwrap().take() {
        let _ = request.reply.try_send(false);
    }
    inject::disconnect();
}
fn start() -> io::Result<()> {
    let path = Config::path()?.with_file_name("mobile-host.toml");
    let mut cfg = match fs::read_to_string(&path) {
        Ok(s) => Config::from_toml(&s)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Config::default(),
        Err(e) => return Err(e),
    };
    cfg.role = Role::Receiver;
    cfg.ensure_identity();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", protocol::secure::generate_key()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
    let result = (|| -> io::Result<()> {
        let mut file = options.open(&tmp)?;
        file.write_all(
            toml::to_string_pretty(&cfg)
                .map_err(io::Error::other)?
                .as_bytes(),
        )?;
        file.sync_all()?;
        fs::rename(&tmp, &path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;
    let access = AccessStore::open_path(&path.with_extension("access.toml"), &cfg.shared_secret)?;
    inject::enable_native()?;
    let requests = pending();
    let listener = serve::start_with_access(
        &cfg,
        access.clone(),
        |_| {},
        Arc::new(move |request| {
            let (tx, rx) = mpsc::sync_channel(1);
            {
                let mut pending = requests.lock().unwrap();
                if pending.is_some() {
                    return false;
                }
                *pending = Some(Pending {
                    request: request.clone(),
                    reply: tx,
                });
            }
            rx.recv_timeout(protocol::PAIR_DECISION_TIMEOUT)
                .unwrap_or(false)
        }),
    )?;
    *state().lock().unwrap() = Some(Host {
        _listener: listener,
        access,
    });
    Ok(())
}
pub fn toggle(mtm: MainThreadMarker) {
    if running() {
        stop();
        crate::menubar::show_alert(mtm,"Mac alıcısı kapatıldı","Gelen bağlantılar kapatıldı. Gönderici bağlantısını isterseniz menüden yeniden başlatabilirsiniz.");
        return;
    }
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str("Android’den bu Mac’i kontrol et"));
    alert.setInformativeText(&NSString::from_str("Yerel ağda klavye/fare alıcısı açılacak. Her yeni cihazda parmak izini karşılaştırıp bu Mac’te onay vermelisiniz. Bu sürüm Mac ekranını veya panosunu paylaşmaz. Mac’ten diğer bilgisayara gönderim duraklatılacak. Erişilebilirlik ve Yerel Ağ izinleri gereklidir."));
    alert.addButtonWithTitle(&NSString::from_str("Alıcıyı başlat"));
    alert.addButtonWithTitle(&NSString::from_str("Vazgeç"));
    if alert.runModal() != 1000 {
        return;
    }
    let paused_by_host = !crate::desktop::shared()
        .paused
        .load(std::sync::atomic::Ordering::SeqCst);
    if paused_by_host {
        crate::desktop::toggle_connection();
    }
    if let Err(e) = start() {
        if paused_by_host {
            crate::desktop::toggle_connection();
        }
        crate::menubar::show_alert(mtm, "Mac alıcısı başlatılamadı", &e.to_string());
    } else {
        crate::menubar::show_alert(mtm,"Mac alıcısı hazır","Android uygulamasında Cihaz ara’yı seçin. Kapatmak için aynı menü öğesine yeniden tıklayın. Bilgisayardaki erişimleri Mac alıcı erişimleri… bölümünden iptal edebilirsiniz.");
    }
}
pub fn poll(mtm: MainThreadMarker) {
    let request = pending().lock().unwrap().take();
    let Some(pending) = request else {
        return;
    };
    if !running() {
        let _ = pending.reply.try_send(false);
        return;
    }
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&format!(
        "{} bu Mac’i kontrol etmek istiyor",
        pending.request.peer_name
    )));
    alert.setInformativeText(&NSString::from_str("Android’deki parmak izinin SEKİZ grubunu aşağıdakiyle karşılaştırın. Tamamı aynı değilse reddedin. Onaylanan cihaz bu Mac’te yazabilir ve fareyi kullanabilir."));
    let groups = protocol::pairing::code_display(&pending.request.code);
    let groups: Vec<_> = groups.split_whitespace().collect();
    let code = format!("{}\n{}", groups[..4].join(" "), groups[4..].join(" "));
    let label = NSTextField::wrappingLabelWithString(&NSString::from_str(&code), mtm);
    label.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(15.0, 0.0)));
    label.setSelectable(true);
    label.setFrame(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(460.0, 62.0),
    ));
    alert.setAccessoryView(Some(&label));
    alert.addButtonWithTitle(&NSString::from_str("Reddet"));
    alert.addButtonWithTitle(&NSString::from_str("İzin ver"));
    let allow = alert.runModal() == 1001;
    if pending.reply.try_send(allow).is_err() {
        crate::menubar::show_alert(
            mtm,
            "İstek süresi doldu",
            "Erişim verilmedi. Android’den yeniden eşleştirin.",
        );
    }
}
pub fn access(mtm: MainThreadMarker) {
    let access = state().lock().unwrap().as_ref().map(|h| h.access.clone());
    let access = match access {
        Some(store) => store,
        None => {
            // Revocation must not require reopening network listeners first.
            let result = (|| -> io::Result<Arc<AccessStore>> {
                let path = Config::path()?.with_file_name("mobile-host.toml");
                let cfg = Config::from_toml(&fs::read_to_string(&path)?)?;
                AccessStore::open_path(&path.with_extension("access.toml"), &cfg.shared_secret)
            })();
            match result {
                Ok(store) => store,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    crate::menubar::show_alert(
                        mtm,
                        "Mac alıcı erişimleri",
                        "Henüz yetkilendirilmiş cihaz yok.",
                    );
                    return;
                }
                Err(e) => {
                    crate::menubar::show_alert(mtm, "Erişimler okunamadı", &e.to_string());
                    return;
                }
            }
        }
    };
    let controllers = access.controllers();
    if controllers.is_empty() {
        crate::menubar::show_alert(
            mtm,
            "Mac alıcı erişimleri",
            "Henüz yetkilendirilmiş cihaz yok.",
        );
        return;
    }
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str("Mac alıcı erişimleri"));
    alert.setInformativeText(&NSString::from_str("İptal edilen cihazın bağlantısı hemen kapatılır; tekrar erişim için yeniden eşleştirme gerekir."));
    let popup = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(420.0, 30.0)),
        false,
    );
    for c in &controllers {
        popup.addItemWithTitle(&NSString::from_str(&c.name));
    }
    alert.setAccessoryView(Some(&popup));
    alert.addButtonWithTitle(&NSString::from_str("Kapat"));
    alert.addButtonWithTitle(&NSString::from_str("Seçileni iptal et"));
    alert.addButtonWithTitle(&NSString::from_str("Tümünü iptal et"));
    let result = match alert.runModal() {
        1001 => controllers
            .get(popup.indexOfSelectedItem() as usize)
            .map(|c| access.revoke(&c.id)),
        1002 => Some(access.revoke_all()),
        _ => None,
    };
    if let Some(Err(e)) = result {
        crate::menubar::show_alert(mtm, "Erişim değiştirilemedi", &e.to_string());
    }
}

#[cfg(debug_assertions)]
pub fn check_ui(mtm: MainThreadMarker) {
    let code = "DDDDDDDD DDDDDDDD DDDDDDDD DDDDDDDD\nDDDDDDDD DDDDDDDD DDDDDDDD DDDDDDDD";
    let label = NSTextField::wrappingLabelWithString(&NSString::from_str(code), mtm);
    label.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(15.0, 0.0)));
    label.setFrame(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(460.0, 62.0),
    ));
    let alert = NSAlert::new(mtm);
    alert.setAccessoryView(Some(&label));
    assert_eq!(
        label.stringValue().to_string().split_whitespace().count(),
        8
    );
    assert!(!alert.window().isVisible());
}
