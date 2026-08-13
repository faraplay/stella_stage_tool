use std::fmt::Display;

#[derive(Debug)]
pub enum CryptError {
    IoError(std::io::Error),
    FileSizeError(String),
}

impl Display for CryptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptError::IoError(error) => error.fmt(f),
            CryptError::FileSizeError(message) => message.fmt(f),
        }
    }
}

impl std::error::Error for CryptError {}

impl From<std::io::Error> for CryptError {
    fn from(value: std::io::Error) -> Self {
        CryptError::IoError(value)
    }
}

impl CryptError {
    pub(crate) fn file_size_error(message: impl ToString) -> Self {
        CryptError::FileSizeError(message.to_string())
    }
}
