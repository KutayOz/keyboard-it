//! Android authority stays with ADB. Discovery is read-only; saving and opening a
//! specific, approved target are explicit actions. No shell-built local commands.
pub mod process;
pub mod session;
use process::{CommandSpec, Output, ProcessRunner, Runner};
use protocol::{
    devices::{
        valid_adb_serial, valid_android_id, DeviceTransport, PairedDevice, Permissions, Platform,
    },
    stream::StreamSettings,
};
use std::{
    collections::HashSet,
    ffi::OsString,
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

const TOOL_TIMEOUT: Duration = Duration::from_secs(8);
pub fn safe_text(text: &str, limit: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect()
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn checked(output: Output) -> io::Result<String> {
    if !output.success {
        return Err(io::Error::other(safe_text(
            &format!(
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
            600,
        )));
    }
    String::from_utf8(output.stdout).map_err(|_| invalid("Android tool returned invalid UTF-8"))
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceState {
    Ready,
    Unauthorized,
    Offline,
    Unavailable(String),
}
impl DeviceState {
    pub fn label(&self) -> &str {
        match self {
            Self::Ready => "hazır",
            Self::Unauthorized => "telefondan izin bekleniyor",
            Self::Offline => "çevrimdışı",
            Self::Unavailable(_) => "kullanılamıyor",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AndroidDevice {
    pub serial: String,
    pub name: String,
    pub state: DeviceState,
}
pub fn parse_devices(output: &str) -> io::Result<Vec<AndroidDevice>> {
    let mut devices = Vec::new();
    let mut seen = HashSet::new();
    let mut header = false;
    for line in output.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if line == "List of devices attached" {
            header = true;
            continue;
        }
        if line.starts_with('*') {
            continue;
        } // adb server startup banner
        if !header {
            return Err(invalid("Unexpected adb devices response"));
        }
        let mut fields = line.split_whitespace();
        let serial = fields.next().unwrap();
        if !valid_adb_serial(serial) || !seen.insert(serial) || devices.len() >= 128 {
            return Err(invalid("Invalid or duplicate ADB serial"));
        }
        let state = match fields.next() {
            Some("device") => DeviceState::Ready,
            Some("unauthorized") => DeviceState::Unauthorized,
            Some("offline") => DeviceState::Offline,
            Some(other) => DeviceState::Unavailable(safe_text(other, 64)),
            None => return Err(invalid("Missing ADB device state")),
        };
        let model = fields
            .find_map(|f| f.strip_prefix("model:"))
            .unwrap_or(serial)
            .replace('_', " ");
        devices.push(AndroidDevice {
            serial: serial.into(),
            name: safe_text(&model, 100),
            state,
        });
    }
    if !header {
        return Err(invalid("Missing adb devices header"));
    }
    Ok(devices)
}

#[derive(Clone, Debug)]
pub struct Tools {
    pub adb: PathBuf,
    pub scrcpy: Option<PathBuf>,
}
impl Tools {
    pub fn locate() -> io::Result<Self> {
        Ok(Self {
            adb: locate_tool("KEYBOARD_IT_ADB", "adb")?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "ADB bulunamadı. Android SDK Platform Tools kurun.",
                )
            })?,
            scrcpy: locate_tool("KEYBOARD_IT_SCRCPY", "scrcpy")?,
        })
    }
}
fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return path
            .metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0);
    }
    #[cfg(not(unix))]
    {
        true
    }
}
fn locate_tool(variable: &str, name: &str) -> io::Result<Option<PathBuf>> {
    if let Some(path) = std::env::var_os(variable) {
        let path = PathBuf::from(path);
        if !path.is_absolute() || !executable(&path) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{variable} geçerli bir mutlak çalıştırılabilir dosya yolu olmalı"),
            ));
        }
        return Ok(Some(path));
    }
    #[cfg(windows)]
    let name = format!("{name}.exe");
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|v| {
            std::env::split_paths(&v)
                .filter(|p| p.is_absolute())
                .collect()
        })
        .unwrap_or_default();
    dirs.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    if let Some(sdk) =
        std::env::var_os("ANDROID_HOME").or_else(|| std::env::var_os("ANDROID_SDK_ROOT"))
    {
        dirs.push(PathBuf::from(sdk).join("platform-tools"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Library/Android/sdk/platform-tools"));
    }
    Ok(dirs
        .into_iter()
        .map(|p| p.join(&name))
        .find(|p| p.is_absolute() && executable(p)))
}
#[derive(Clone)]
pub struct Adapter<R = ProcessRunner> {
    pub tools: Tools,
    runner: R,
}
impl Adapter<ProcessRunner> {
    pub fn local() -> io::Result<Self> {
        Ok(Self::new(Tools::locate()?, ProcessRunner))
    }
}
impl<R: Runner> Adapter<R> {
    pub fn new(tools: Tools, runner: R) -> Self {
        Self { tools, runner }
    }
    fn adb(&self, args: impl IntoIterator<Item = impl Into<OsString>>) -> CommandSpec {
        CommandSpec::new(&self.tools.adb, args)
    }
    pub fn discover(&self) -> io::Result<Vec<AndroidDevice>> {
        parse_devices(&checked(
            self.runner
                .run(&self.adb(["devices", "-l"]), TOOL_TIMEOUT)?,
        )?)
    }
    pub fn identity(&self, serial: &str) -> io::Result<String> {
        if !valid_adb_serial(serial) {
            return Err(invalid("Invalid ADB serial"));
        }
        let result = checked(self.runner.run(
            &self.adb([
                "-s",
                serial,
                "shell",
                "settings",
                "get",
                "secure",
                "android_id",
            ]),
            TOOL_TIMEOUT,
        )?)?;
        let id = result.trim().to_ascii_lowercase();
        if !valid_android_id(&id) {
            return Err(invalid("Android kimliği okunamadı. Telefonun kilidini açıp USB hata ayıklama iznini kontrol edin."));
        }
        Ok(id)
    }
    pub fn saveable(&self, device: &AndroidDevice) -> io::Result<PairedDevice> {
        if device.state != DeviceState::Ready {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Telefonda bu bilgisayara izin verin, ardından listeyi yenileyin.",
            ));
        }
        // Recheck before saving; the popup may have become stale while the user read it.
        let ready = self
            .discover()?
            .into_iter()
            .any(|d| d.serial == device.serial && d.state == DeviceState::Ready);
        if !ready {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "Android cihazı artık hazır değil; listeyi yenileyin.",
            ));
        }
        Ok(PairedDevice {
            transport: DeviceTransport::Adb {
                serial: device.serial.clone(),
                android_id: self.identity(&device.serial)?,
            },
            id: String::new(),
            name: device.name.clone(),
            platform: Platform::Android,
            permissions: Permissions {
                screen: true,
                input: true,
                clipboard: false,
            },
            capabilities: None,
            peer_device_id: String::new(),
            shared_secret: String::new(),
            host: String::new(),
            ip: String::new(),
            port: protocol::DEFAULT_PORT,
            desktop_port: protocol::desktop::DEFAULT_PORT,
        })
    }
    pub fn pair_wifi(&self, endpoint: &str, code: &str) -> io::Result<()> {
        let endpoint = endpoint_address(endpoint)?;
        if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid("Eşleştirme kodu 6 rakam olmalı"));
        }
        let mut command = self.adb(["pair", &endpoint]);
        command.input = Some(format!("{code}\n").into_bytes());
        let result = self
            .runner
            .run(&command, Duration::from_secs(30))
            .and_then(checked)
            .map_err(|e| io::Error::new(e.kind(), e.to_string().replace(code, "[redacted]")))?;
        if !result
            .lines()
            .map(|l| l.strip_prefix("Enter pairing code: ").unwrap_or(l))
            .any(|l| l.starts_with("Successfully paired to "))
        {
            return Err(io::Error::other("Android Wi-Fi eşleştirmesi tamamlanmadı; güncel kodu ve eşleştirme adresini kontrol edin."));
        }
        Ok(())
    }
    pub fn connect_wifi(&self, endpoint: &str) -> io::Result<()> {
        let endpoint = endpoint_address(endpoint)?;
        let result = checked(
            self.runner
                .run(&self.adb(["connect", &endpoint]), TOOL_TIMEOUT)?,
        )?;
        if !result.lines().any(|line| {
            line == format!("connected to {endpoint}")
                || line == format!("already connected to {endpoint}")
        }) {
            return Err(io::Error::other(format!(
                "Wi-Fi bağlantısı kurulamadı: {}",
                safe_text(&result, 300)
            )));
        }
        Ok(())
    }
    pub fn mirror_command(
        &self,
        device: &PairedDevice,
        stream: StreamSettings,
    ) -> io::Result<CommandSpec> {
        let command = mirror_command(&self.tools, device, stream)?;
        let help = checked(self.runner.run(
            &CommandSpec::new(&command.program, ["--help"]),
            TOOL_TIMEOUT,
        )?)?;
        for flag in [
            "--serial",
            "--no-control",
            "--no-audio",
            "--no-clipboard-autosync",
            "--video-codec",
            "--window-title",
            "--max-fps",
            "--max-size",
            "--video-bit-rate",
        ] {
            if !help.contains(flag) {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("scrcpy gerekli {flag} seçeneğini desteklemiyor; güncel scrcpy kurun."),
                ));
            }
        }
        let DeviceTransport::Adb { serial, android_id } = &device.transport else {
            unreachable!()
        };
        if !self
            .discover()?
            .iter()
            .any(|d| d.serial == *serial && d.state == DeviceState::Ready)
        {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "Android bağlı değil veya telefondan izin bekliyor. Listeyi yenileyin.",
            ));
        }
        if !self.identity(serial)?.eq_ignore_ascii_case(android_id) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Bu adreste farklı bir Android kimliği var. Cihazı yeniden ekleyin.",
            ));
        }
        Ok(command)
    }
}
pub fn endpoint_address(value: &str) -> io::Result<String> {
    let address: SocketAddr = value
        .trim()
        .parse()
        .map_err(|_| invalid("Adres IP:port biçiminde olmalı (IPv6 için [IP]:port)"))?;
    if address.port() == 0 || address.ip().is_unspecified() || address.ip().is_multicast() {
        return Err(invalid("Geçersiz Android bağlantı adresi"));
    }
    Ok(address.to_string())
}
pub fn mirror_command(
    tools: &Tools,
    device: &PairedDevice,
    stream: StreamSettings,
) -> io::Result<CommandSpec> {
    if !device.valid_connection() {
        return Err(invalid("Invalid saved Android target"));
    }
    let DeviceTransport::Adb { serial, .. } = &device.transport else {
        return Err(invalid("Not an Android target"));
    };
    if !device.permissions.screen {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Bu cihaz için Ekran izni kapalı.",
        ));
    }
    let scrcpy = tools.scrcpy.as_ref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "scrcpy bulunamadı; Android görüntüsü için scrcpy kurun.",
        )
    })?;
    let stream = stream.normalized();
    let max_size = match stream.max_width.max(stream.max_height) {
        0 => 4096,
        size => size.min(4096),
    };
    let mut command = CommandSpec::new(
        scrcpy,
        [
            format!("--serial={serial}"),
            format!(
                "--window-title=keyboard-it · {}",
                safe_text(&device.name, 100)
            ),
            "--no-audio".into(),
            "--no-clipboard-autosync".into(),
            "--video-codec=h264".into(),
            format!("--max-fps={}", stream.fps),
            format!("--max-size={max_size}"),
        ],
    );
    if !device.permissions.input {
        command.args.push("--no-control".into());
    }
    if stream.bitrate_kbps != 0 {
        command
            .args
            .push(format!("--video-bit-rate={}", u64::from(stream.bitrate_kbps) * 1000).into());
    }
    command
        .env
        .push(("ADB".into(), tools.adb.as_os_str().into()));
    Ok(command)
}

#[cfg(test)]
mod tests;
