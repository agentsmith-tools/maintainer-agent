//! The two output streams: a closed pipe ends the run the way a shell reports it.

use mx_install::console::{BROKEN_PIPE, Console, Exit};
use std::io::{self, ErrorKind, Write};

struct Failing(ErrorKind);

impl Write for Failing {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(self.0))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_write_to_a_closed_pipe_ends_the_run_with_141_on_either_stream() {
    let mut err = Vec::new();
    let mut gone = Failing(ErrorKind::BrokenPipe);
    assert_eq!(BROKEN_PIPE, 141);
    assert_eq!(Console::new(&mut gone, &mut err).say(b"x"), Err(Exit(141)));
    let mut out = Vec::new();
    let mut gone = Failing(ErrorKind::BrokenPipe);
    assert_eq!(Console::new(&mut out, &mut gone).echo_err(b"x"), Err(Exit(141)));
}

#[test]
fn another_write_error_is_not_a_reason_to_stop_and_the_text_reaches_a_working_stream_whole() {
    let mut err = Vec::new();
    let mut broken = Failing(ErrorKind::PermissionDenied);
    assert_eq!(Console::new(&mut broken, &mut err).out(b"x"), Ok(()));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    {
        let mut con = Console::new(&mut out, &mut err);
        con.say(b"one").unwrap();
        con.echo(b"two").unwrap();
        con.echo_err(b"three").unwrap();
        con.err(b"four").unwrap();
    }
    assert_eq!(out, b"  one\ntwo\n");
    assert_eq!(err, b"three\nfour");
}
