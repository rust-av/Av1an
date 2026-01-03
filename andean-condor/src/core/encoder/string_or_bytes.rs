use std::fmt::{Debug, Display};

#[derive(Clone)]
pub enum StringOrBytes {
    String(String),
    Bytes(Vec<u8>),
}

impl Debug for StringOrBytes {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String(s) => {
                if f.alternate() {
                    f.write_str(&textwrap::indent(s, "        "))?; // 8 spaces
                } else {
                    f.write_str(s)?;
                }
            },
            Self::Bytes(b) => write!(f, "raw bytes: {b:?}")?,
        }

        Ok(())
    }
}

impl Display for StringOrBytes {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String(s) => f.write_str(s),
            Self::Bytes(b) => write!(f, "bytes: {:#?}", b),
        }
    }
}

impl From<Vec<u8>> for StringOrBytes {
    #[inline]
    fn from(bytes: Vec<u8>) -> Self {
        #[expect(
            clippy::option_if_let_else,
            reason = "https://github.com/rust-lang/rust-clippy/issues/15142"
        )]
        if let Ok(res) = simdutf8::basic::from_utf8(&bytes) {
            Self::String(res.to_string())
        } else {
            Self::Bytes(bytes)
        }
    }
}

impl From<String> for StringOrBytes {
    #[inline]
    fn from(s: String) -> Self {
        Self::String(s)
    }
}
