//! `random()`, docs/Functions.md. `penv run` generates the value; this reads the call.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RandomKind {
    Alnum,
    Hex,
    Base64Url,
    Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandomSpec {
    pub kind: RandomKind,
    pub length: Option<usize>,
}

/// `None` when the text is not a `random()` call.
pub fn spec(text: &str) -> Result<Option<RandomSpec>, String> {
    let text = text.trim();
    if text.starts_with("random(") && text.ends_with(')') {
        Err("random() is not read yet".into())
    } else {
        Ok(None)
    }
}
