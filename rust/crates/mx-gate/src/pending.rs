//! Stand-ins for the suite and sandbox adapters of the `p3a` lane, which are not in this build
//! yet. Each answer is a refusal that names the missing piece, so a verify or a partial-receipt
//! merge fails closed instead of reading an adapter that does not exist.

use mx_core::NonEmpty;
use mx_sandbox::{
    ContainedTree, ContainerExit, ContainerSpec, Escape, ExtractedTree, Mutated, Sandbox,
    SandboxError, SedExpression,
};
use mx_suite::{
    CoverageFailure, CoverageRow, MutateGlob, PickRefusal, Picked, ProdGlobs, Suite, SuiteError,
    SuiteHost, SuiteName, SuiteSelector,
};
use std::path::{Path, PathBuf};

/// The adapter that is not built yet.
pub struct Pending;

fn missing() -> SuiteError {
    SuiteError::Bash {
        reason: "the Rust suite host is not in this build; use bin/maintainer-merge".into(),
    }
}

impl SuiteHost for Pending {
    fn dir(&self) -> Result<PathBuf, SuiteError> {
        Err(missing())
    }

    fn names(&self) -> Result<Vec<SuiteName>, SuiteError> {
        Err(missing())
    }

    fn load(&self, _name: &SuiteName) -> Result<Box<dyn Suite>, SuiteError> {
        Err(missing())
    }
}

impl SuiteSelector for Pending {
    fn suites_cover_production(
        &self,
        _globs: &ProdGlobs,
        _names: &[String],
        _changed: &NonEmpty<String>,
    ) -> Result<(), CoverageFailure> {
        Err(CoverageFailure::Suite(missing()))
    }

    fn pick_suite(
        &self,
        _globs: &ProdGlobs,
        _named: Option<&SuiteName>,
        _changed: &NonEmpty<String>,
    ) -> Result<Picked, PickRefusal> {
        Err(PickRefusal::Suite(missing()))
    }

    fn coverage_map(&self, _changed: &[String]) -> Result<Vec<CoverageRow>, SuiteError> {
        Err(missing())
    }
}

fn no_sandbox() -> SandboxError {
    SandboxError::Run {
        reason: "the Rust sandbox is not in this build; use bin/maintainer-merge".into(),
    }
}

impl Sandbox for Pending {
    fn extract_tree(
        &self,
        _repo: &Path,
        _rev: &str,
        _dest: &Path,
    ) -> Result<ExtractedTree, SandboxError> {
        Err(no_sandbox())
    }

    fn tree_escapes(&self, _tree: &ExtractedTree) -> Result<Vec<Escape>, SandboxError> {
        Err(no_sandbox())
    }

    fn apply_mutation(
        &self,
        _tree: &ContainedTree,
        _globs: &NonEmpty<MutateGlob>,
        _expression: &SedExpression,
    ) -> Result<Mutated, SandboxError> {
        Err(no_sandbox())
    }

    fn scratch_dir(&self, _parent: &Path) -> Result<PathBuf, SandboxError> {
        Err(no_sandbox())
    }

    fn seed_index(&self, _tree: &ContainedTree) -> Result<(), SandboxError> {
        Err(no_sandbox())
    }

    fn run_container(
        &self,
        _spec: &ContainerSpec,
        _log: &Path,
    ) -> Result<ContainerExit, SandboxError> {
        Err(no_sandbox())
    }
}
