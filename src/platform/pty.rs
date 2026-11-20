//! An installer owns a terminal and a process group, not a Rex session.
#[cfg(unix)]
mod unix {
    use std::{
        fs::File,
        io::{self, Read, Write},
        os::fd::{AsRawFd, FromRawFd},
        os::unix::process::CommandExt,
        process::{Child, Command, ExitStatus, Stdio},
    };

    pub struct Pty {
        master: File,
        child: Child,
        reaped: bool,
    }

    impl Pty {
        pub fn spawn(command: &mut Command) -> io::Result<Self> {
            let mut master = -1;
            let mut slave = -1;
            let mut size = libc::winsize {
                ws_row: 40,
                ws_col: 100,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            // On success both descriptors are ours. The child takes a duplicate of the
            // slave as fd 0; neither original descriptor crosses exec.
            if unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &raw mut size,
                )
            } == -1
            {
                return Err(io::Error::last_os_error());
            }
            let master = unsafe { File::from_raw_fd(master) };
            let slave = unsafe { File::from_raw_fd(slave) };
            let fd = master.as_raw_fd();
            if unsafe {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) == -1
                    || libc::fcntl(slave.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) == -1
                    || libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) == -1
            } {
                return Err(io::Error::last_os_error());
            }
            command
                .stdin(Stdio::from(slave.try_clone()?))
                .stdout(Stdio::from(slave.try_clone()?))
                .stderr(Stdio::from(slave));
            // Only async-signal-safe calls between fork and exec.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let child = command.spawn()?;
            Ok(Self {
                master,
                child,
                reaped: false,
            })
        }

        pub fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            match self.master.read(bytes) {
                // Linux reports EIO when the last slave closes.
                Err(e) if e.raw_os_error() == Some(libc::EIO) => Ok(0),
                result => result,
            }
        }

        pub fn answer(&mut self, line: &[u8]) -> io::Result<()> {
            self.master.write_all(line)
        }

        pub fn try_wait(
            &mut self,
            stopping: bool,
            terminal_closed: bool,
        ) -> io::Result<Option<ExitStatus>> {
            let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
            // Keep the PID reserved until the whole group has been signalled,
            // even when the group leader exits on its own.
            if unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id(),
                    info.as_mut_ptr(),
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            } == -1
            {
                return Err(io::Error::last_os_error());
            }
            if unsafe { info.assume_init().si_pid() } == 0 {
                return Ok(None);
            }
            // During Stop, the leader may exit before other processes sharing
            // the terminal. Keep the group owned until they close it or the
            // caller's deadline expires. A closed terminal lets us finalize
            // promptly, but other group members may have closed their fds.
            if stopping && !terminal_closed {
                return Ok(None);
            }
            self.kill();
            let status = self.child.wait()?;
            self.reaped = true;
            Ok(Some(status))
        }

        fn signal(&self, signal: libc::c_int) {
            if !self.reaped {
                // This unreaped child owns the PID and created this process group.
                unsafe { libc::kill(-(self.child.id() as libc::pid_t), signal) };
            }
        }

        pub fn terminate(&self) {
            self.signal(libc::SIGTERM);
        }

        pub fn kill(&self) {
            self.signal(libc::SIGKILL);
        }
    }

    impl Drop for Pty {
        fn drop(&mut self) {
            self.kill();
            if !self.reaped {
                let _ = self.child.wait();
            }
        }
    }
}

#[cfg(unix)]
pub use unix::Pty;

#[cfg(not(unix))]
mod unsupported {
    use std::{
        io,
        process::{Command, ExitStatus},
    };

    pub struct Pty;

    impl Pty {
        pub fn spawn(_: &mut Command) -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "SSH install is unavailable on Windows",
            ))
        }
        pub fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            unreachable!()
        }
        pub fn answer(&mut self, _: &[u8]) -> io::Result<()> {
            unreachable!()
        }
        pub fn try_wait(&mut self, _: bool, _: bool) -> io::Result<Option<ExitStatus>> {
            unreachable!()
        }
        pub fn terminate(&self) {
            unreachable!()
        }
        pub fn kill(&self) {
            unreachable!()
        }
    }
}

#[cfg(not(unix))]
pub use unsupported::Pty;
