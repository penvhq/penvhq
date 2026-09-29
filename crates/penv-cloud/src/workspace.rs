//! The workspace a machine login is for. penv.cloud takes only its id as the
//! OIDC audience and in the signed AWS header; a slug is refused here, before
//! any request, and never looked up.

use crate::error::{CloudError, Result};

/// A workspace id: a UUID, held lowercase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceId(String);

impl WorkspaceId {
    /// The id, or the refusal naming what was given instead.
    pub fn parse(given: &str) -> Result<WorkspaceId> {
        if is_uuid(given) {
            Ok(WorkspaceId(given.to_ascii_lowercase()))
        } else {
            Err(CloudError::NotWorkspaceId(given.to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `8-4-4-4-12` hex digits, either case.
pub fn is_uuid(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uuid_is_a_workspace_id_in_either_case_and_held_lowercase() {
        let id = WorkspaceId::parse("3F2504E0-4F89-11D3-9A0C-0305E82C3301").unwrap();
        assert_eq!(id.as_str(), "3f2504e0-4f89-11d3-9a0c-0305e82c3301");
    }

    #[test]
    fn a_slug_or_anything_near_a_uuid_is_refused_and_named() {
        for given in [
            "acme",
            "",
            "3f2504e0-4f89-11d3-9a0c-0305e82c330",
            "3f2504e0-4f89-11d3-9a0c-0305e82c33011",
            "3f2504e04f8911d39a0c0305e82c3301abcd",
            "3f2504e0-4f89-11d3-9a0c-0305e82c330g",
            "3f2504e0_4f89_11d3_9a0c_0305e82c3301",
            " 3f2504e0-4f89-11d3-9a0c-0305e82c3301",
        ] {
            assert_eq!(
                WorkspaceId::parse(given),
                Err(CloudError::NotWorkspaceId(given.to_string())),
                "{given:?}"
            );
        }
    }
}
