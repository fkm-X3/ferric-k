//! Line-ending translation shared by [`ferric_api::TextSink`] implementors.

/// Expands bare `\n` to `\r\n`; every other byte passes through untouched.
pub(crate) fn expand_lf_to_crlf(s: &str, mut emit: impl FnMut(u8)) {
    for byte in s.bytes() {
        if byte == b'\n' {
            emit(b'\r');
        }
        emit(byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lone_lf_expands_to_crlf() {
        let mut out = Vec::new();
        expand_lf_to_crlf("a\nb\r\nc", |byte| out.push(byte));
        assert_eq!(out, b"a\r\nb\r\r\nc");
    }

    #[test]
    fn empty_and_untouched_strings_pass_through() {
        let mut empty = Vec::new();
        expand_lf_to_crlf("", |byte| empty.push(byte));
        assert!(empty.is_empty());

        let mut plain = Vec::new();
        expand_lf_to_crlf("plain \t text", |byte| plain.push(byte));
        assert_eq!(plain, b"plain \t text");
    }
}
