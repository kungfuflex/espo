#[cfg(feature = "jemalloc-prof")]
mod imp {
    use crate::config::JemallocProfileConfig;
    use anyhow::{Context, Result};
    use std::ffi::CString;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tikv_jemalloc_ctl::raw;

    pub struct JemallocProfileGuard {
        enabled: bool,
        dump_dir: PathBuf,
        dump_on_shutdown: bool,
        shutdown_dumped: bool,
        stop_tx: Option<Sender<()>>,
        handle: Option<JoinHandle<()>>,
    }

    impl JemallocProfileGuard {
        fn disabled() -> Self {
            Self {
                enabled: false,
                dump_dir: PathBuf::new(),
                dump_on_shutdown: false,
                shutdown_dumped: true,
                stop_tx: None,
                handle: None,
            }
        }

        pub fn shutdown_dump(&mut self) {
            self.stop_periodic_thread();
            if !self.enabled || !self.dump_on_shutdown || self.shutdown_dumped {
                return;
            }
            if let Err(e) = dump_profile(&self.dump_dir, "shutdown") {
                eprintln!("[jemalloc] shutdown profile dump failed: {e:?}");
            }
            self.shutdown_dumped = true;
        }

        fn stop_periodic_thread(&mut self) {
            if let Some(stop_tx) = self.stop_tx.take() {
                let _ = stop_tx.send(());
            }
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    impl Drop for JemallocProfileGuard {
        fn drop(&mut self) {
            self.shutdown_dump();
        }
    }

    pub fn start(config: &JemallocProfileConfig) -> JemallocProfileGuard {
        if !config.enabled {
            return JemallocProfileGuard::disabled();
        }

        match start_inner(config) {
            Ok(guard) => guard,
            Err(e) => {
                eprintln!("[jemalloc] profiling disabled: {e:?}");
                JemallocProfileGuard::disabled()
            }
        }
    }

    fn start_inner(config: &JemallocProfileConfig) -> Result<JemallocProfileGuard> {
        let dump_dir = PathBuf::from(&config.dump_dir);
        std::fs::create_dir_all(&dump_dir)
            .with_context(|| format!("failed to create {}", dump_dir.display()))?;

        ensure_prof_enabled()?;
        set_prof_active(true)?;
        eprintln!(
            "[jemalloc] profiling active; dump_dir={} interval_secs={} dump_on_shutdown={}",
            dump_dir.display(),
            config.interval_secs,
            config.dump_on_shutdown
        );

        if let Err(e) = dump_profile(&dump_dir, "startup") {
            eprintln!("[jemalloc] startup profile dump failed: {e:?}");
        }

        let (stop_tx, stop_rx) = mpsc::channel();
        let handle = if config.interval_secs > 0 {
            let interval = Duration::from_secs(config.interval_secs);
            let thread_dump_dir = dump_dir.clone();
            Some(thread::spawn(move || {
                let mut seq = 0u64;
                loop {
                    match stop_rx.recv_timeout(interval) {
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {
                            seq = seq.saturating_add(1);
                            let reason = format!("periodic-{seq}");
                            if let Err(e) = dump_profile(&thread_dump_dir, &reason) {
                                eprintln!("[jemalloc] periodic profile dump failed: {e:?}");
                            }
                        }
                    }
                }
            }))
        } else {
            None
        };

        Ok(JemallocProfileGuard {
            enabled: true,
            dump_dir,
            dump_on_shutdown: config.dump_on_shutdown,
            shutdown_dumped: false,
            stop_tx: Some(stop_tx),
            handle,
        })
    }

    fn ensure_prof_enabled() -> Result<()> {
        let prof_enabled: bool = unsafe { raw::read(b"opt.prof\0") }
            .map_err(|e| anyhow::anyhow!("failed to read jemalloc opt.prof: {e:?}"))?;
        if !prof_enabled {
            anyhow::bail!(
                "jemalloc profiling is not enabled; run with MALLOC_CONF=prof:true,prof_active:false,lg_prof_sample:20"
            );
        }
        Ok(())
    }

    fn set_prof_active(active: bool) -> Result<()> {
        unsafe { raw::write(b"prof.active\0", active) }
            .map_err(|e| anyhow::anyhow!("failed to update jemalloc prof.active: {e:?}"))
    }

    fn dump_profile(dump_dir: &Path, reason: &str) -> Result<()> {
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let pid = std::process::id();
        let filename = format!("espo.{pid}.{ts}.{reason}.heap");
        let path = dump_dir.join(filename);
        let path_string = path.to_string_lossy();
        let c_path = CString::new(path_string.as_bytes()).context("profile path contains NUL")?;

        unsafe {
            raw::write::<*const std::os::raw::c_char>(b"prof.dump\0", c_path.as_ptr()).map_err(
                |e| anyhow::anyhow!("failed to dump jemalloc profile to {}: {e:?}", path.display()),
            )?;
        }
        eprintln!("[jemalloc] dumped profile {}", path.display());
        Ok(())
    }
}

#[cfg(not(feature = "jemalloc-prof"))]
mod imp {
    use crate::config::JemallocProfileConfig;

    pub struct JemallocProfileGuard;

    impl JemallocProfileGuard {
        pub fn shutdown_dump(&mut self) {}
    }

    pub fn start(config: &JemallocProfileConfig) -> JemallocProfileGuard {
        if config.enabled {
            eprintln!(
                "[jemalloc] profiling requested, but this binary was not built with --features jemalloc-prof"
            );
        }
        JemallocProfileGuard
    }
}

pub use imp::{JemallocProfileGuard, start};
