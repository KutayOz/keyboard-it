//! Android setup is explicit; no adb commands run merely because the app starts.
//! The mirror is an owned scrcpy process and is cancelled on every target change.
#![allow(non_snake_case)]
use android_adapter::{Adapter, AndroidDevice, DeviceState};
use objc2::{
    define_class, msg_send, rc::Retained, runtime::AnyObject, sel, DefinedClass, MainThreadOnly,
};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSPopUpButton, NSSecureTextField, NSTextField,
    NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    NSTimer,
};
use protocol::config::Config;
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};

static SESSIONS: OnceLock<android_adapter::session::Sessions> = OnceLock::new();
static PENDING_ERROR: Mutex<Option<(u64, String)>> = Mutex::new(None);
pub static SELECTED: AtomicBool = AtomicBool::new(false);
fn sessions() -> &'static android_adapter::session::Sessions {
    SESSIONS.get_or_init(Default::default)
}
pub fn stop() {
    sessions().cancel();
}
pub fn status() -> android_adapter::session::Status {
    sessions().status()
}
fn report_error(generation: u64, message: &str) {
    if status().generation == generation {
        *PENDING_ERROR.lock().unwrap() = Some((generation, message.into()));
    }
}
pub fn show_pending_error(mtm: MainThreadMarker) {
    let error = PENDING_ERROR.lock().unwrap().take();
    if let Some((generation, message)) = error {
        if status().generation == generation {
            crate::menubar::show_alert(mtm, "Android bağlantısı", &message);
        }
    }
}
pub fn update_selection(cfg: &Config) {
    SELECTED.store(cfg.android_selected(), Ordering::SeqCst);
}
pub fn open_viewer(mtm: MainThreadMarker) {
    if crate::desktop::shared().paused.load(Ordering::SeqCst) {
        crate::menubar::show_alert(
            mtm,
            "Bağlantı duraklatıldı",
            "Menüde Bağlantıyı duraklat seçeneğini kapatıp yeniden deneyin.",
        );
        return;
    }
    let result = Config::load()
        .and_then(|c| c.ok_or_else(|| std::io::Error::other("Önce Android cihazı ekleyin.")));
    let cfg = match result {
        Ok(c) => c,
        Err(e) => {
            crate::menubar::show_alert(mtm, "Android", &e.to_string());
            return;
        }
    };
    let Some(device) = cfg.active_device().filter(|d| d.is_android()).cloned() else {
        return;
    };
    if !device.permissions.screen {
        crate::menubar::show_alert(
            mtm,
            "Ekran izni kapalı",
            "Cihazlarım ekranından bu cihazın Ekran iznini açın.",
        );
        return;
    }
    if status().running || status().starting {
        return;
    }
    let target = cfg.session_target();
    let generation = sessions().begin();
    std::thread::spawn(move || {
        let result = (|| -> std::io::Result<()> {
            let command = Adapter::local()?.mirror_command(&device, cfg.stream)?;
            if Config::load()?.is_none_or(|c| {
                c.session_target() != target || c.permissions() != device.permissions
            }) {
                sessions().fail(
                    generation,
                    "Android seçimi değişti; Ekranı Aç ile yeniden başlatın.",
                );
                return Ok(());
            }
            if !sessions().launch(generation, &command)? {
                return Ok(());
            }
            loop {
                let status = status();
                if status.generation != generation || !status.running {
                    if status.generation == generation && status.failed {
                        report_error(generation, &status.message);
                    }
                    break;
                }
                if Config::load()?.is_none_or(|c| {
                    c.session_target() != target || c.permissions() != device.permissions
                }) {
                    sessions().cancel_if_current(generation);
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(())
        })();
        if let Err(e) = result {
            sessions().fail(generation, &e.to_string());
            report_error(generation, &e.to_string());
        }
    });
}

#[derive(Default)]
struct SetupState {
    devices: Vec<AndroidDevice>,
    message: String,
    busy: bool,
}
struct Ivars {
    window: Retained<NSWindow>,
    popup: Retained<NSPopUpButton>,
    refresh: Retained<NSButton>,
    save: Retained<NSButton>,
    pair: Retained<NSButton>,
    pair_address: Retained<NSTextField>,
    connect_address: Retained<NSTextField>,
    code: Retained<NSSecureTextField>,
    status: Retained<NSTextField>,
    shown: RefCell<Vec<AndroidDevice>>,
    state: Arc<Mutex<SetupState>>,
}
thread_local! { static SETUP: RefCell<Option<Retained<AndroidSetup>>> = const { RefCell::new(None) }; }
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KbItAndroidSetup"]
    #[ivars = Ivars]
    struct AndroidSetup;
    unsafe impl NSObjectProtocol for AndroidSetup {}
    impl AndroidSetup {
        #[unsafe(method(refresh:))]
        fn refresh(&self, _sender: Option<&AnyObject>) {
            let state = self.ivars().state.clone();
            if !begin(&state, "Android cihazları aranıyor…") { return; }
            self.render();
            std::thread::spawn(move || {
                let result = Adapter::local().and_then(|a| a.discover().map(|d| (d, a.tools.scrcpy.is_some())));
                let mut s = state.lock().unwrap();
                s.busy = false;
                match result {
                    Ok((devices, mirror)) => {
                        s.message = if !mirror { "ADB hazır; ekranı açmak için scrcpy de kurun." } else if devices.is_empty() {
                            "Android bulunamadı. USB kablosunu, hata ayıklamayı ve telefondaki izni kontrol edin."
                        } else { "Hazır cihazı seçip Kaydet'e basın. Ardından Cihazlarım listesinden seçip Ekranı Aç'ı kullanın." }.into();
                        s.devices = devices;
                    }
                    Err(e) => { s.devices.clear(); s.message = e.to_string(); }
                }
            });
        }
        #[unsafe(method(selected:))]
        fn selected(&self, _sender: Option<&AnyObject>) { self.render(); }
        #[unsafe(method(save:))]
        fn save(&self, _sender: Option<&AnyObject>) {
            let index = self.ivars().popup.indexOfSelectedItem();
            let Some(device) = index.checked_sub(1).and_then(|i| self.ivars().shown.borrow().get(i as usize).cloned()) else { return; };
            if device.state != DeviceState::Ready { return; }
            let state = self.ivars().state.clone();
            if !begin(&state, "Android kimliği doğrulanıp kaydediliyor…") { return; }
            self.render();
            std::thread::spawn(move || {
                let result = Adapter::local().and_then(|a| a.saveable(&device)).and_then(|d| {
                    Config::transaction(|c| { c.remember_device(d)?; Ok(()) })
                });
                let mut s = state.lock().unwrap(); s.busy = false;
                s.message = match result { Ok(()) => "Android kaydedildi. Cihazlarım listesinden seçip Ekranı Aç'ı kullanın.".into(), Err(e) => e.to_string() };
            });
        }
        #[unsafe(method(pairWifi:))]
        fn pair_wifi(&self, _sender: Option<&AnyObject>) {
            let iv = self.ivars();
            let pair_address = iv.pair_address.stringValue().to_string();
            let connect_address = iv.connect_address.stringValue().to_string();
            let code = iv.code.stringValue().to_string();
            let checked = android_adapter::endpoint_address(&pair_address).and_then(|_| android_adapter::endpoint_address(&connect_address));
            if let Err(e) = checked { iv.state.lock().unwrap().message = e.to_string(); self.render(); return; }
            let state = iv.state.clone();
            if !begin(&state, "Android Wi-Fi eşleştirmesi yapılıyor…") { return; }
            iv.code.setStringValue(ns_string!(""));
            self.render();
            std::thread::spawn(move || {
                let result = Adapter::local().and_then(|a| { a.pair_wifi(&pair_address, &code)?; a.connect_wifi(&connect_address)?; a.discover() });
                let mut s = state.lock().unwrap(); s.busy = false;
                match result {
                    Ok(devices) => { s.devices = devices; s.message = "Wi-Fi bağlantısı kuruldu. Listeden hazır cihazı seçip kaydedin.".into(); }
                    Err(e) => s.message = e.to_string(),
                }
            });
        }
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: Option<&AnyObject>) { if self.ivars().window.isVisible() { self.render(); } }
    }
);
fn begin(state: &Mutex<SetupState>, message: &str) -> bool {
    let mut s = state.lock().unwrap();
    if s.busy {
        return false;
    }
    s.busy = true;
    s.message = message.into();
    true
}
impl AndroidSetup {
    fn create(mtm: MainThreadMarker) -> Retained<Self> {
        let rect = |x, y, w, h| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, 640.0, 440.0),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setTitle(ns_string!("keyboard-it — Android ekle"));
        window.center();
        let content = window.contentView().unwrap();
        let label = |text: &str, x, y, w, h| {
            let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
            l.setFrame(rect(x, y, w, h));
            content.addSubview(&l);
            l
        };
        label(
            "Telefonda USB hata ayıklamayı açın ve bu bilgisayara izin verin.",
            16.0,
            396.0,
            608.0,
            24.0,
        );
        let popup = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(16.0, 350.0, 608.0, 26.0),
            false,
        );
        content.addSubview(&popup);
        let button = |text: &str, x, y, w| {
            let b = unsafe {
                NSButton::buttonWithTitle_target_action(&NSString::from_str(text), None, None, mtm)
            };
            b.setFrame(rect(x, y, w, 32.0));
            content.addSubview(&b);
            b
        };
        let refresh = button("Listeyi Yenile", 16.0, 302.0, 150.0);
        let save = button("Seçili Android'i Kaydet", 404.0, 302.0, 220.0);
        label(
            "Kablosuz hata ayıklama (Android 11+)",
            16.0,
            252.0,
            608.0,
            24.0,
        );
        label("Eşleştirme IP:port", 16.0, 213.0, 146.0, 20.0);
        label("Kod", 386.0, 213.0, 44.0, 20.0);
        label("Bağlantı IP:port", 16.0, 175.0, 146.0, 20.0);
        let field = |x, y, w| {
            let f = NSTextField::textFieldWithString(ns_string!(""), mtm);
            f.setFrame(rect(x, y, w, 24.0));
            f.setPlaceholderString(Some(ns_string!("192.168.1.10:port")));
            content.addSubview(&f);
            f
        };
        let pair_address = field(162.0, 209.0, 210.0);
        let connect_address = field(162.0, 171.0, 210.0);
        let code = NSSecureTextField::initWithFrame(
            NSSecureTextField::alloc(mtm),
            rect(430.0, 209.0, 194.0, 24.0),
        );
        content.addSubview(&code);
        let pair = button("Wi-Fi Eşleştir ve Bağlan", 386.0, 165.0, 238.0);
        label(
            "Eşleştirme ve bağlantı portları telefonda ayrı gösterilir.",
            16.0,
            129.0,
            608.0,
            24.0,
        );
        let status = label(
            "Listeyi yenileyin veya Wi-Fi bilgilerini girin.",
            16.0,
            40.0,
            608.0,
            66.0,
        );
        status.setMaximumNumberOfLines(3);
        let this = Self::alloc(mtm).set_ivars(Ivars {
            window,
            popup: popup.clone(),
            refresh: refresh.clone(),
            save: save.clone(),
            pair: pair.clone(),
            pair_address,
            connect_address,
            code,
            status,
            shown: RefCell::new(Vec::new()),
            state: Arc::new(Mutex::new(SetupState::default())),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        unsafe {
            refresh.setTarget(Some(&*this));
            refresh.setAction(Some(sel!(refresh:)));
            save.setTarget(Some(&*this));
            save.setAction(Some(sel!(save:)));
            pair.setTarget(Some(&*this));
            pair.setAction(Some(sel!(pairWifi:)));
            popup.setTarget(Some(&*this));
            popup.setAction(Some(sel!(selected:)));
            let _ = NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                0.2,
                &*this,
                sel!(tick:),
                None,
                true,
            );
        }
        this.render();
        this
    }
    fn render(&self) {
        let iv = self.ivars();
        let s = iv.state.lock().unwrap();
        if s.devices != *iv.shown.borrow() {
            let previous = iv.popup.indexOfSelectedItem();
            let selected = previous
                .checked_sub(1)
                .and_then(|i| iv.shown.borrow().get(i as usize).map(|d| d.serial.clone()));
            iv.popup.removeAllItems();
            iv.popup
                .addItemWithTitle(ns_string!("Android cihazı seçin…"));
            for d in &s.devices {
                iv.popup.addItemWithTitle(&NSString::from_str(&format!(
                    "{} — {} — {}",
                    d.name,
                    d.serial,
                    d.state.label()
                )));
            }
            if let Some(i) = s
                .devices
                .iter()
                .position(|d| Some(&d.serial) == selected.as_ref())
            {
                iv.popup.selectItemAtIndex((i + 1) as isize);
            }
            *iv.shown.borrow_mut() = s.devices.clone();
        } else if iv.popup.numberOfItems() == 0 {
            iv.popup
                .addItemWithTitle(ns_string!("Android cihazı seçin…"));
        }
        iv.popup.setEnabled(!s.busy && !s.devices.is_empty());
        iv.refresh.setEnabled(!s.busy);
        iv.pair.setEnabled(!s.busy);
        iv.pair_address.setEnabled(!s.busy);
        iv.connect_address.setEnabled(!s.busy);
        iv.code.setEnabled(!s.busy);
        let index = iv.popup.indexOfSelectedItem();
        iv.save.setEnabled(
            !s.busy
                && index
                    .checked_sub(1)
                    .and_then(|i| s.devices.get(i as usize))
                    .is_some_and(|d| d.state == DeviceState::Ready),
        );
        iv.status.setStringValue(&NSString::from_str(&s.message));
    }
}
pub fn open_setup(mtm: MainThreadMarker) {
    SETUP.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(AndroidSetup::create(mtm));
        }
        let setup = slot.as_ref().unwrap();
        unsafe {
            let _: () =
                msg_send![&*NSApplication::sharedApplication(mtm), activateIgnoringOtherApps: true];
        }
        setup.ivars().window.makeKeyAndOrderFront(None);
        setup.refresh(sel!(refresh:), None);
    });
}
#[cfg(debug_assertions)]
pub fn check_setup(mtm: MainThreadMarker) {
    let setup = AndroidSetup::create(mtm);
    let iv = setup.ivars();
    assert!(!iv.save.isEnabled());
    iv.state.lock().unwrap().devices = vec![AndroidDevice {
        serial: "fixture-phone".into(),
        name: "Test Android".into(),
        state: DeviceState::Unauthorized,
    }];
    setup.render();
    iv.popup.selectItemAtIndex(1);
    setup.render();
    assert!(!iv.save.isEnabled());
    iv.state.lock().unwrap().devices[0].state = DeviceState::Ready;
    setup.render();
    assert!(iv.save.isEnabled());
    iv.state.lock().unwrap().busy = true;
    setup.render();
    assert!(!iv.save.isEnabled() && !iv.refresh.isEnabled());
    iv.window.close();
}
