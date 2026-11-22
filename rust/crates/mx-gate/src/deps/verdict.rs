//! The classifier's answer, as types.

use serde::Deserialize;
use serde_json::{Map, Value};
use thiserror::Error;

/// Why the classifier's output is not a [`Verdict`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VerdictError {
    /// Not JSON, or not an object.
    #[error("the classifier printed something that is not a JSON object: {reason}")]
    NotJson { reason: String },
    /// A field is missing, has the wrong type, or is one the gate does not know.
    /// The classifier and the gate are versioned together, so an unknown field is
    /// a gate that no longer understands its helper.
    #[error("the classifier's verdict has the wrong shape: {reason}")]
    Shape { reason: String },
}

/// One action pin the diff adds: `uses: <action>@<sha>  # <tag>`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub action: String,
    pub sha: String,
    pub tag: String,
    /// The comment said `(branch)`: the claim is that the SHA is on that branch.
    pub branch: bool,
}

/// One base image pinned by digest.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub image: String,
    pub tag: String,
    pub digest: String,
    /// The tag also appeared on a removed line.
    pub same_tag: bool,
}

/// One pip requirement whose pin moved.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipPackage {
    pub path: String,
    pub name: String,
    pub version: String,
    pub hashes: Vec<String>,
}

/// A diff that is nothing but a dependency bump. Everything it lists still has to
/// be checked against its registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bump {
    /// Every changed path and the kind the classifier gave it, in the classifier's
    /// order (`cargo-lock`, `pip-requirements`, `workflow`, `dockerfile`, ...).
    pub files: Vec<(String, String)>,
    pub changed_lines: u64,
    pub pins: Vec<Pin>,
    pub images: Vec<Image>,
    pub pip: Vec<PipPackage>,
}

impl Bump {
    /// `", ".join(f"{k} ({v})" for k, v in files.items())`: the `files` field of the receipt.
    #[must_use]
    pub fn files_summary(&self) -> String {
        self.files
            .iter()
            .map(|(path, kind)| format!("{path} ({kind})"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The changed requirements files, which `verify-deps` reads whole at the
    /// merge base and at the head.
    #[must_use]
    pub fn pip_files(&self) -> Vec<&str> {
        self.files
            .iter()
            .filter(|(_, kind)| kind == "pip-requirements")
            .map(|(path, _)| path.as_str())
            .collect()
    }
}

/// Where in the file a refusal points: a line number, or the text of the line.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum LineRef {
    Number(u64),
    Text(String),
}

/// The classifier refused: the diff is not a pure bump.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Refused {
    pub reason: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<LineRef>,
}

/// What `classify_dependency_bump.py` printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Exit 0.
    Bump(Bump),
    /// Exit 1.
    Refused(Refused),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBump {
    #[allow(dead_code, reason = "read to choose the variant")]
    verdict: String,
    files: Map<String, Value>,
    changed_lines: u64,
    pins: Vec<Pin>,
    images: Vec<Image>,
    pip: Vec<PipPackage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRefused {
    #[allow(dead_code, reason = "read to choose the variant")]
    verdict: String,
    reason: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    line: Option<LineRef>,
}

impl Verdict {
    /// Reads the helper's standard output. The `verdict` field picks the variant;
    /// any other value, a missing field, a field of the wrong type or an unknown
    /// field is an error. The exit status is the caller's to cross-check: 0 with a
    /// `refused` verdict, or 1 with a `dependency-bump` one, is a helper that
    /// contradicts itself.
    pub fn parse(text: &str) -> Result<Self, VerdictError> {
        let shape = |error: serde_json::Error| VerdictError::Shape { reason: error.to_string() };
        let value: Value = serde_json::from_str(text)
            .map_err(|error| VerdictError::NotJson { reason: error.to_string() })?;
        let Value::Object(top) = &value else {
            return Err(VerdictError::NotJson { reason: "the top level is not an object".into() });
        };
        match top.get("verdict").and_then(Value::as_str) {
            Some("dependency-bump") => {
                let raw: RawBump = serde_json::from_value(value).map_err(shape)?;
                let mut files = Vec::with_capacity(raw.files.len());
                for (path, kind) in raw.files {
                    let Value::String(kind) = kind else {
                        return Err(VerdictError::Shape {
                            reason: format!("the kind of {path} is not text"),
                        });
                    };
                    files.push((path, kind));
                }
                Ok(Self::Bump(Bump {
                    files,
                    changed_lines: raw.changed_lines,
                    pins: raw.pins,
                    images: raw.images,
                    pip: raw.pip,
                }))
            }
            Some("refused") => {
                let raw: RawRefused = serde_json::from_value(value).map_err(shape)?;
                Ok(Self::Refused(Refused { reason: raw.reason, path: raw.path, line: raw.line }))
            }
            _ => Err(VerdictError::Shape {
                reason: "the verdict field is missing or is neither dependency-bump nor refused"
                    .into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The five samples are the real output of scripts/classify_dependency_bump.py
    // on a hand-written diff of each kind, captured on 2026-11-22.
    const CARGO: &str = r#"{
  "verdict": "dependency-bump",
  "files": {
    "Cargo.lock": "cargo-lock"
  },
  "changed_lines": 4,
  "pins": [],
  "images": [],
  "pip": []
}"#;

    const WORKFLOW: &str = r#"{
  "verdict": "dependency-bump",
  "files": {
    ".github/workflows/ci.yml": "workflow"
  },
  "changed_lines": 2,
  "pins": [
    {
      "action": "actions/checkout",
      "sha": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "tag": "v4.2.2",
      "branch": false
    }
  ],
  "images": [],
  "pip": []
}"#;

    const PIP: &str = r#"{
  "verdict": "dependency-bump",
  "files": {
    "requirements.txt": "pip-requirements"
  },
  "changed_lines": 4,
  "pins": [],
  "images": [],
  "pip": [
    {
      "path": "requirements.txt",
      "name": "PyYAML",
      "version": "6.0.3",
      "hashes": [
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
      ]
    }
  ]
}"#;

    const DOCKERFILE: &str = r#"{
  "verdict": "dependency-bump",
  "files": {
    "Dockerfile": "dockerfile"
  },
  "changed_lines": 2,
  "pins": [],
  "images": [
    {
      "image": "docker.io/library/rust",
      "tag": "1-bookworm",
      "digest": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "same_tag": true
    }
  ],
  "pip": []
}"#;

    const REFUSED: &str = r#"{
  "verdict": "refused",
  "reason": "src/lib.rs is not dependency metadata, so this is not a pure bump",
  "path": "src/lib.rs"
}"#;

    fn bump(text: &str) -> Bump {
        match Verdict::parse(text).unwrap() {
            Verdict::Bump(bump) => bump,
            other @ Verdict::Refused(_) => panic!("expected a bump, got {other:?}"),
        }
    }

    #[test]
    fn a_lockfile_bump_has_one_file_and_nothing_to_verify_against_a_registry() {
        let b = bump(CARGO);
        assert_eq!(b.files, [("Cargo.lock".to_string(), "cargo-lock".to_string())]);
        assert_eq!(b.changed_lines, 4);
        assert!(b.pins.is_empty() && b.images.is_empty() && b.pip.is_empty());
    }

    #[test]
    fn a_workflow_bump_lists_the_pin_it_adds() {
        let b = bump(WORKFLOW);
        assert_eq!(
            b.pins,
            [Pin {
                action: "actions/checkout".into(),
                sha: "b".repeat(40),
                tag: "v4.2.2".into(),
                branch: false
            }]
        );
    }

    #[test]
    fn a_pip_bump_lists_the_package_with_its_hashes_and_path() {
        let b = bump(PIP);
        assert_eq!(b.pip.len(), 1);
        assert_eq!(b.pip[0].name, "PyYAML");
        assert_eq!(b.pip[0].version, "6.0.3");
        assert_eq!(b.pip[0].path, "requirements.txt");
        assert_eq!(b.pip[0].hashes, ["b".repeat(64)]);
    }

    #[test]
    fn a_dockerfile_bump_lists_the_image_with_its_digest() {
        let b = bump(DOCKERFILE);
        assert_eq!(b.images[0].image, "docker.io/library/rust");
        assert_eq!(b.images[0].tag, "1-bookworm");
        assert_eq!(b.images[0].digest, format!("sha256:{}", "b".repeat(64)));
        assert!(b.images[0].same_tag);
    }

    #[test]
    fn a_refusal_names_the_reason_and_the_file() {
        let Verdict::Refused(r) = Verdict::parse(REFUSED).unwrap() else { panic!("not refused") };
        assert_eq!(r.reason, "src/lib.rs is not dependency metadata, so this is not a pure bump");
        assert_eq!(r.path.as_deref(), Some("src/lib.rs"));
        assert_eq!(r.line, None);
    }

    #[test]
    fn a_refusal_line_is_a_number_or_the_text_of_the_line() {
        let number = r#"{"verdict":"refused","reason":"r","path":"p","line":12}"#;
        let text = r#"{"verdict":"refused","reason":"r","path":"p","line":"PyYAML==6.0.3"}"#;
        let Verdict::Refused(n) = Verdict::parse(number).unwrap() else { panic!() };
        let Verdict::Refused(t) = Verdict::parse(text).unwrap() else { panic!() };
        assert_eq!(n.line, Some(LineRef::Number(12)));
        assert_eq!(t.line, Some(LineRef::Text("PyYAML==6.0.3".into())));
    }

    #[test]
    fn files_keep_the_classifiers_order() {
        let text = r#"{"verdict":"dependency-bump","files":{"z.lock":"cargo-lock","a.txt":"pip-requirements","m.yml":"workflow"},"changed_lines":1,"pins":[],"images":[],"pip":[]}"#;
        let b = bump(text);
        let paths: Vec<&str> = b.files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, ["z.lock", "a.txt", "m.yml"]);
    }

    #[test]
    fn the_files_summary_is_the_receipts_files_field() {
        // bash: print(", ".join(f"{k} ({v})" for k, v in d["files"].items()))
        let text = r#"{"verdict":"dependency-bump","files":{"Cargo.lock":"cargo-lock","requirements.txt":"pip-requirements"},"changed_lines":1,"pins":[],"images":[],"pip":[]}"#;
        assert_eq!(
            bump(text).files_summary(),
            "Cargo.lock (cargo-lock), requirements.txt (pip-requirements)"
        );
        assert_eq!(bump(CARGO).files_summary(), "Cargo.lock (cargo-lock)");
    }

    #[test]
    fn the_pip_files_are_the_ones_of_kind_pip_requirements() {
        let text = r#"{"verdict":"dependency-bump","files":{"Cargo.lock":"cargo-lock","requirements.txt":"pip-requirements","docs/req.txt":"pip-requirements"},"changed_lines":1,"pins":[],"images":[],"pip":[]}"#;
        assert_eq!(bump(text).pip_files(), ["requirements.txt", "docs/req.txt"]);
        assert!(bump(CARGO).pip_files().is_empty());
    }

    #[test]
    fn text_that_is_not_a_json_object_is_refused() {
        for text in ["", "not json", "[]", "null", "\"x\"", "42"] {
            assert!(
                matches!(
                    Verdict::parse(text),
                    Err(VerdictError::NotJson { .. } | VerdictError::Shape { .. })
                ),
                "{text:?}"
            );
        }
        assert!(matches!(Verdict::parse("not json"), Err(VerdictError::NotJson { .. })));
    }

    #[test]
    fn an_unknown_verdict_word_is_refused() {
        let text = r#"{"verdict":"maybe","reason":"x"}"#;
        assert!(matches!(Verdict::parse(text), Err(VerdictError::Shape { .. })));
        assert!(matches!(Verdict::parse("{}"), Err(VerdictError::Shape { .. })));
    }

    #[test]
    fn a_missing_field_a_wrong_type_or_an_unknown_field_is_refused() {
        let missing_pins =
            r#"{"verdict":"dependency-bump","files":{},"changed_lines":1,"images":[],"pip":[]}"#;
        let wrong_type = r#"{"verdict":"dependency-bump","files":{},"changed_lines":"4","pins":[],"images":[],"pip":[]}"#;
        let unknown = r#"{"verdict":"dependency-bump","files":{},"changed_lines":1,"pins":[],"images":[],"pip":[],"extra":1}"#;
        let bad_pin = r#"{"verdict":"dependency-bump","files":{},"changed_lines":1,"pins":[{"action":"a/b","sha":"x","tag":"t"}],"images":[],"pip":[]}"#;
        let refused_extra = r#"{"verdict":"refused","reason":"r","surprise":true}"#;
        for text in [missing_pins, wrong_type, unknown, bad_pin, refused_extra] {
            assert!(matches!(Verdict::parse(text), Err(VerdictError::Shape { .. })), "{text}");
        }
    }

    #[test]
    fn a_file_kind_that_is_not_text_is_refused() {
        let text = r#"{"verdict":"dependency-bump","files":{"a":1},"changed_lines":1,"pins":[],"images":[],"pip":[]}"#;
        assert!(matches!(Verdict::parse(text), Err(VerdictError::Shape { .. })));
    }

    #[test]
    fn the_refusals_name_what_went_wrong() {
        let not_json = Verdict::parse("nope").unwrap_err().to_string();
        assert!(
            not_json.starts_with("the classifier printed something that is not a JSON object: "),
            "{not_json}"
        );
        let shape = Verdict::parse("{}").unwrap_err().to_string();
        assert!(shape.starts_with("the classifier's verdict has the wrong shape: "), "{shape}");
    }
}
