//! Private connections to QEMU's control (QMP) and screen (VNC) servers.
//!
//! On Linux and macOS these are Unix sockets in a private folder. Windows has
//! no Unix sockets in QEMU, so there they are TCP ports on 127.0.0.1.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
#[cfg(unix)]
use std::path::PathBuf;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::net::UnixStream;

#[derive(Clone, Debug)]
pub enum Endpoint {
    #[cfg(unix)]
    Unix(PathBuf),
    Tcp(u16),
}

pub enum Stream {
    #[cfg(unix)]
    Unix(UnixStream),
    Tcp(TcpStream),
}

/// Doubles commas, which separate QEMU option values.
#[cfg(unix)]
fn opt(p: &std::path::Path) -> String {
    p.to_string_lossy().replace(',', ",,")
}

impl Endpoint {
    pub fn connect(&self) -> io::Result<Stream> {
        match self {
            #[cfg(unix)]
            Endpoint::Unix(path) => UnixStream::connect(path).map(Stream::Unix),
            Endpoint::Tcp(port) => {
                let addr = std::net::SocketAddr::from(([127, 0, 0, 1], *port));
                let s = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
                let _ = s.set_nodelay(true);
                Ok(Stream::Tcp(s))
            }
        }
    }

    /// Whether QEMU may already be listening (a Unix socket file exists).
    pub fn ready(&self) -> bool {
        match self {
            #[cfg(unix)]
            Endpoint::Unix(path) => path.exists(),
            Endpoint::Tcp(_) => true,
        }
    }

    /// The value for QEMU's `-vnc` option.
    pub fn vnc_arg(&self) -> String {
        match self {
            #[cfg(unix)]
            Endpoint::Unix(path) => format!("unix:{}", opt(path)),
            // VNC takes a "display number": port 5900 + n.
            Endpoint::Tcp(port) => format!("127.0.0.1:{}", port - 5900),
        }
    }

    /// The value for QEMU's `-qmp` option.
    pub fn qmp_arg(&self) -> String {
        match self {
            #[cfg(unix)]
            Endpoint::Unix(path) => format!("unix:{},server=on,wait=off", opt(path)),
            Endpoint::Tcp(port) => format!("tcp:127.0.0.1:{port},server=on,wait=off"),
        }
    }
}

/// A free TCP port on 127.0.0.1 within `range`.
#[allow(dead_code)]
pub fn free_port(range: std::ops::Range<u16>) -> Option<u16> {
    range
        .into_iter()
        .find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
}

impl Stream {
    pub fn try_clone(&self) -> io::Result<Stream> {
        match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.try_clone().map(Stream::Unix),
            Stream::Tcp(s) => s.try_clone().map(Stream::Tcp),
        }
    }

    pub fn set_read_timeout(&self, t: Option<Duration>) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.set_read_timeout(t),
            Stream::Tcp(s) => s.set_read_timeout(t),
        }
    }

    pub fn shutdown(&self) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.shutdown(Shutdown::Both),
            Stream::Tcp(s) => s.shutdown(Shutdown::Both),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.read(buf),
            Stream::Tcp(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.write(buf),
            Stream::Tcp(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Stream::Unix(s) => s.flush(),
            Stream::Tcp(s) => s.flush(),
        }
    }
}
