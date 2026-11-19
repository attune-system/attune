//! Payload supervisor, not a service. No platform configuration is loaded here.
use attune_common::child_process_environment::ChildProcessEnvironment;
use attune_workload_backend::runner_protocol::{
    decode_context, invalid, MAX_START_CONTEXT, START_MAGIC,
};
use std::{
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd},
    process::Stdio,
};
use tokio::{
    io::unix::AsyncFd,
    signal::unix::{signal, SignalKind},
};

struct Input {
    fd: AsyncFd<File>,
    flags: libc::c_int,
}
impl Input {
    fn new() -> io::Result<Self> {
        Self::duplicate(0)
    }
    fn duplicate(original: libc::c_int) -> io::Result<Self> {
        let raw = unsafe { libc::fcntl(original, libc::F_DUPFD_CLOEXEC, 3) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_fd(raw) };
        let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: File exclusively owns this duplicated descriptor and never
        // changes/closes it while registered; AsyncFd owns File until drop.
        match unsafe { AsyncFd::register(file) } {
            Ok(fd) => Ok(Self { fd, flags }),
            Err(error) => {
                unsafe {
                    libc::fcntl(original, libc::F_SETFL, flags);
                }
                Err(error.into())
            }
        }
    }
    fn finish(self) -> io::Result<()> {
        if unsafe { libc::fcntl(self.fd.get_ref().as_raw_fd(), libc::F_SETFL, self.flags) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    async fn exact(&self, bytes: &mut [u8]) -> io::Result<()> {
        let mut offset = 0;
        while offset < bytes.len() {
            let count = self.read(&mut bytes[offset..]).await?;
            if count == 0 {
                return Err(invalid());
            }
            offset += count;
        }
        Ok(())
    }
    async fn read(&self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut ready = self.fd.readable().await?;
            match ready.try_io(|fd| {
                let count = unsafe {
                    libc::read(
                        fd.get_ref().as_raw_fd(),
                        bytes.as_mut_ptr().cast(),
                        bytes.len(),
                    )
                };
                if count < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(count as usize)
                }
            }) {
                Ok(Ok(count)) => return Ok(count),
                Ok(Err(error)) => return Err(error),
                Err(_) => {}
            }
        }
    }
    async fn document(&self, maximum: usize) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let count = self.read(&mut chunk).await?;
            if count == 0 {
                return Ok(bytes);
            }
            if bytes
                .len()
                .checked_add(count)
                .is_none_or(|size| size > maximum)
            {
                return Err(invalid());
            }
            bytes.extend_from_slice(&chunk[..count]);
        }
    }
    async fn context(&self) -> io::Result<attune_common::workload_launch::LaunchInvocation> {
        let mut magic = vec![0; START_MAGIC.len()];
        self.exact(&mut magic).await?;
        if magic != START_MAGIC {
            return Err(invalid());
        }
        let mut length = [0; 4];
        self.exact(&mut length).await?;
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > MAX_START_CONTEXT {
            return Err(invalid());
        }
        let mut context = vec![0; length];
        self.exact(&mut context).await?;
        decode_context(&context)
    }
}
impl Drop for Input {
    fn drop(&mut self) {
        // dup shares the open-file-description flags with fd0 inherited by the
        // child. Restore blocking stdin after consuming exactly the header.
        unsafe {
            libc::fcntl(self.fd.get_ref().as_raw_fd(), libc::F_SETFL, self.flags);
        }
    }
}
pub fn run() -> io::Result<i32> {
    super::boundary_guard_basic()?;
    attune_common::parent_process_protection::protect_service_parent().map_err(|_| invalid())?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            // Install handlers before waiting at the gate, including as Pod PID1.
            let mut terminate = signal(SignalKind::terminate())?;
            let mut interrupt = signal(SignalKind::interrupt())?;
            let input = Input::new()?;
            let mut invocation = tokio::select! {
                result = input.context() => result?,
                _ = terminate.recv() => return Ok(143),
                _ = interrupt.recv() => return Ok(130),
            };
            let prepared_input = if super::parameter_input::required_at_gate(&invocation)? {
                let bytes = tokio::select! {
                    result=input.document(attune_common::workload_stdin::MAX_PRIVATE_STDIN_BYTES)=>result?,
                    _=terminate.recv()=>return Ok(143),
                    _=interrupt.recv()=>return Ok(130),
                };
                Some(super::parameter_input::prepare(&mut invocation.env, &bytes)?)
            } else {
                None
            };
            input.finish()?;
            let baseline = ChildProcessEnvironment::from_parent(std::env::vars_os(), &[])
                .map_err(|_| invalid())?;
            let mut command = tokio::process::Command::new(&invocation.argv[0]);
            baseline.apply(command.as_std_mut());
            command
                .args(&invocation.argv[1..])
                .envs(&invocation.env)
                .current_dir(attune_workload_backend::planning::working_directory(
                    invocation.working_root,
                ))
                .stdin(match &prepared_input {
                    Some(prepared) => Stdio::from(prepared.stdin.try_clone()?),
                    None => Stdio::inherit(),
                })
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.as_std_mut().process_group(0);
            }
            let mut child = command.spawn()?;
            let pid = child.id().ok_or_else(invalid)? as i32;
            let status = tokio::select! {
                result = child.wait() => result?,
                _ = terminate.recv() => stop_child(&mut child, pid).await?,
                _ = interrupt.recv() => stop_child(&mut child, pid).await?,
            };
            use std::os::unix::process::ExitStatusExt;
            Ok(status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)))
        })
}
async fn stop_child(
    child: &mut tokio::process::Child,
    pid: i32,
) -> io::Result<std::process::ExitStatus> {
    if pid <= 1 {
        return Err(invalid());
    }
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    if let Ok(status) = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await
    {
        return status;
    }
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    child.wait().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::workload_launch::{LaunchInvocation, LaunchRoot};
    use std::{collections::BTreeMap, io::Write, net::Shutdown, os::unix::net::UnixStream};

    #[tokio::test]
    async fn gate_consumes_exact_header_then_real_eof_and_restores_owned_descriptor() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let invocation = LaunchInvocation {
            argv: vec!["/bin/selected".into()],
            env: BTreeMap::from([
                ("ATTUNE_PARAMETER_DELIVERY".into(), "file".into()),
                ("ATTUNE_PARAMETER_FORMAT".into(), "yaml".into()),
            ]),
            working_root: LaunchRoot::Pack,
        };
        let frame = attune_workload_backend::runner_protocol::encode_start(&invocation).unwrap();
        let sender = std::thread::spawn(move || {
            writer.write_all(&frame).unwrap();
            writer.write_all(br#"{"value":"input"}"#).unwrap();
            writer.shutdown(Shutdown::Write).unwrap();
        });
        let input = Input::duplicate(reader.as_raw_fd()).unwrap();
        let context = input.context().await.unwrap();
        assert_eq!(context.argv, invocation.argv);
        assert_eq!(context.env, invocation.env);
        let bytes = input.document(1024).await.unwrap();
        input.finish().unwrap();
        sender.join().unwrap();
        assert_eq!(bytes, br#"{"value":"input"}"#);
        assert_eq!(
            unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_GETFL) } & libc::O_NONBLOCK,
            0,
        );
    }

    #[tokio::test]
    async fn gate_rejects_truncated_header_and_oversized_document() {
        for (bytes, header) in [(START_MAGIC[..2].to_vec(), true), (b"1234".to_vec(), false)] {
            let (reader, mut writer) = UnixStream::pair().unwrap();
            let sender = std::thread::spawn(move || {
                writer.write_all(&bytes).unwrap();
                writer.shutdown(Shutdown::Write).unwrap();
            });
            let input = Input::duplicate(reader.as_raw_fd()).unwrap();
            let denied = if header {
                input.context().await.is_err()
            } else {
                input.document(3).await.is_err()
            };
            input.finish().unwrap();
            sender.join().unwrap();
            assert!(denied);
        }
    }
}
