//! Parses a finished command line into a [`Command`] the kernel dispatches.

/// A parsed command line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command<'a> {
    /// A blank (whitespace-only) line; the shell just prints a fresh prompt.
    Empty,
    /// `help`: list the built-in commands.
    Help,
    /// `clear`: blank the screen and re-home the cursor.
    Clear,
    /// `echo <args>`: print everything after the command name.
    Echo(&'a [char]),
    /// `uptime`: print seconds since boot.
    Uptime,
    /// `arch`: print the CPU architecture.
    Arch,
    /// `panic`: crash the kernel on purpose (manual test hook).
    Panic,
    /// `halt`: power the machine off in the emulator.
    Halt,
    /// Anything else; carries the command name as typed (without arguments).
    Unknown(&'a [char]),
}

/// Parses `line`: a leading ASCII token names the command (case-insensitive);
/// everything after the first whitespace run is the `echo` argument.
pub fn parse_command(line: &[char]) -> Command<'_> {
    let line = trim(line);
    if line.is_empty() {
        return Command::Empty;
    }
    let head_len = line
        .iter()
        .position(|&c| c.is_whitespace())
        .unwrap_or(line.len());
    let (name, rest) = line.split_at(head_len);
    let rest = trim(rest);
    if is_name(name, "help") {
        Command::Help
    } else if is_name(name, "clear") {
        Command::Clear
    } else if is_name(name, "echo") {
        Command::Echo(rest)
    } else if is_name(name, "uptime") {
        Command::Uptime
    } else if is_name(name, "arch") {
        Command::Arch
    } else if is_name(name, "panic") {
        Command::Panic
    } else if is_name(name, "halt") {
        Command::Halt
    } else {
        Command::Unknown(name)
    }
}

fn is_name(token: &[char], name: &str) -> bool {
    token.len() == name.len()
        && token
            .iter()
            .zip(name.chars())
            .all(|(&c, n)| c.to_ascii_lowercase() == n)
}

fn trim(mut slice: &[char]) -> &[char] {
    while let Some((&c, tail)) = slice.split_first() {
        if c.is_whitespace() {
            slice = tail;
        } else {
            break;
        }
    }
    while let Some((&c, head)) = slice.split_last() {
        if c.is_whitespace() {
            slice = head;
        } else {
            break;
        }
    }
    slice
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn empty_and_blank_lines_parse_to_empty() {
        assert_eq!(parse_command(&[]), Command::Empty);
        assert_eq!(parse_command(&line("   ")), Command::Empty);
        assert_eq!(parse_command(&line("\t ")), Command::Empty);
    }

    #[test]
    fn names_are_case_insensitive() {
        for name in ["HELP", "Help", "hElP", "help"] {
            assert_eq!(parse_command(&line(name)), Command::Help);
        }
        assert_eq!(parse_command(&line("HALT")), Command::Halt);
        assert_eq!(parse_command(&line("UPTIME")), Command::Uptime);
        assert_eq!(parse_command(&line("ECHO")), Command::Echo(&[]));
    }

    #[test]
    fn keyword_commands_ignore_trailing_arguments() {
        assert_eq!(parse_command(&line("help me")), Command::Help);
        assert_eq!(parse_command(&line("clear all")), Command::Clear);
        assert_eq!(parse_command(&line("halt now")), Command::Halt);
    }

    #[test]
    fn echo_carries_the_trimmed_remainder() {
        assert_eq!(parse_command(&line("echo hi")), Command::Echo(&['h', 'i']));
        assert_eq!(
            parse_command(&line("  echo   hello world  ")),
            Command::Echo(&['h', 'e', 'l', 'l', 'o', ' ', 'w', 'o', 'r', 'l', 'd'])
        );
        assert_eq!(parse_command(&line("echo")), Command::Echo(&[]));
    }

    #[test]
    fn unknown_commands_carry_the_name_without_arguments() {
        assert_eq!(
            parse_command(&line("xyz")),
            Command::Unknown(&['x', 'y', 'z'])
        );
        assert_eq!(
            parse_command(&line("echoo hi")),
            Command::Unknown(&['e', 'c', 'h', 'o', 'o'])
        );
    }
}
