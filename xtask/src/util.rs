//! Shared helpers: errors, process execution, workspace paths.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

/// Return early with a formatted error.
macro_rules! bail {
    ($($arg:tt)*) => {
        return Err(format!($($arg)*).into())
    };
}
pub(crate) use bail;

/// Workspace root (parent of `xtask/`).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

/// Run `program args...` in the workspace root, returning trimmed stdout.
pub fn run(program: &str, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    output(&mut cmd)
}

/// Run `cmd` in the workspace root, returning trimmed stdout; a non-zero exit
/// becomes an error carrying stderr.
pub fn output(cmd: &mut Command) -> Result<String> {
    let out = cmd
        .current_dir(root())
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
        .map_err(|e| format!("cannot run {:?}: {e}", cmd.get_program()))?;
    if !out.status.success() {
        bail!(
            "{:?} {:?} failed ({}):\n{}",
            cmd.get_program(),
            cmd.get_args().collect::<Vec<_>>(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}

/// Run `cmd` in the workspace root with inherited stdio (for builds).
pub fn status(cmd: &mut Command) -> Result<()> {
    let st = cmd
        .current_dir(root())
        .status()
        .map_err(|e| format!("cannot run {:?}: {e}", cmd.get_program()))?;
    if !st.success() {
        bail!("{:?} failed ({st})", cmd.get_program());
    }
    Ok(())
}

/// `cargo` executable driving this xtask.
pub fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

/// Stable `X.Y.Z` version (optionally `v`-prefixed) as a comparable triple.
pub fn parse_semver(value: &str) -> Result<(u64, u64, u64)> {
    let v = value.strip_prefix('v').unwrap_or(value);
    let parts: Vec<&str> = v.split('.').collect();
    let valid = parts.len() == 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
        });
    if !valid {
        bail!("expected a stable version X.Y.Z (optional v prefix), got {value:?}");
    }
    Ok((parts[0].parse()?, parts[1].parse()?, parts[2].parse()?))
}

/// The workspace version from `[workspace.package]` in the root Cargo.toml.
pub fn workspace_version() -> Result<String> {
    let text = std::fs::read_to_string(root().join("Cargo.toml"))?;
    let doc: toml_edit::DocumentMut = text.parse()?;
    doc["workspace"]["package"]["version"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "Cargo.toml has no [workspace.package] version".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_accepts_stable_versions_only() {
        assert_eq!(parse_semver("v1.2.3").unwrap(), (1, 2, 3));
        assert_eq!(parse_semver("0.10.0").unwrap(), (0, 10, 0));
        for bad in ["1.2", "1.2.3-rc1", "01.2.3", "v1.2.3.4", "", "x.y.z"] {
            assert!(parse_semver(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn reads_workspace_version() {
        parse_semver(&workspace_version().unwrap()).unwrap();
    }
}
