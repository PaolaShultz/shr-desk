//! Bounded streaming APC demultiplexer, used before and during interaction.
//! Byte fragmentation never changes protocol meaning; unrelated input is retained.
use crate::assets;
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    time::{Duration, Instant},
};
#[derive(Default)]
pub struct Input {
    pending: Vec<u8>,
    pub keys: VecDeque<u8>,
    pub replies: VecDeque<String>,
    pub overflow: bool,
    discarding: bool,
    discard_escape: bool,
}
impl Input {
    pub fn feed(&mut self, bytes: &[u8]) {
        for &c in bytes {
            if self.discarding {
                if self.discard_escape && c == b'\\' {
                    self.discarding = false;
                }
                self.discard_escape = c == 27;
                continue;
            }
            if self.pending.is_empty() {
                if c == 27 {
                    self.pending.push(c)
                } else {
                    self.key(c)
                };
                continue;
            }
            self.pending.push(c);
            if self.pending.len() == 2 && c != b'_' {
                let p = std::mem::take(&mut self.pending);
                for x in p {
                    self.key(x)
                }
                continue;
            }
            if self.pending.ends_with(b"\x1b\\") {
                let p = std::mem::take(&mut self.pending);
                if let Ok(s) = std::str::from_utf8(&p[2..p.len() - 2])
                    && s.starts_with("25a1;")
                {
                    if self.replies.len() < 64 {
                        self.replies.push_back(s.into());
                    } else {
                        self.overflow = true;
                    }
                }
            } else if self.pending.len() > 2048 {
                self.pending.clear();
                self.discarding = true;
                self.overflow = true;
            }
        }
    }
    fn key(&mut self, c: u8) {
        if self.keys.len() < 1024 {
            self.keys.push_back(c)
        } else {
            self.overflow = true;
        }
    }
    pub fn read(&mut self, ms: i32) -> io::Result<()> {
        let mut p = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized descriptor, valid for this synchronous call.
        let n = unsafe { libc::poll(&mut p, 1, ms) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n > 0 {
            let mut b = [0; 1024];
            let n = io::stdin().read(&mut b)?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.feed(&b[..n]);
        }
        Ok(())
    }
    fn wait(&mut self, predicate: impl Fn(&str) -> bool) -> io::Result<Option<String>> {
        let end = Instant::now() + Duration::from_millis(500);
        loop {
            if let Some(i) = self.replies.iter().position(|s| predicate(s)) {
                return Ok(self.replies.remove(i));
            }
            if Instant::now() >= end {
                return Ok(None);
            }
            self.read(10)?;
        }
    }
}
pub fn field<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let mut found = None;
    for part in s.split(';').skip(2) {
        if let Some((k, v)) = part.split_once('=')
            && k == key
        {
            if found.is_some() {
                return None;
            }
            found = Some(v);
        }
    }
    found
}
pub struct Session {
    owned: Vec<u32>,
}
impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}
impl Session {
    pub fn new() -> Self {
        Self { owned: Vec::new() }
    }
    pub fn register(&mut self, out: &mut impl Write, input: &mut Input) -> io::Result<bool> {
        out.write_all(b"\x1b_25a1;s\x1b\\")?;
        out.flush()?;
        let Some(s) = input.wait(|s| s.starts_with("25a1;s;"))? else {
            return Ok(false);
        };
        if !field(&s, "fmt").is_some_and(|f| f.split(',').any(|f| f == "colrv0")) {
            return Ok(false);
        }
        for (i, (_, payload)) in assets::ASSETS.iter().enumerate() {
            let cp = assets::BASE + i as u32;
            let hex = format!("{cp:x}");
            write!(out, "\x1b_25a1;q;cp={hex}\x1b\\")?;
            out.flush()?;
            let Some(q) =
                input.wait(|s| s.starts_with("25a1;q;") && field(s, "cp") == Some(&hex))?
            else {
                return Ok(false);
            };
            // Never overwrite somebody else's registration or system-font coverage.
            if field(&q, "status") != Some("") {
                return Ok(false);
            }
            self.owned.push(cp); // Clear even if the ACK is lost after successful registration.
            write!(
                out,
                "\x1b_25a1;r;cp={hex};fmt=colrv0;reply=1;upm=1000;aw=1000;lh=1000;width=1;size=stretch;align=center,center;pad=0,0,0,0;{payload}\x1b\\"
            )?;
            out.flush()?;
            let Some(r) =
                input.wait(|s| s.starts_with("25a1;r;") && field(s, "cp") == Some(&hex))?
            else {
                return Ok(false);
            };
            if field(&r, "status") != Some("0") {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub fn clear(&mut self, out: &mut impl Write, input: &mut Input) -> io::Result<()> {
        // Send every owned clear before waiting: a lost ACK must not leave the
        // rest registered. Keep ownership on failure so a caller can retry.
        for cp in &self.owned {
            write!(out, "\x1b_25a1;c;cp={cp:x}\x1b\\")?;
        }
        out.flush()?;
        for _ in 0..self.owned.len() {
            // Older spec omits cp in clear ACK; count this bounded batch.
            if input.wait(|s| s.starts_with("25a1;c;"))?.is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "glyph cleanup ACK timeout",
                ));
            }
        }
        self.owned.clear();
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_reply_keeps_keyboard_input() {
        let bytes = b"a\x1b_25a1;s;fmt=glyf,colrv0\x1b\\b";
        for chunk in 1..bytes.len() {
            let mut i = Input::default();
            for b in bytes.chunks(chunk) {
                i.feed(b);
            }
            assert_eq!(i.keys.into_iter().collect::<Vec<_>>(), b"ab");
            assert_eq!(
                i.replies.pop_front().as_deref(),
                Some("25a1;s;fmt=glyf,colrv0")
            );
        }
    }
    #[test]
    fn malformed_and_duplicate_fields_fail_closed() {
        assert_eq!(field("25a1;s;fmt=colrv0;fmt=glyf", "fmt"), None);
        let mut i = Input::default();
        i.feed(b"\x1b_25a1;");
        i.feed(&vec![b'x'; 3000]);
        assert!(i.overflow);
        assert!(i.pending.len() <= 2048);
        assert!(i.keys.len() <= 1024);
    }
    #[test]
    fn arrows_are_not_protocol() {
        let mut i = Input::default();
        i.feed(b"\x1b[A");
        assert_eq!(i.keys.into_iter().collect::<Vec<_>>(), b"\x1b[A");
    }
}
