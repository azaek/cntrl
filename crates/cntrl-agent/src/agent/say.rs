//! Printing for the command line. `println!` panics once the reader has gone,
//! as when `cntrl policy allow x | head -1` closes the pipe after one line;
//! these carry on quietly instead, so a command still finishes what it
//! started, such as asking the agent to reload its policy.

/// `println!` that a closed pipe can't stop.
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

/// `eprintln!` that a closed pipe can't stop.
macro_rules! say_err {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

pub(crate) use {say, say_err};
