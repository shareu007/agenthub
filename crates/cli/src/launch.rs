//! Resolve from the caller's environment before switching to the native project.
use anyhow::{bail, Context, Result};
use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
};

pub fn validate_cwd(cwd: Option<&Path>) -> Result<()> {
    if let Some(cwd) = cwd {
        if !cwd.is_dir() {
            bail!("session working directory does not exist or is not a directory: {}; if the project moved, use ah resume <agent:id> --cwd /new/project --exec", cwd.display());
        }
    }
    Ok(())
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Freeze relative/empty PATH entries against the caller, not the session cwd.
/// This also keeps env-based shebangs (e.g. /usr/bin/env node) working after chdir.
pub fn absolute_path() -> Result<OsString> {
    let cwd = env::current_dir().context("read launch working directory")?;
    let entries = env::var_os("PATH").unwrap_or_default();
    env::join_paths(
        env::split_paths(&entries).map(|p| if p.is_absolute() { p } else { cwd.join(p) }),
    )
    .context("resolve native PATH")
}

pub fn resolve(program: &str, path: &OsStr) -> Result<PathBuf> {
    let caller = env::current_dir().context("read launch working directory")?;
    let program_path = Path::new(program);
    if program_path.components().count() > 1 || program_path.is_absolute() {
        let absolute = if program_path.is_absolute() {
            program_path.into()
        } else {
            caller.join(program_path)
        };
        if !is_executable(&absolute) {
            bail!(
                "native executable is missing or not executable: {}",
                absolute.display()
            );
        }
        return Ok(absolute);
    }
    let mut not_executable = None;
    for directory in env::split_paths(path) {
        let candidate = if directory.is_absolute() {
            directory.join(program)
        } else {
            caller.join(directory).join(program)
        };
        if is_executable(&candidate) {
            return Ok(candidate);
        }
        if candidate.is_file() {
            not_executable = Some(candidate);
        }
    }
    if let Some(candidate) = not_executable {
        bail!(
            "{program} was found at {} but is not executable; check its execute permissions",
            candidate.display()
        );
    }
    bail!("native executable '{program}' was not found in the PATH inherited by ah; shell aliases/functions are not executable files. Run 'type -a {program}' in the same terminal and add its real executable directory to PATH")
}

pub fn installed_agents() -> Result<std::collections::HashSet<agenthub_core::Agent>> {
    let path = absolute_path()?;
    Ok(agenthub_core::Agent::ALL
        .into_iter()
        .filter(|agent| resolve(agent.as_str(), &path).is_ok())
        .collect())
}

pub fn validate_ready(plan: &agenthub_core::ResumeCommand) -> Result<()> {
    validate_cwd(plan.cwd.as_deref())?;
    resolve(&plan.program, &absolute_path()?)?;
    Ok(())
}
