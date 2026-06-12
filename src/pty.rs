use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use anyhow::Result;
use parking_lot::Mutex;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::event::AppEvent;

pub type SessionId = u64;

pub struct PtySession {
    pub title: String,
    pub parser: Arc<Mutex<vt100::Parser>>,
    /// Set by the reader thread when new output arrived; cleared before each
    /// render of the active tab. Gates PtyOutput events (coalescing).
    pub dirty: Arc<AtomicBool>,
    /// Exit code once the child has exited.
    pub exited: Option<u32>,
    /// Close the pane automatically on exit (used for script-runner panes).
    pub auto_close: bool,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    size: (u16, u16), // rows, cols
}

impl PtySession {
    pub fn spawn(
        id: SessionId,
        mut cmd: CommandBuilder,
        (rows, cols): (u16, u16),
        scrollback: usize,
        title: String,
        auto_close: bool,
        tx: Sender<AppEvent>,
    ) -> Result<Self> {
        // vt100's grid math (line wrap + scroll) misbehaves below 2x4
        let rows = rows.max(2);
        let cols = cols.max(4);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        let pty = native_pty_system();
        let pair = pty.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, scrollback)));
        let dirty = Arc::new(AtomicBool::new(false));

        {
            let parser = Arc::clone(&parser);
            let dirty = Arc::clone(&dirty);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => {
                            let _ = tx.send(AppEvent::PtyExited(id));
                            break;
                        }
                        Ok(n) => {
                            // a vt100 parsing panic must not kill the reader,
                            // or the session would silently stop updating
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                                || parser.lock().process(&buf[..n]),
                            ));
                            if !dirty.swap(true, Ordering::AcqRel) {
                                let _ = tx.send(AppEvent::PtyOutput);
                            }
                        }
                    }
                }
            });
        }

        Ok(Self {
            title,
            parser,
            dirty,
            exited: None,
            auto_close,
            writer,
            master: pair.master,
            child,
            size: (rows, cols),
        })
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(2);
        let cols = cols.max(4);
        if self.size == (rows, cols) {
            return;
        }
        self.size = (rows, cols);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.parser.lock().screen_mut().set_size(rows, cols);
    }

    pub fn mark_exited(&mut self) {
        let code = self
            .child
            .try_wait()
            .ok()
            .flatten()
            .map(|s| s.exit_code())
            .unwrap_or(0);
        self.exited = Some(code);
    }

    /// Working directory of the child process (Linux only; None elsewhere).
    pub fn cwd(&self) -> Option<PathBuf> {
        #[cfg(target_os = "linux")]
        {
            let pid = self.child.process_id()?;
            std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if self.exited.is_none() {
            let _ = self.child.kill();
        }
    }
}
