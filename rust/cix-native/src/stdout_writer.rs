//! Interrupt-aware stdout writes for CIX streams.
//!
//! A pipe reader can stop consuming while keeping the pipe open.  A blocking
//! `write(2)` then cannot observe the process cancellation flag, especially
//! when the platform restarts interrupted syscalls.  This writer owns a dup of
//! stdout, makes that dup nonblocking, and waits in short `poll(POLLOUT)`
//! intervals so `limits::check` remains observable.

#[cfg(unix)]
mod unix {
    use std::io;
    use std::os::fd::RawFd;

    const POLL_INTERVAL_MS: libc::c_int = 50;

    /// A Unix stdout writer which makes stopped pipe consumers interruptible.
    ///
    /// `new` duplicates fd 1.  The duplicate keeps the writer valid if a caller
    /// rearranges stdio descriptors later, and it lets `Drop` close only the fd it
    /// owns.  File-status flags are shared by duplicate descriptors, so the
    /// original nonblocking state is restored when the writer is dropped.
    #[derive(Debug)]
    pub struct StdoutWriter {
        fd: RawFd,
        restore_nonblocking: bool,
    }

    impl StdoutWriter {
        /// Creates a writer for the process's current stdout.
        pub fn new() -> io::Result<Self> {
            Self::from_fd(libc::STDOUT_FILENO)
        }

        /// Creates a writer from a borrowed Unix fd by duplicating it.
        ///
        /// This is useful for embedding and tests. The supplied descriptor stays
        /// owned by the caller; the returned writer owns and closes its duplicate.
        pub fn from_fd(fd: RawFd) -> io::Result<Self> {
            if fd < 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "stdout file descriptor is negative",
                ));
            }
            let duplicate = duplicate_cloexec(fd)?;
            match Self::from_owned_fd(duplicate) {
                Ok(writer) => Ok(writer),
                Err(error) => {
                    // SAFETY: `duplicate` was returned by dup and has not moved.
                    unsafe {
                        libc::close(duplicate);
                    }
                    Err(error)
                }
            }
        }

        fn from_owned_fd(fd: RawFd) -> io::Result<Self> {
            // SAFETY: fd is an owned, valid Unix file descriptor.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 {
                return Err(io::Error::last_os_error());
            }
            let restore_nonblocking = flags & libc::O_NONBLOCK == 0;
            if restore_nonblocking {
                // O_NONBLOCK is a file-status flag.  We retain every other flag.
                // SAFETY: fd remains owned by this writer construction.
                if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(Self {
                fd,
                restore_nonblocking,
            })
        }

        fn check_interrupted() -> io::Result<()> {
            crate::limits::check()
                .map_err(|message| io::Error::new(io::ErrorKind::Interrupted, message))
        }

        fn wait_writable(&self) -> io::Result<()> {
            loop {
                Self::check_interrupted()?;
                let mut descriptor = libc::pollfd {
                    fd: self.fd,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                // SAFETY: descriptor points to initialized storage for one pollfd.
                let outcome = unsafe { libc::poll(&mut descriptor, 1, POLL_INTERVAL_MS) };
                if outcome == 0 {
                    continue;
                }
                if outcome < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        Self::check_interrupted()?;
                        continue;
                    }
                    return Err(error);
                }
                let events = descriptor.revents;
                if events & libc::POLLOUT != 0 {
                    return Ok(());
                }
                if events & libc::POLLNVAL != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "stdout descriptor became invalid",
                    ));
                }
                if events & (libc::POLLERR | libc::POLLHUP) != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "stdout consumer closed its pipe",
                    ));
                }
                // A spurious readiness wake is harmless; the next poll stays
                // bounded and tests cancellation again.
            }
        }
    }

    impl io::Write for StdoutWriter {
        /// Writes the complete supplied buffer unless cancellation or I/O fails.
        ///
        /// Returning the complete count makes callers using a trait object safe
        /// even if they call `write` directly rather than `write_all`.
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            let mut written = 0;
            while written < buffer.len() {
                Self::check_interrupted()?;
                self.wait_writable()?;
                // SAFETY: the buffer remains live for the call and `written` is
                // kept within its bounds by the loop invariant.
                let count = unsafe {
                    libc::write(
                        self.fd,
                        buffer[written..].as_ptr().cast::<libc::c_void>(),
                        buffer.len() - written,
                    )
                };
                if count > 0 {
                    written += count as usize;
                    continue;
                }
                if count == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "stdout write made no progress",
                    ));
                }
                let error = io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(errno) if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK => continue,
                    Some(libc::EINTR) => {
                        Self::check_interrupted()?;
                    }
                    _ => return Err(error),
                }
            }
            Self::check_interrupted()?;
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            // write(2) has already handed all buffered data to the kernel; there
            // is no userspace buffer to flush here.
            Self::check_interrupted()
        }

        fn write_all(&mut self, buffer: &[u8]) -> io::Result<()> {
            // `std::io::Write::write_all` retries `Interrupted` by design.  CIX
            // uses that error kind for a cooperative Ctrl-C, so retrying would
            // turn a stopped consumer into an uninterruptible loop. `write`
            // already handles partial kernel writes itself and returns only after
            // consuming the entire buffer or seeing a terminal condition.
            self.write(buffer).map(|_| ())
        }
    }

    impl Drop for StdoutWriter {
        fn drop(&mut self) {
            if self.restore_nonblocking {
                // Preserve any file-status changes made while the writer existed,
                // changing only the bit this writer enabled.  It is best-effort:
                // Drop cannot report an error, but the owned descriptor is still
                // always closed below.
                // SAFETY: self.fd remains owned until the close below.
                let flags = unsafe { libc::fcntl(self.fd, libc::F_GETFL) };
                if flags >= 0 {
                    // SAFETY: self.fd remains owned until the close below.
                    unsafe {
                        libc::fcntl(self.fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
                    }
                }
            }
            // SAFETY: this writer owns exactly one duplicate fd.
            unsafe {
                libc::close(self.fd);
            }
        }
    }

    fn duplicate_cloexec(fd: RawFd) -> io::Result<RawFd> {
        // F_DUPFD_CLOEXEC is available on the supported Unix host.  It avoids an
        // unrelated child inheriting this temporary duplicate if it is spawned
        // while CIX is streaming output.
        // SAFETY: fcntl reads a valid borrowed fd and returns a new descriptor.
        let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        if duplicate >= 0 {
            Ok(duplicate)
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(unix)]
pub use unix::StdoutWriter;

/// Portable stdout writer fallback.
///
/// The bounded nonblocking pipe implementation is currently qualified on the
/// native Unix host. Other targets retain ordinary `std::io::Stdout` behavior
/// and compile without Unix descriptor APIs.
#[cfg(not(unix))]
pub struct StdoutWriter {
    stdout: std::io::Stdout,
}

#[cfg(not(unix))]
impl StdoutWriter {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            stdout: std::io::stdout(),
        })
    }

    fn check_interrupted() -> std::io::Result<()> {
        crate::limits::check()
            .map_err(|message| std::io::Error::new(std::io::ErrorKind::Interrupted, message))
    }
}

#[cfg(not(unix))]
impl std::io::Write for StdoutWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        Self::check_interrupted()?;
        let mut lock = self.stdout.lock();
        std::io::Write::write(&mut lock, buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Self::check_interrupted()?;
        let mut lock = self.stdout.lock();
        std::io::Write::flush(&mut lock)
    }

    fn write_all(&mut self, buffer: &[u8]) -> std::io::Result<()> {
        Self::check_interrupted()?;
        let mut lock = self.stdout.lock();
        std::io::Write::write_all(&mut lock, buffer)
    }
}
