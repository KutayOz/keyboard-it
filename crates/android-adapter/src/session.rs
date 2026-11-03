//! Only a current launch ticket may publish a viewer. Cancellation also rejects
//! slow probes that finish after A -> B -> A selection changes.
use crate::process::{CommandSpec, MirrorProcess};
use std::{io, sync::Mutex};
#[derive(Default)]
struct State {
    generation: u64,
    process: Option<MirrorProcess>,
    status: String,
    starting: bool,
    failed: bool,
}
#[derive(Default)]
pub struct Sessions(Mutex<State>);
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub generation: u64,
    pub starting: bool,
    pub running: bool,
    pub failed: bool,
    pub message: String,
}
impl Sessions {
    pub fn begin(&self) -> u64 {
        let mut s = self.0.lock().unwrap();
        s.generation = s
            .generation
            .checked_add(1)
            .expect("Android session generation exhausted");
        s.process.take();
        s.starting = true;
        s.failed = false;
        s.status = "Android bağlantısı hazırlanıyor…".into();
        s.generation
    }
    pub fn cancel(&self) {
        let mut s = self.0.lock().unwrap();
        Self::cancel_locked(&mut s);
    }
    pub fn cancel_if_current(&self, generation: u64) -> bool {
        let mut s = self.0.lock().unwrap();
        if s.generation != generation {
            return false;
        }
        Self::cancel_locked(&mut s);
        true
    }
    fn cancel_locked(s: &mut State) {
        s.generation = s
            .generation
            .checked_add(1)
            .expect("Android session generation exhausted");
        s.process.take();
        s.starting = false;
        s.failed = false;
        s.status = "Android penceresi kapalı".into();
    }
    pub fn fail(&self, generation: u64, error: &str) {
        let mut s = self.0.lock().unwrap();
        if s.generation == generation {
            s.process.take();
            s.starting = false;
            s.failed = true;
            s.status = error.into();
        }
    }
    pub fn launch(&self, generation: u64, command: &CommandSpec) -> io::Result<bool> {
        let mut s = self.0.lock().unwrap();
        if s.generation != generation {
            return Ok(false);
        }
        let process = MirrorProcess::spawn(command)?;
        s.process = Some(process);
        s.starting = false;
        s.failed = false;
        s.status = "Android penceresi açılıyor…".into();
        Ok(true)
    }
    pub fn status(&self) -> Status {
        let mut s = self.0.lock().unwrap();
        if let Some(process) = &mut s.process {
            match process.poll() {
                Ok(Some(exit)) => {
                    s.process.take();
                    s.failed = !exit.success;
                    s.status = exit.message;
                }
                Err(e) => {
                    s.process.take();
                    s.failed = true;
                    s.status = format!("Android oturumu sona erdi: {e}");
                }
                Ok(None) => {}
            }
        }
        Status {
            generation: s.generation,
            starting: s.starting,
            running: s.process.is_some(),
            failed: s.failed,
            message: s.status.clone(),
        }
    }
}
