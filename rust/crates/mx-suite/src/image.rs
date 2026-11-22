//! The container image a suite runs in.

use thiserror::Error;

/// Why a string is not an image reference.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ImageRefError {
    /// Nothing was printed.
    #[error("the suite names no container image")]
    Empty,
    /// A space, tab, newline or other control character.
    #[error("a container image reference has no whitespace or control character")]
    Whitespace,
    /// It starts with `-`, so the runtime would read it as one of its own options.
    #[error(
        "a container image reference cannot start with '-': the runtime would read it as an option"
    )]
    LooksLikeOption,
}

/// The output of `suite_image`, for example `docker.io/library/rust:1-slim`.
///
/// The bash gate puts it on the `podman run` line between `-w /repo` and
/// `sh -c`, as one argument. A value that starts with `-` would be an option
/// there, so the type refuses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef(String);

impl ImageRef {
    /// Checks a reference.
    pub fn parse(text: &str) -> Result<Self, ImageRefError> {
        if text.is_empty() {
            return Err(ImageRefError::Empty);
        }
        if text.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(ImageRefError::Whitespace);
        }
        if text.starts_with('-') {
            return Err(ImageRefError::LooksLikeOption);
        }
        Ok(Self(text.to_string()))
    }

    /// The reference as one argument.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_images_of_the_shipped_suites_are_accepted() {
        for image in [
            "docker.io/library/rust:1-slim",
            "docker.io/library/python:3.12",
            "docker.io/library/python:3.12-slim",
            "localhost/magent-node:1@sha256:0123456789abcdef",
        ] {
            assert_eq!(ImageRef::parse(image).unwrap().as_str(), image);
        }
    }

    #[test]
    fn an_empty_image_is_refused() {
        assert_eq!(ImageRef::parse(""), Err(ImageRefError::Empty));
    }

    #[test]
    fn whitespace_or_a_control_character_is_refused() {
        for image in ["rust 1", "rust\t1", "rust\n", "rust\u{7}"] {
            assert_eq!(ImageRef::parse(image), Err(ImageRefError::Whitespace), "{image:?}");
        }
    }

    #[test]
    fn a_leading_hyphen_is_refused() {
        assert_eq!(ImageRef::parse("--privileged"), Err(ImageRefError::LooksLikeOption));
        assert_eq!(ImageRef::parse("-v"), Err(ImageRefError::LooksLikeOption));
    }
}
