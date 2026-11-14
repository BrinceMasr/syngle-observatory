//! The one error a verb can end in: a fixed code (the contract's list), a message for a person, and the query it refused.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovError {
    pub code: &'static str,
    pub message: String,
    /// The refused query as a JSON object, echoed in the refusal.
    pub query: Option<String>,
}

pub type Res<T> = Result<T, GovError>;

impl GovError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), query: None }
    }

    pub fn with_query(mut self, query: String) -> Self {
        self.query = Some(query);
        self
    }

    pub fn refresh(why: impl fmt::Display) -> Self {
        Self::new("refresh-failed", why.to_string())
    }

    pub fn bad_source(why: impl fmt::Display) -> Self {
        Self::new("bad-source", why.to_string())
    }

    pub fn damaged(why: impl fmt::Display) -> Self {
        Self::new("no-data", format!("the index cannot be read ({why}); run syngle-map gov refresh again"))
    }

    pub fn no_data() -> Self {
        Self::new(
            "no-data",
            "nothing is indexed yet: run syngle-map gov refresh --source abr,asic,asicBn (opt-in downloads) or --from <folder>",
        )
    }
}

impl fmt::Display for GovError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for GovError {}

/// `io::Result` into a verb's result: a read that fails means the snapshot is damaged.
pub trait OrDamaged<T> {
    fn damaged(self) -> Res<T>;
}

impl<T> OrDamaged<T> for std::io::Result<T> {
    fn damaged(self) -> Res<T> {
        self.map_err(GovError::damaged)
    }
}
