use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub const MAX_OUTPUT: usize = 1024 * 1024;
#[derive(Clone)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub(crate) input: Option<Vec<u8>>,
}
impl std::fmt::Debug for CommandSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandSpec")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("stdin", &self.input.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}
impl CommandSpec {
    pub fn new(
        program: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            env: Vec::new(),
            input: None,
        }
    }
    fn command(&self) -> Command {
        let mut c = Command::new(&self.program);
        c.args(&self.args).envs(self.env.iter().cloned());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x08000000); // CREATE_NO_WINDOW for console helpers
        }
        c
    }
}
pub struct Output {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
pub trait Runner: Send + Sync {
    fn run(&self, command: &CommandSpec, timeout: Duration) -> io::Result<Output>;
}
#[derive(Clone, Copy)]
pub struct ProcessRunner;
fn read_bounded(reader: impl Read + Send + 'static) -> mpsc::Receiver<io::Result<Vec<u8>>> {
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = reader
            .take((MAX_OUTPUT + 1) as u64)
            .read_to_end(&mut bytes)
            .and_then(|_| {
                if bytes.len() > MAX_OUTPUT {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Tool output exceeds limit",
                    ))
                } else {
                    Ok(bytes)
                }
            });
        let _ = tx.send(result);
    });
    rx
}
impl Runner for ProcessRunner {
    fn run(&self, spec: &CommandSpec, timeout: Duration) -> io::Result<Output> {
        let deadline = Instant::now() + timeout;
        let mut child = spec
            .command()
            .stdin(if spec.input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = read_bounded(child.stdout.take().unwrap());
        let stderr = read_bounded(child.stderr.take().unwrap());
        if let Some(input) = &spec.input {
            // Pairing input is six digits and one newline, far below pipe capacity.
            if let Err(e) = child.stdin.take().unwrap().write_all(input) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        }
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Android tool timed out",
                    ));
                }
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(e);
                }
            }
        };
        let receive = |rx: mpsc::Receiver<io::Result<Vec<u8>>>| -> io::Result<Vec<u8>> {
            rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::TimedOut, "Android tool output did not close")
                })?
        };
        Ok(Output {
            success: status.success(),
            stdout: receive(stdout)?,
            stderr: receive(stderr)?,
        })
    }
}

/// Owned process: every cancellation path terminates and reaps the viewer.
/// No global adb server is killed or debugging authorization revoked here.
pub struct MirrorProcess {
    child: Option<Child>,
    stderr: mpsc::Receiver<io::Result<Vec<u8>>>,
}
pub struct ExitNotice {
    pub success: bool,
    pub message: String,
}
impl MirrorProcess {
    pub fn spawn(spec: &CommandSpec) -> io::Result<Self> {
        let mut child = spec
            .command()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let stderr = read_bounded(child.stderr.take().unwrap());
        Ok(Self {
            child: Some(child),
            stderr,
        })
    }
    pub fn poll(&mut self) -> io::Result<Option<ExitNotice>> {
        let Some(child) = &mut self.child else {
            return Ok(Some(ExitNotice {
                success: true,
                message: "Android penceresi kapandı".into(),
            }));
        };
        let Some(status) = child.try_wait()? else {
            return Ok(None);
        };
        self.child.take();
        let diagnostic = self
            .stderr
            .recv_timeout(Duration::from_millis(100))
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        let message = if status.success() {
            "Android penceresi kapandı".into()
        } else {
            format!(
                "Android görüntüsü açılamadı ({status}): {}",
                crate::safe_text(&String::from_utf8_lossy(&diagnostic), 500)
            )
        };
        Ok(Some(ExitNotice {
            success: status.success(),
            message,
        }))
    }
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
impl Drop for MirrorProcess {
    fn drop(&mut self) {
        self.stop();
    }
}
