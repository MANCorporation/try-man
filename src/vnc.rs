//! A tiny VNC (RFB 3.8) client, so the VM screen can be shown inside our
//! own Adwaita window instead of a separate QEMU window.
//!
//! It talks to QEMU over a private connection and only needs the simplest
//! encodings: Raw, CopyRect, DesktopSize and Cursor.

use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::net::{Endpoint, Stream};

const ENC_RAW: i32 = 0;
const ENC_COPYRECT: i32 = 1;
const ENC_DESKTOP_SIZE: i32 = -223;
const ENC_CURSOR: i32 = -239;

/// Mouse pointer image, RGBA.
pub struct Cursor {
    pub width: u32,
    pub height: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub rgba: Vec<u8>,
}

struct Screen {
    width: u32,
    height: u32,
    /// BGRx, 4 bytes per pixel.
    pixels: Vec<u8>,
    dirty: bool,
    cursor: Option<Cursor>,
}

pub struct VncClient {
    writer: Arc<Mutex<Stream>>,
    screen: Arc<Mutex<Screen>>,
    closed: Arc<AtomicBool>,
}

impl VncClient {
    pub fn connect(endpoint: &Endpoint) -> io::Result<Arc<Self>> {
        let mut stream = endpoint.connect()?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let (width, height) = handshake(&mut stream)?;
        stream.set_read_timeout(None)?;

        let screen = Arc::new(Mutex::new(Screen {
            width,
            height,
            pixels: vec![0; (width * height * 4) as usize],
            dirty: true,
            cursor: None,
        }));
        let writer = Arc::new(Mutex::new(stream.try_clone()?));
        let closed = Arc::new(AtomicBool::new(false));

        {
            let screen = screen.clone();
            let writer = writer.clone();
            let closed = closed.clone();
            std::thread::spawn(move || {
                let _ = read_loop(stream, &writer, &screen);
                closed.store(true, Ordering::Relaxed);
            });
        }

        let client = Arc::new(VncClient { writer, screen, closed });
        request_update(&client.writer, false, width, height)?;
        Ok(client)
    }

    fn send(&self, data: &[u8]) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(data);
        }
    }

    /// `keysym` is an X11 keysym; GDK key values are the same numbers.
    pub fn key(&self, keysym: u32, down: bool) {
        let mut msg = [4u8, down as u8, 0, 0, 0, 0, 0, 0];
        msg[4..8].copy_from_slice(&keysym.to_be_bytes());
        self.send(&msg);
    }

    /// `buttons`: bit 0 = left, 1 = middle, 2 = right, 3/4 = wheel up/down.
    pub fn pointer(&self, buttons: u8, x: u16, y: u16) {
        let mut msg = [5u8, buttons, 0, 0, 0, 0];
        msg[2..4].copy_from_slice(&x.to_be_bytes());
        msg[4..6].copy_from_slice(&y.to_be_bytes());
        self.send(&msg);
    }

    pub fn size(&self) -> (u32, u32) {
        let s = self.screen.lock().unwrap();
        (s.width, s.height)
    }

    /// A copy of the screen, if it changed since the last call.
    pub fn take_frame(&self) -> Option<(u32, u32, Vec<u8>)> {
        let mut s = self.screen.lock().unwrap();
        if !s.dirty || s.width == 0 || s.height == 0 {
            return None;
        }
        s.dirty = false;
        Some((s.width, s.height, s.pixels.clone()))
    }

    pub fn take_cursor(&self) -> Option<Cursor> {
        self.screen.lock().unwrap().cursor.take()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    pub fn shutdown(&self) {
        if let Ok(w) = self.writer.lock() {
            let _ = w.shutdown();
        }
    }
}

fn read_u8(s: &mut impl Read) -> io::Result<u8> {
    let mut b = [0u8; 1];
    s.read_exact(&mut b)?;
    Ok(b[0])
}

fn read_u16(s: &mut impl Read) -> io::Result<u16> {
    let mut b = [0u8; 2];
    s.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}

fn read_u32(s: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    s.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

fn skip(s: &mut impl Read, n: usize) -> io::Result<()> {
    io::copy(&mut s.take(n as u64), &mut io::sink()).and_then(|copied| {
        if copied as usize == n {
            Ok(())
        } else {
            Err(io::ErrorKind::UnexpectedEof.into())
        }
    })
}

fn protocol_error(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn handshake(s: &mut Stream) -> io::Result<(u32, u32)> {
    let mut version = [0u8; 12];
    s.read_exact(&mut version)?;
    if &version[..4] != b"RFB " {
        return Err(protocol_error("not a VNC server"));
    }
    s.write_all(b"RFB 003.008\n")?;

    let count = read_u8(s)? as usize;
    if count == 0 {
        let len = read_u32(s)? as usize;
        let mut reason = vec![0u8; len];
        s.read_exact(&mut reason)?;
        return Err(protocol_error(String::from_utf8_lossy(&reason)));
    }
    let mut types = vec![0u8; count];
    s.read_exact(&mut types)?;
    if !types.contains(&1) {
        return Err(protocol_error("the VNC server wants a password"));
    }
    s.write_all(&[1])?; // security: none
    if read_u32(s)? != 0 {
        return Err(protocol_error("VNC login failed"));
    }

    s.write_all(&[1])?; // ClientInit: shared
    let width = read_u16(s)? as u32;
    let height = read_u16(s)? as u32;
    skip(s, 16)?; // server pixel format
    let name_len = read_u32(s)? as usize;
    skip(s, name_len)?;

    // SetPixelFormat: 32 bpp, little endian, R<<16 G<<8 B  => bytes B,G,R,x
    s.write_all(&[
        0, 0, 0, 0, // type + padding
        32, 24, 0, 1, // bpp, depth, big-endian, true-colour
        0, 255, 0, 255, 0, 255, // max r, g, b
        16, 8, 0, // shifts
        0, 0, 0, // padding
    ])?;

    let encodings = [ENC_COPYRECT, ENC_RAW, ENC_DESKTOP_SIZE, ENC_CURSOR];
    let mut msg = vec![2u8, 0];
    msg.extend((encodings.len() as u16).to_be_bytes());
    for e in encodings {
        msg.extend(e.to_be_bytes());
    }
    s.write_all(&msg)?;

    Ok((width, height))
}

fn request_update(writer: &Mutex<Stream>, incremental: bool, w: u32, h: u32) -> io::Result<()> {
    let mut msg = [3u8, incremental as u8, 0, 0, 0, 0, 0, 0, 0, 0];
    msg[6..8].copy_from_slice(&(w as u16).to_be_bytes());
    msg[8..10].copy_from_slice(&(h as u16).to_be_bytes());
    writer.lock().unwrap().write_all(&msg)
}

/// Copies a rectangle of BGRx pixels onto the screen, clipped to its size.
fn blit(screen: &mut Screen, x: u32, y: u32, w: u32, h: u32, src: &[u8]) {
    if x >= screen.width || y >= screen.height {
        return;
    }
    let copy_w = w.min(screen.width - x) as usize * 4;
    for row in 0..h.min(screen.height - y) {
        let dst = (((y + row) * screen.width + x) * 4) as usize;
        let src_off = (row * w * 4) as usize;
        screen.pixels[dst..dst + copy_w].copy_from_slice(&src[src_off..src_off + copy_w]);
    }
}

fn read_loop(mut s: Stream, writer: &Mutex<Stream>, screen: &Mutex<Screen>) -> io::Result<()> {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        match read_u8(&mut s)? {
            // FramebufferUpdate
            0 => {
                skip(&mut s, 1)?;
                let rects = read_u16(&mut s)?;
                let mut resized = false;
                for _ in 0..rects {
                    let x = read_u16(&mut s)? as u32;
                    let y = read_u16(&mut s)? as u32;
                    let w = read_u16(&mut s)? as u32;
                    let h = read_u16(&mut s)? as u32;
                    let encoding = read_u32(&mut s)? as i32;
                    match encoding {
                        ENC_RAW => {
                            buf.resize((w * h * 4) as usize, 0);
                            s.read_exact(&mut buf)?;
                            blit(&mut screen.lock().unwrap(), x, y, w, h, &buf);
                        }
                        ENC_COPYRECT => {
                            let sx = read_u16(&mut s)? as u32;
                            let sy = read_u16(&mut s)? as u32;
                            let mut sc = screen.lock().unwrap();
                            if sx + w <= sc.width && sy + h <= sc.height {
                                buf.clear();
                                for row in 0..h {
                                    let off = (((sy + row) * sc.width + sx) * 4) as usize;
                                    buf.extend_from_slice(&sc.pixels[off..off + (w * 4) as usize]);
                                }
                                blit(&mut sc, x, y, w, h, &buf);
                            }
                        }
                        ENC_DESKTOP_SIZE => {
                            let mut sc = screen.lock().unwrap();
                            sc.width = w;
                            sc.height = h;
                            sc.pixels = vec![0; (w * h * 4) as usize];
                            resized = true;
                        }
                        ENC_CURSOR => {
                            let pixel_len = (w * h * 4) as usize;
                            let mask_stride = w.div_ceil(8) as usize;
                            buf.resize(pixel_len + mask_stride * h as usize, 0);
                            s.read_exact(&mut buf)?;
                            let (pixels, mask) = buf.split_at(pixel_len);
                            let mut rgba = vec![0u8; pixel_len];
                            for py in 0..h as usize {
                                for px in 0..w as usize {
                                    let i = (py * w as usize + px) * 4;
                                    let visible = mask[py * mask_stride + px / 8] & (0x80 >> (px % 8)) != 0;
                                    rgba[i] = pixels[i + 2];
                                    rgba[i + 1] = pixels[i + 1];
                                    rgba[i + 2] = pixels[i];
                                    rgba[i + 3] = if visible { 255 } else { 0 };
                                }
                            }
                            screen.lock().unwrap().cursor = Some(Cursor {
                                width: w,
                                height: h,
                                hot_x: x,
                                hot_y: y,
                                rgba,
                            });
                        }
                        other => {
                            return Err(protocol_error(format!("unsupported VNC encoding {other}")));
                        }
                    }
                }
                let (w, h) = {
                    let mut sc = screen.lock().unwrap();
                    sc.dirty = true;
                    (sc.width, sc.height)
                };
                request_update(writer, !resized, w, h)?;
            }
            // SetColourMapEntries
            1 => {
                skip(&mut s, 3)?;
                let n = read_u16(&mut s)? as usize;
                skip(&mut s, n * 6)?;
            }
            // Bell
            2 => {}
            // ServerCutText
            3 => {
                skip(&mut s, 3)?;
                let len = read_u32(&mut s)? as usize;
                skip(&mut s, len)?;
            }
            other => return Err(protocol_error(format!("unknown VNC message {other}"))),
        }
    }
}
