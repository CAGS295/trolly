use std::sync::atomic::Ordering;
use std::sync::{atomic::AtomicBool, Arc};

#[derive(Clone)]
pub struct Terminate(Arc<AtomicBool>);

impl Default for Terminate {
    fn default() -> Self {
        Self::new()
    }
}

impl Terminate {
    pub fn new() -> Terminate {
        let flag = Terminate(Arc::new(AtomicBool::new(false)));
        Self::set_ctrl_c(&flag);
        flag
    }

    fn set_ctrl_c(termination_flag: &Self) {
        let termination_flag = termination_flag.clone();
        ctrlc::set_handler(move || {
            termination_flag
                .0
                .store(true, std::sync::atomic::Ordering::Relaxed);
            println!("\r received Ctrl+C! terminating");
        })
        .expect("/r Error setting Ctrl-C handler");
    }

    pub fn is_terminated(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
