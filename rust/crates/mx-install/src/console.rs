//! The two output streams of a program, written the moment text is made.
//!
//! A bash script's standard output and standard error are unbuffered files, so a
//! capture of both shows the lines in the order the script made them. This type
//! writes and flushes at every call to keep that order. A write that meets a closed
//! pipe ends the run with status 141, which is what a shell reports for a process the
//! `SIGPIPE` killed (Rust ignores the signal, and restoring it needs `unsafe`).

use std::ffi::OsStr;
use std::io::{ErrorKind, Write};
use std::os::unix::ffi::OsStrExt;

/// The status a program ends with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit(pub i32);

/// What an operation that may stop the program returns.
pub type Flow<T = ()> = Result<T, Exit>;

/// Where a program writes.
pub struct Console<'a> {
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
}

/// The status of a process the `SIGPIPE` killed.
pub const BROKEN_PIPE: i32 = 141;

fn put(sink: &mut dyn Write, bytes: &[u8]) -> Flow {
    match sink.write_all(bytes).and_then(|()| sink.flush()) {
        Err(error) if error.kind() == ErrorKind::BrokenPipe => Err(Exit(BROKEN_PIPE)),
        // Bash reports other write errors and goes on; there is nowhere to report this one.
        Ok(()) | Err(_) => Ok(()),
    }
}

impl<'a> Console<'a> {
    pub fn new(out: &'a mut dyn Write, err: &'a mut dyn Write) -> Self {
        Self { out, err }
    }

    /// Bytes to standard output, as they are.
    pub fn out(&mut self, bytes: &[u8]) -> Flow {
        put(self.out, bytes)
    }

    /// Bytes to standard error, as they are.
    pub fn err(&mut self, bytes: &[u8]) -> Flow {
        put(self.err, bytes)
    }

    /// `echo text`: the text and a newline on standard output.
    pub fn echo(&mut self, text: &[u8]) -> Flow {
        let mut line = text.to_vec();
        line.push(b'\n');
        self.out(&line)
    }

    /// `echo text >&2`.
    pub fn echo_err(&mut self, text: &[u8]) -> Flow {
        let mut line = text.to_vec();
        line.push(b'\n');
        self.err(&line)
    }

    /// `say text`: two spaces, the text and a newline on standard output.
    pub fn say(&mut self, text: &[u8]) -> Flow {
        let mut line = b"  ".to_vec();
        line.extend_from_slice(text);
        line.push(b'\n');
        self.out(&line)
    }
}

/// The bytes of a path or any other operating-system string.
#[must_use]
pub fn bytes(text: &OsStr) -> &[u8] {
    text.as_bytes()
}
