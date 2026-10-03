use anyhow::{Context, Result};
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use std::io::{self, IsTerminal};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum WatchControl {
    Cancel,
    Detach,
}

pub(super) struct WatchControls {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(windows)]
    interrupt: tokio::signal::windows::CtrlC,
    events: Option<EventStream>,
    terminal: Option<TerminalMode>,
}

impl WatchControls {
    pub(super) fn new() -> Result<Self> {
        #[cfg(unix)]
        let interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        #[cfg(windows)]
        let interrupt = tokio::signal::windows::ctrl_c()?;
        let interactive = io::stdin().is_terminal();
        #[cfg(unix)]
        // SAFETY: these calls inspect process and terminal IDs without changing state.
        let interactive =
            interactive && unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) == libc::getpgrp() };
        let terminal = if interactive {
            Some(TerminalMode::new()?)
        } else {
            None
        };
        let events = terminal.as_ref().map(|_| EventStream::new());
        if terminal.is_some() {
            eprintln!("Ctrl+C requests cancellation; Ctrl+D detaches.");
        }
        Ok(Self {
            interrupt,
            events,
            terminal,
        })
    }

    pub(super) async fn next(&mut self) -> Result<WatchControl> {
        loop {
            tokio::select! {
                biased;
                _ = self.interrupt.recv() => return Ok(WatchControl::Cancel),
                event = async {
                    match self.events.as_mut() {
                        Some(events) => events.next().await,
                        None => std::future::pending().await,
                    }
                } => {
                    let event = event.context("Terminal input stream closed")??;
                    if let Event::Key(key) = event {
                        if let Some(control) = key_control(key) {
                            return Ok(control);
                        }
                    }
                }
            }
        }
    }

    pub(super) fn close(&mut self) -> Result<()> {
        self.events.take();
        if let Some(terminal) = self.terminal.as_mut() {
            terminal.close()?;
        }
        self.terminal.take();
        Ok(())
    }
}

fn key_control(key: KeyEvent) -> Option<WatchControl> {
    if key.kind != KeyEventKind::Press || !key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    match key.code {
        KeyCode::Char('c' | 'C') => Some(WatchControl::Cancel),
        KeyCode::Char('d' | 'D') => Some(WatchControl::Detach),
        _ => None,
    }
}

struct TerminalMode {
    active: bool,
}

impl TerminalMode {
    fn new() -> Result<Self> {
        #[cfg(unix)]
        let original = terminal_attributes()?;
        enable_raw_mode().context("Failed to enable watch keyboard controls")?;
        let guard = Self { active: true };
        #[cfg(unix)]
        {
            let mut attributes = terminal_attributes()?;
            // Keep newline translation and terminal-generated SIGINT while reading keys immediately.
            attributes.c_oflag = original.c_oflag;
            attributes.c_lflag |= original.c_lflag & libc::ISIG;
            // SAFETY: stdin is a terminal and attributes is an initialized termios value.
            if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &attributes) } != 0 {
                return Err(io::Error::last_os_error())
                    .context("Failed to configure watch terminal");
            }
        }
        Ok(guard)
    }

    fn close(&mut self) -> Result<()> {
        if self.active {
            disable_raw_mode().context("Failed to restore terminal mode")?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for TerminalMode {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(unix)]
fn terminal_attributes() -> Result<libc::termios> {
    let mut attributes = std::mem::MaybeUninit::uninit();
    // SAFETY: tcgetattr initializes the value on success; it is only read after checking the result.
    if unsafe { libc::tcgetattr(libc::STDIN_FILENO, attributes.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error()).context("Failed to read terminal mode");
    }
    // SAFETY: tcgetattr succeeded and initialized attributes.
    Ok(unsafe { attributes.assume_init() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_control_key_presses_cancel_or_detach() {
        assert_eq!(
            key_control(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(WatchControl::Cancel)
        );
        assert_eq!(
            key_control(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            Some(WatchControl::Detach)
        );
        assert_eq!(
            key_control(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            key_control(KeyEvent::new_with_kind(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
                KeyEventKind::Release
            )),
            None
        );
    }
}
