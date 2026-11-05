pub mod commands;
pub mod convert;
pub mod frame;

#[cfg(test)]
pub(crate) mod test_util {
    /// Parse space-separated hex bytes, e.g. "FD FC 01".
    pub fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).expect("valid hex byte"))
            .collect()
    }
}
