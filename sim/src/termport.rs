//! The simulator's programming cable for the terminal (manual chapter 37): a pseudo-terminal that `screen` or
//! `minicom` opens like the real cable, or a TCP port for systems without one (`--term-tcp PORT`). Bytes move
//! between it and [`SimCore`] once per loop, never blocking the window.

use ferrite_sim::simcore::SimCore;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::FromRawFd;

pub enum Port {
    /// The master side, and the slave kept open so the master never reads EIO while no program has it.
    Pty { master: File, _slave: File, name: String, pending: Vec<u8> },
    /// `bt`: the BT513's serial side (a phone's link or terminal) instead of the cable.
    Tcp { listener: TcpListener, client: Option<TcpStream>, port: u16, pending: Vec<u8>, bt: bool },
}

impl Port {
    /// A new pseudo-terminal, raw (no echo, no line editing) at 115200, the master non-blocking.
    pub fn pty() -> std::io::Result<Port> {
        let (mut m, mut s) = (0, 0);
        let mut name = [0 as libc::c_char; 128];
        // SAFETY: plain libc calls with valid pointers; the fds are owned by the Files made from them.
        unsafe {
            if libc::openpty(&mut m, &mut s, name.as_mut_ptr(), core::ptr::null_mut(), core::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut t: libc::termios = core::mem::zeroed();
            libc::tcgetattr(s, &mut t);
            libc::cfmakeraw(&mut t);
            libc::cfsetspeed(&mut t, libc::B115200);
            libc::tcsetattr(s, libc::TCSANOW, &t);
            let fl = libc::fcntl(m, libc::F_GETFL);
            libc::fcntl(m, libc::F_SETFL, fl | libc::O_NONBLOCK);
            let name = std::ffi::CStr::from_ptr(name.as_ptr()).to_string_lossy().into_owned();
            Ok(Port::Pty { master: File::from_raw_fd(m), _slave: File::from_raw_fd(s), name, pending: Vec::new() })
        }
    }

    /// A TCP port on localhost; one client at a time.
    pub fn tcp(port: u16) -> std::io::Result<Port> {
        Self::tcp_on(port, false)
    }

    /// A TCP port on localhost standing in for Bluetooth (`--bt-tcp PORT`): what a phone sends over the BT513
    /// and what the radio sends back, as `ferrite_link.py tcp:...` on the cable's side does for the cable.
    pub fn tcp_bt(port: u16) -> std::io::Result<Port> {
        Self::tcp_on(port, true)
    }

    fn tcp_on(port: u16, bt: bool) -> std::io::Result<Port> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        listener.set_nonblocking(true)?;
        Ok(Port::Tcp { listener, client: None, port, pending: Vec::new(), bt })
    }

    /// How to connect, for the console.
    pub fn describe(&self) -> String {
        match self {
            Port::Pty { name, .. } => format!("Terminal: screen {name} 115200   (press Enter; Ctrl-A K leaves screen)"),
            Port::Tcp { port, bt: true, .. } => format!("Bluetooth: tcp:127.0.0.1:{port}"),
            Port::Tcp { port, .. } => format!("Terminal: stty raw -echo; nc 127.0.0.1 {port}; stty sane   (press Enter)"),
        }
    }

    /// What the terminal program typed into the simulator; what the terminal drew out to it.
    pub fn pump(&mut self, sim: &mut SimCore) {
        let mut buf = [0u8; 256];
        match self {
            Port::Pty { master, pending, .. } => {
                while let Ok(n) = master.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    sim.term_feed(&buf[..n]);
                }
                pending.extend(sim.term_take());
                send(master, pending);
            }
            Port::Tcp { listener, client, pending, bt, .. } => {
                let bt = *bt;
                let take = |sim: &mut SimCore| if bt { sim.bt_take() } else { sim.term_take() };
                if client.is_none() {
                    if let Ok((c, _)) = listener.accept() {
                        let _ = c.set_nonblocking(true);
                        let _ = c.set_nodelay(true);
                        *client = Some(c);
                    }
                }
                let Some(c) = client else {
                    take(sim); // nobody on the cable: the output goes nowhere
                    return;
                };
                loop {
                    match c.read(&mut buf) {
                        Ok(0) => {
                            *client = None;
                            pending.clear();
                            return;
                        }
                        Ok(n) if bt => sim.bt_feed(&buf[..n]),
                        Ok(n) => sim.term_feed(&buf[..n]),
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(_) => {
                            *client = None;
                            return;
                        }
                    }
                }
                pending.extend(take(sim));
                send(c, pending);
            }
        }
    }
}

/// Writes what the other side takes now; the rest waits for the next pump.
fn send(w: &mut impl Write, pending: &mut Vec<u8>) {
    while !pending.is_empty() {
        match w.write(pending) {
            Ok(0) => break,
            Ok(n) => {
                pending.drain(..n);
            }
            Err(_) => break, // WouldBlock, or nobody reading: try again later
        }
    }
}
