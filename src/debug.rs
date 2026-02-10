use std::time::Instant;

pub struct DebugTimer {
    module: &'static str,
    name: &'static str,
    start: Instant,
}

impl DebugTimer {
    pub fn new(module: &'static str, name: &'static str) -> Self {
        Self { module, name, start: Instant::now() }
    }
}

impl Drop for DebugTimer {
    fn drop(&mut self) {
        let elapsed_ms = self.start.elapsed().as_millis();
        let ignore_below = crate::config::debug_ignore_ms() as u128;
        if ignore_below != 0 && elapsed_ms < ignore_below {
            return;
        }
        eprintln!("[debug] module={} fn={} elapsed_ms={}", self.module, self.name, elapsed_ms);
    }
}

#[macro_export]
macro_rules! debug_timer_log {
    ($name:expr) => {
        let _debug_timer = if crate::config::debug_enabled() {
            Some(crate::debug::DebugTimer::new(std::module_path!(), $name))
        } else {
            None
        };
    };
}

pub fn start_if(enabled: bool) -> Option<Instant> {
    if enabled { Some(Instant::now()) } else { None }
}

pub fn log_elapsed(module: &str, section: &str, start: Option<Instant>) {
    if let Some(start) = start {
        let elapsed_ms = start.elapsed().as_millis();
        let ignore_below = crate::config::debug_ignore_ms() as u128;
        if ignore_below != 0 && elapsed_ms < ignore_below {
            return;
        }
        eprintln!("[debug] module={} section={} elapsed_ms={}", module, section, elapsed_ms);
    }
}
