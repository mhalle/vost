use std::fmt;

/// CLI error with a user-visible message and exit code.
pub struct CliError {
    pub message: String,
    pub exit_code: i32,
}

impl CliError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            exit_code: 1,
        }
    }

    pub fn with_code(msg: impl Into<String>, code: i32) -> Self {
        Self {
            message: msg.into(),
            exit_code: code,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl fmt::Debug for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CliError({}): {}", self.exit_code, self.message)
    }
}

impl From<crate::Error> for CliError {
    fn from(e: crate::Error) -> Self {
        match &e {
            crate::Error::StaleSnapshot(_) => {
                CliError::new("Branch modified concurrently — retry")
            }
            crate::Error::KeyNotFound(key) => {
                CliError::new(format!("Not found: {}", key))
            }
            crate::Error::KeyExists(key) => {
                CliError::new(format!("Already exists: {}", key))
            }
            crate::Error::NotFound(path) => {
                CliError::new(format!("Not found: {}", path))
            }
            crate::Error::IsADirectory(path) => {
                CliError::new(format!("{} is a directory", path))
            }
            crate::Error::NotADirectory(path) => {
                CliError::new(format!("{} is not a directory", path))
            }
            crate::Error::Permission(msg) => {
                CliError::new(format!("Permission denied: {}", msg))
            }
            crate::Error::InvalidPath(msg) => {
                CliError::new(format!("Invalid path: {}", msg))
            }
            crate::Error::InvalidRefName(name) => {
                CliError::new(format!("Invalid ref name: {}", name))
            }
            crate::Error::InvalidHash(hash) => {
                CliError::new(format!("Invalid hash: {}", hash))
            }
            crate::Error::BatchClosed => {
                CliError::new("Batch already closed")
            }
            _ => CliError::new(e.to_string()),
        }
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::new(e.to_string())
    }
}
