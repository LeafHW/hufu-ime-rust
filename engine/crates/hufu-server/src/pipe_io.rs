//! Windows byte-pipe I/O with kernel event waits. Every pending operation is
//! cancelled and reaped before its OVERLAPPED, event, or caller buffer can die.

use std::io;
use std::time::{Duration, Instant};

#[repr(C)]
union OffsetOrPointer {
    offsets: [u32; 2],
    pointer: *mut core::ffi::c_void,
}

#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset_or_pointer: OffsetOrPointer,
    event: isize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateEventW(
        sa: *const core::ffi::c_void,
        manual: i32,
        initial: i32,
        name: *const u16,
    ) -> isize;
    fn CloseHandle(handle: isize) -> i32;
    fn ConnectNamedPipe(pipe: isize, overlapped: *mut Overlapped) -> i32;
    fn ReadFile(pipe: isize, buf: *mut u8, size: u32, got: *mut u32, ov: *mut Overlapped) -> i32;
    fn WriteFile(pipe: isize, buf: *const u8, size: u32, got: *mut u32, ov: *mut Overlapped)
        -> i32;
    fn WaitForSingleObject(handle: isize, timeout: u32) -> u32;
    fn CancelIoEx(pipe: isize, overlapped: *mut Overlapped) -> i32;
    fn GetOverlappedResult(
        pipe: isize,
        overlapped: *mut Overlapped,
        got: *mut u32,
        wait: i32,
    ) -> i32;
}

const ERROR_IO_PENDING: i32 = 997;
const ERROR_PIPE_CONNECTED: i32 = 535;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
const INFINITE: u32 = u32::MAX;

pub(super) struct OwnedHandle(pub(super) isize);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct Operation {
    pipe: isize,
    // Box keeps the OS-visible address fixed when this wrapper moves.
    overlapped: Box<Overlapped>,
    _event: OwnedHandle,
    pending: bool,
}

impl Operation {
    fn new(pipe: isize) -> io::Result<Self> {
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut overlapped: Box<Overlapped> = Box::new(unsafe { std::mem::zeroed() });
        overlapped.event = event;
        Ok(Self {
            pipe,
            overlapped,
            _event: OwnedHandle(event),
            pending: false,
        })
    }

    fn finish(&mut self, started: i32, deadline: Option<Instant>) -> io::Result<usize> {
        if started == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING) {
                self.pending = false;
                return Err(error);
            }
            // No polling: an idle established connection sleeps until data,
            // disconnect, or the operation's absolute deadline.
            loop {
                let timeout = match deadline {
                    None => INFINITE,
                    Some(end) => {
                        let remaining = end.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            return Err(io::Error::new(io::ErrorKind::TimedOut, "管道 I/O 超时"));
                        }
                        remaining
                            .as_millis()
                            .saturating_add(u128::from(remaining.subsec_nanos() % 1_000_000 != 0))
                            .min((INFINITE - 1) as u128) as u32
                    }
                };
                match unsafe { WaitForSingleObject(self.overlapped.event, timeout) } {
                    WAIT_OBJECT_0 => break,
                    WAIT_TIMEOUT => continue, // Recheck the absolute deadline.
                    _ => return Err(io::Error::last_os_error()),
                }
            }
        }
        let mut got = 0;
        let ok = unsafe { GetOverlappedResult(self.pipe, &mut *self.overlapped, &mut got, 0) };
        // Both success and an I/O completion error mean the request has settled.
        self.pending = false;
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(got as usize)
        }
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        if self.pending {
            unsafe {
                // Cancellation is only a request. Even if completion raced and
                // CancelIoEx returns ERROR_NOT_FOUND, reap before freeing memory.
                CancelIoEx(self.pipe, &mut *self.overlapped);
                let mut ignored = 0;
                GetOverlappedResult(self.pipe, &mut *self.overlapped, &mut ignored, 1);
            }
        }
    }
}

pub(super) fn connect(pipe: isize, deadline: Option<Instant>) -> io::Result<()> {
    let mut op = Operation::new(pipe)?;
    op.pending = true;
    let started = unsafe { ConnectNamedPipe(pipe, &mut *op.overlapped) };
    if started == 0 && io::Error::last_os_error().raw_os_error() == Some(ERROR_PIPE_CONNECTED) {
        // A client opened between CreateNamedPipe and ConnectNamedPipe; no I/O
        // was queued in this case, so GetOverlappedResult would be inappropriate.
        op.pending = false;
        return Ok(());
    }
    op.finish(started, deadline).map(|_| ())
}

pub(super) fn read_exact(
    pipe: isize,
    size: usize,
    mut deadline: Option<Instant>,
    partial_timeout: Option<Duration>,
) -> io::Result<Vec<u8>> {
    let mut buf = vec![0; size];
    let mut done = 0;
    while done < size {
        if deadline.is_some_and(|end| Instant::now() >= end) {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "管道读帧超时"));
        }
        let mut op = Operation::new(pipe)?;
        op.pending = true;
        let started = unsafe {
            ReadFile(
                pipe,
                buf.as_mut_ptr().add(done),
                (size - done).min(u32::MAX as usize) as u32,
                std::ptr::null_mut(),
                &mut *op.overlapped,
            )
        };
        let got = op.finish(started, deadline)?;
        if got == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "管道关闭"));
        }
        if done == 0 && got < size {
            if let Some(timeout) = partial_timeout {
                let end = Instant::now() + timeout;
                deadline = Some(deadline.map_or(end, |old| old.min(end)));
            }
        }
        done += got;
    }
    Ok(buf)
}

pub(super) fn write_all(pipe: isize, buf: &[u8], deadline: Instant) -> io::Result<()> {
    let mut done = 0;
    while done < buf.len() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "管道写帧超时"));
        }
        let mut op = Operation::new(pipe)?;
        op.pending = true;
        let started = unsafe {
            WriteFile(
                pipe,
                buf.as_ptr().add(done),
                (buf.len() - done).min(u32::MAX as usize) as u32,
                std::ptr::null_mut(),
                &mut *op.overlapped,
            )
        };
        let wrote = op.finish(started, Some(deadline))?;
        if wrote == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "管道写入零字节"));
        }
        done += wrote;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateNamedPipeW(
            name: *const u16,
            open: u32,
            mode: u32,
            instances: u32,
            out_size: u32,
            in_size: u32,
            timeout: u32,
            sa: *const core::ffi::c_void,
        ) -> isize;
        fn DisconnectNamedPipe(pipe: isize) -> i32;
    }

    struct TestPipe(OwnedHandle);
    impl Drop for TestPipe {
        fn drop(&mut self) {
            unsafe {
                DisconnectNamedPipe(self.0 .0);
            }
        }
    }

    fn listener() -> (TestPipe, String) {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let name = format!(
            r"\\.\pipe\hufu-io-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let handle = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                3 | 0x4000_0000,
                0,
                1,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };
        assert_ne!(handle, -1, "{}", io::Error::last_os_error());
        (TestPipe(OwnedHandle(handle)), name)
    }

    fn pair() -> (TestPipe, File) {
        let (server, name) = listener();
        let client = std::thread::spawn(move || {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(name)
                .unwrap()
        });
        connect(server.0 .0, Some(Instant::now() + Duration::from_secs(3))).unwrap();
        (server, client.join().unwrap())
    }

    #[test]
    fn overlapped_idle_header_wakes_and_accepts_fragmented_frame() {
        let (server, mut client) = pair();
        let worker = std::thread::spawn(move || {
            let head = read_exact(server.0 .0, 4, None, Some(Duration::from_secs(2))).unwrap();
            let body = read_exact(
                server.0 .0,
                5,
                Some(Instant::now() + Duration::from_secs(2)),
                None,
            )
            .unwrap();
            assert_eq!(head, [5, 0, 0, 0]);
            assert_eq!(body, b"hello");
            write_all(server.0 .0, b"ok", Instant::now() + Duration::from_secs(2)).unwrap();
        });
        // Typical human inter-key spacing: the server must wait on the event.
        std::thread::sleep(Duration::from_millis(83));
        client.write_all(&[5]).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        client.write_all(&[0, 0, 0, b'h', b'e']).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        client.write_all(b"llo").unwrap();
        let mut result = [0; 2];
        client.read_exact(&mut result).unwrap();
        assert_eq!(&result, b"ok");
        worker.join().unwrap();
    }

    #[test]
    fn overlapped_timeout_cancels_before_buffer_is_reused() {
        let (server, mut client) = pair();
        for _ in 0..16 {
            let result = read_exact(
                server.0 .0,
                4,
                Some(Instant::now() + Duration::from_millis(2)),
                None,
            );
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
            client.write_all(b"safe").unwrap();
            assert_eq!(
                read_exact(
                    server.0 .0,
                    4,
                    Some(Instant::now() + Duration::from_secs(2)),
                    None
                )
                .unwrap(),
                b"safe"
            );
        }
    }

    #[test]
    fn overlapped_large_response_spans_pipe_buffer() {
        let (server, mut client) = pair();
        let expected: Vec<u8> = (0..(1 << 20)).map(|i| (i % 251) as u8).collect();
        let output = expected.clone();
        let worker = std::thread::spawn(move || {
            write_all(
                server.0 .0,
                &output,
                Instant::now() + Duration::from_secs(3),
            )
            .unwrap();
        });
        let mut actual = vec![0; expected.len()];
        client.read_exact(&mut actual).unwrap();
        assert_eq!(actual, expected);
        worker.join().unwrap();
    }

    #[test]
    fn overlapped_partial_header_has_deadline_after_idle() {
        let (server, mut client) = pair();
        client.write_all(&[7]).unwrap();
        let result = read_exact(server.0 .0, 4, None, Some(Duration::from_millis(30)));
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        // A cancelled remainder must not consume the next read's data.
        client.write_all(b"new").unwrap();
        assert_eq!(
            read_exact(
                server.0 .0,
                3,
                Some(Instant::now() + Duration::from_secs(2)),
                None
            )
            .unwrap(),
            b"new"
        );
    }

    #[test]
    fn overlapped_body_uses_one_absolute_deadline() {
        let (server, mut client) = pair();
        client.write_all(b"ab").unwrap();
        let result = read_exact(
            server.0 .0,
            6,
            Some(Instant::now() + Duration::from_millis(30)),
            None,
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn overlapped_nonreading_client_does_not_pin_writer() {
        let (server, _client) = pair();
        let large = vec![42; 1 << 20];
        let result = write_all(
            server.0 .0,
            &large,
            Instant::now() + Duration::from_millis(30),
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn overlapped_disconnect_wakes_idle_reader() {
        let (server, client) = pair();
        let close = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            drop(client);
        });
        assert!(read_exact(server.0 .0, 4, None, Some(Duration::from_secs(2))).is_err());
        close.join().unwrap();
    }

    #[test]
    fn overlapped_accept_handles_preconnected_client_and_timeout() {
        let (server, name) = listener();
        let _client = OpenOptions::new()
            .read(true)
            .write(true)
            .open(name)
            .unwrap();
        connect(server.0 .0, Some(Instant::now() + Duration::from_secs(2))).unwrap();
        let (idle, _) = listener();
        assert_eq!(
            connect(idle.0 .0, Some(Instant::now() + Duration::from_millis(30)))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }
}
