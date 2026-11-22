//! The proof that the registry checks inspected everything the classifier declared.

use crate::deps::{Bump, DepsRefusal};
use mx_receipt::DependencyFacts;

/// How many pins, base images and pip packages the checks verified, proved equal
/// to how many the classifier declared.
///
/// The classifier's output could be read into a variable, the loop could be fed an
/// empty stream because its producer died (`while ... done < <(python3 ...)`
/// reports the status of the redirection and never of the command inside it),
/// `checked` would stay at zero, and a sealed receipt said "0 pin(s) verified"
/// about a diff that bumped a pin. The declared counts are the other half: a
/// verifier that inspected nothing must not look like one that found nothing to
/// inspect. The only way to a `Checked` is [`Checked::new`], and only a `Checked`
/// becomes [`DependencyFacts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checked {
    pins: u64,
    images: u64,
    pip: u64,
}

impl Checked {
    /// Compares what the checks inspected with what the classifier declared in
    /// `bump`. The refusals come in the order of the bash: pins, then images, then
    /// pip packages.
    pub fn new(bump: &Bump, pins: u64, images: u64, pip: u64) -> Result<Self, DepsRefusal> {
        let declared = |n: usize| n as u64;
        if pins != declared(bump.pins.len()) {
            return Err(DepsRefusal::PinsUnchecked {
                declared: declared(bump.pins.len()),
                checked: pins,
            });
        }
        if images != declared(bump.images.len()) {
            return Err(DepsRefusal::ImagesUnchecked {
                declared: declared(bump.images.len()),
                checked: images,
            });
        }
        if pip != declared(bump.pip.len()) {
            return Err(DepsRefusal::PipUnchecked {
                declared: declared(bump.pip.len()),
                checked: pip,
            });
        }
        Ok(Self { pins, images, pip })
    }

    /// The facts the receipt records: the verdict's files and changed lines, and how
    /// many of each were verified.
    #[must_use]
    pub fn facts(&self, bump: &Bump) -> DependencyFacts {
        DependencyFacts {
            files: bump.files_summary(),
            changed_lines: bump.changed_lines,
            pins_verified: self.pins,
            images_verified: self.images,
            pip_verified: self.pip,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::{Image, Pin, PipPackage};

    fn bump(pins: usize, images: usize, pip: usize) -> Bump {
        Bump {
            files: vec![
                ("Cargo.lock".into(), "cargo-lock".into()),
                ("Dockerfile".into(), "dockerfile".into()),
            ],
            changed_lines: 9,
            pins: (0..pins)
                .map(|i| Pin {
                    action: format!("a/b{i}"),
                    sha: "a".repeat(40),
                    tag: "v1".into(),
                    branch: false,
                })
                .collect(),
            images: (0..images)
                .map(|i| Image {
                    image: format!("i{i}"),
                    tag: "1".into(),
                    digest: format!("sha256:{}", "c".repeat(64)),
                    same_tag: true,
                })
                .collect(),
            pip: (0..pip)
                .map(|i| PipPackage {
                    path: "r.txt".into(),
                    name: format!("p{i}"),
                    version: "1".into(),
                    hashes: vec!["d".repeat(64)],
                })
                .collect(),
        }
    }

    #[test]
    fn counts_that_match_the_declaration_are_proof() {
        let checked = Checked::new(&bump(2, 1, 3), 2, 1, 3).unwrap();
        let facts = checked.facts(&bump(2, 1, 3));
        assert_eq!(
            facts,
            DependencyFacts {
                files: "Cargo.lock (cargo-lock), Dockerfile (dockerfile)".into(),
                changed_lines: 9,
                pins_verified: 2,
                images_verified: 1,
                pip_verified: 3
            }
        );
    }

    #[test]
    fn nothing_declared_and_nothing_checked_is_proof_of_a_bump_with_nothing_to_check() {
        let checked = Checked::new(&bump(0, 0, 0), 0, 0, 0).unwrap();
        assert_eq!(checked.facts(&bump(0, 0, 0)).pins_verified, 0);
    }

    #[test]
    fn a_pin_the_loop_never_reached_refuses_even_when_none_was_wrong() {
        let refused = Checked::new(&bump(2, 0, 0), 0, 0, 0).unwrap_err();
        assert_eq!(refused, DepsRefusal::PinsUnchecked { declared: 2, checked: 0 });
        // bash: die "the classifier declared $declared_pins action pin(s) and $checked
        //   were verified. A receipt is a claim about all of them, so this one is not
        //   written. Nothing here says the bump is bad; it says the check did not run."
        assert_eq!(
            refused.to_string(),
            "the classifier declared 2 action pin(s) and 0\n  were verified. A receipt is a claim about all of them, so this one is not\n  written. Nothing here says the bump is bad; it says the check did not run."
        );
    }

    #[test]
    fn more_checked_than_declared_refuses_too() {
        assert_eq!(
            Checked::new(&bump(1, 0, 0), 2, 0, 0),
            Err(DepsRefusal::PinsUnchecked { declared: 1, checked: 2 })
        );
    }

    #[test]
    fn a_base_image_the_loop_never_reached_refuses_with_the_images_sentence() {
        let refused = Checked::new(&bump(1, 2, 0), 1, 1, 0).unwrap_err();
        assert_eq!(refused, DepsRefusal::ImagesUnchecked { declared: 2, checked: 1 });
        // bash: die "the classifier declared $declared_images base image(s) and
        //   $images_checked were verified. No receipt written: the check did not run."
        assert_eq!(
            refused.to_string(),
            "the classifier declared 2 base image(s) and\n  1 were verified. No receipt written: the check did not run."
        );
    }

    #[test]
    fn a_pip_package_the_loop_never_reached_refuses_with_the_pip_sentence() {
        let refused = Checked::new(&bump(0, 0, 3), 0, 0, 1).unwrap_err();
        assert_eq!(refused, DepsRefusal::PipUnchecked { declared: 3, checked: 1 });
        // bash: die "the classifier declared $declared_pip pip package(s) and
        //   $pip_checked were verified. No receipt written: the check did not run."
        assert_eq!(
            refused.to_string(),
            "the classifier declared 3 pip package(s) and\n  1 were verified. No receipt written: the check did not run."
        );
    }

    #[test]
    fn the_refusals_come_in_the_order_pins_images_pip() {
        assert!(matches!(
            Checked::new(&bump(1, 1, 1), 0, 0, 0),
            Err(DepsRefusal::PinsUnchecked { .. })
        ));
        assert!(matches!(
            Checked::new(&bump(1, 1, 1), 1, 0, 0),
            Err(DepsRefusal::ImagesUnchecked { .. })
        ));
        assert!(matches!(
            Checked::new(&bump(1, 1, 1), 1, 1, 0),
            Err(DepsRefusal::PipUnchecked { .. })
        ));
    }

    #[test]
    fn a_merge_base_that_is_not_a_commit_names_the_first_40_characters() {
        let pr = crate::PrNumber::parse("7").unwrap();
        let refused = DepsRefusal::NoMergeBase { pr, got: "x".repeat(40) };
        assert_eq!(
            refused.to_string(),
            format!("GitHub gave no merge base for #7: '{}'", "x".repeat(40))
        );
    }

    #[test]
    fn the_action_refusal_names_the_action() {
        assert_eq!(
            DepsRefusal::ActionNotOwnerRepo { action: "checkout".into() }.to_string(),
            "the action 'checkout' is not owner/repo[/dir] with plain segments, so it is not put in an API URL"
        );
    }
}
