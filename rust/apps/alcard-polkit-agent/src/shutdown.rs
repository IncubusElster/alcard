//! Graceful termination without a blocked mask inherited by the PAM helper.
use nix::sys::signal::{pthread_sigmask, SigSet, SigmaskHow, Signal};
use signal_hook::iterator::{Handle, Signals};
use smithay_client_toolkit::reexports::calloop::LoopSignal;
use std::{
    error::Error,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};

pub struct Watcher {
    requested: Arc<AtomicBool>,
    handle: Handle,
    thread: Option<thread::JoinHandle<()>>,
    previous: SigSet,
}
impl Watcher {
    pub fn new(wakeup: LoopSignal) -> Result<Self, Box<dyn Error>> {
        let mut signals =
            Signals::new([signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT])?;
        let handle = signals.handle();
        // Install safe handlers before unblocking. Workers and native children
        // now inherit a mask in which ordinary termination signals work.
        let mut managed = SigSet::empty();
        managed.add(Signal::SIGTERM);
        managed.add(Signal::SIGINT);
        let mut previous = SigSet::empty();
        pthread_sigmask(SigmaskHow::SIG_UNBLOCK, Some(&managed), Some(&mut previous))?;
        let requested = Arc::new(AtomicBool::new(false));
        let flag = requested.clone();
        let worker = thread::Builder::new()
            .name("alcard-auth-signals".into())
            .spawn(move || {
                if signals.forever().next().is_some() {
                    flag.store(true, Ordering::Release);
                    wakeup.wakeup();
                }
            })?;
        Ok(Self {
            requested,
            handle,
            thread: Some(worker),
            previous,
        })
    }
    pub fn requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
}
impl Drop for Watcher {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&self.previous), None);
    }
}
#[cfg(test)]
#[path = "unit_tests/shutdown.rs"]
mod tests;
