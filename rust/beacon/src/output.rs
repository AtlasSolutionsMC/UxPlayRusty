//! Text output adapter that preserves Python's Windows CRLF translation.

use std::fmt;
use std::io::Write;

pub fn write_source_output(arguments: fmt::Arguments<'_>, newline: bool, stderr: bool) {
    let mut text = arguments.to_string();
    if newline {
        text.push('\n');
    }
    #[cfg(windows)]
    let text = text.replace('\n', "\r\n");

    if stderr {
        let _ = std::io::stderr().lock().write_all(text.as_bytes());
    } else {
        let _ = std::io::stdout().lock().write_all(text.as_bytes());
    }
}

#[macro_export]
macro_rules! source_println {
    () => {
        $crate::output::write_source_output(format_args!(""), true, false)
    };
    ($($argument:tt)*) => {
        $crate::output::write_source_output(format_args!($($argument)*), true, false)
    };
}

#[macro_export]
macro_rules! source_eprintln {
    () => {
        $crate::output::write_source_output(format_args!(""), true, true)
    };
    ($($argument:tt)*) => {
        $crate::output::write_source_output(format_args!($($argument)*), true, true)
    };
}

#[macro_export]
macro_rules! source_print {
    ($($argument:tt)*) => {
        $crate::output::write_source_output(format_args!($($argument)*), false, false)
    };
}

#[macro_export]
macro_rules! source_eprint {
    ($($argument:tt)*) => {
        $crate::output::write_source_output(format_args!($($argument)*), false, true)
    };
}
