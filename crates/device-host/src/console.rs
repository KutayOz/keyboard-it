//! A terminal host grants access only after a local, matching-code approval.
use protocol::pairing::PairRequest;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Mutex,
    },
    time::{Duration, Instant},
};

struct Pending {
    id: u64,
    code: String,
    deadline: Instant,
    reply: mpsc::Sender<bool>,
}
#[derive(Default)]
pub struct ConsoleApproval {
    next_id: AtomicU64,
    pending: Mutex<Option<Pending>>,
}
impl ConsoleApproval {
    fn begin(&self, code: &str, timeout: Duration) -> Option<(u64, mpsc::Receiver<bool>)> {
        let mut pending = self.pending.lock().unwrap();
        if pending
            .as_ref()
            .is_some_and(|p| p.deadline > Instant::now())
        {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        *pending = Some(Pending {
            id,
            code: code.into(),
            deadline: Instant::now() + timeout,
            reply: tx,
        });
        Some((id, rx))
    }
    pub fn decide(&self, request: &PairRequest) -> bool {
        let timeout = protocol::PAIR_DECISION_TIMEOUT;
        let Some((id, rx)) = self.begin(&request.code, timeout) else {
            return false;
        };
        println!("Pairing request from {:?}; compare code {} on both devices.\nTo approve locally: allow {}\nTo refuse: deny (expires in {}s)",
            request.peer_name, protocol::pairing::code_display(&request.code), request.code, timeout.as_secs());
        let approved = rx.recv_timeout(timeout).unwrap_or(false);
        // Do not remove a newer request if this one's waiter completed late.
        let mut pending = self.pending.lock().unwrap();
        if pending.as_ref().is_some_and(|p| p.id == id) {
            pending.take();
        }
        approved
    }
    pub fn answer(&self, code: Option<&str>) -> bool {
        let mut pending = self.pending.lock().unwrap();
        let Some(p) = pending.as_ref() else {
            return false;
        };
        if p.deadline <= Instant::now() {
            pending.take();
            return false;
        }
        if code.is_some_and(|c| c != p.code) {
            return false;
        }
        pending.take().unwrap().reply.send(code.is_some()).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matching_local_code_is_required_and_approval_cannot_be_replayed() {
        let approval = ConsoleApproval::default();
        let (_, rx) = approval.begin("123456", Duration::from_secs(2)).unwrap();
        assert!(!approval.answer(Some("999999")));
        assert!(rx.try_recv().is_err());
        assert!(approval.answer(Some("123456")));
        assert!(rx.recv().unwrap());
        assert!(!approval.answer(Some("123456")));
    }
    #[test]
    fn expiry_and_denial_cannot_grant_access() {
        let approval = ConsoleApproval::default();
        let (_, rx) = approval.begin("123456", Duration::ZERO).unwrap();
        assert!(!approval.answer(Some("123456")));
        assert!(rx.recv().is_err());
        let (_, rx) = approval.begin("123456", Duration::from_secs(2)).unwrap();
        assert!(approval.begin("999999", Duration::from_secs(2)).is_none());
        assert!(approval.answer(None));
        assert!(!rx.recv().unwrap());
    }
}
