//! `release create|notes` and `changelog` (port of `scripts/release.py`).

use std::path::PathBuf;
use std::process::Command;

use crate::util::{Result, bail, cargo, output, parse_semver, root, run, workspace_version};

const RELEASE_FILES: [&str; 3] = ["CHANGELOG.md", "Cargo.toml", "Cargo.lock"];

fn git_cliff(args: &[&str]) -> Result<String> {
    let program = std::env::var("GIT_CLIFF").unwrap_or_else(|_| "git-cliff".into());
    let mut cmd = Command::new(&program);
    cmd.args(["--config", "cliff.toml"]).args(args);
    output(&mut cmd).map_err(|e| {
        format!("{e}\n(install with `cargo install git-cliff --locked` or set GIT_CLIFF)").into()
    })
}

/// git-cliff output normalised to end with exactly one newline.
fn render_changelog(args: &[&str]) -> Result<String> {
    Ok(format!("{}\n", git_cliff(args)?.trim_end()))
}

/// `cargo xtask changelog`: regenerate CHANGELOG.md (unreleased commits
/// appear under "Unreleased").
pub fn changelog() -> Result<()> {
    let text = render_changelog(&[])?;
    std::fs::write(root().join("CHANGELOG.md"), text)?;
    println!("Wrote CHANGELOG.md");
    Ok(())
}

fn version_tags() -> Result<Vec<String>> {
    Ok(run("git", &["tag", "--list"])?
        .lines()
        .filter(|t| parse_semver(t).is_ok())
        .map(str::to_owned)
        .collect())
}

/// Set `[workspace.package].version` in the root Cargo.toml, keeping formatting.
pub fn set_workspace_version(manifest: &str, version: &str) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = manifest.parse()?;
    let Some(item) = doc
        .get_mut("workspace")
        .and_then(|w| w.get_mut("package"))
        .and_then(|p| p.get_mut("version"))
    else {
        bail!("Cargo.toml has no [workspace.package] version");
    };
    let decor = item.as_value().map(|v| v.decor().clone());
    *item = toml_edit::value(version);
    if let (Some(decor), Some(value)) = (decor, item.as_value_mut()) {
        *value.decor_mut() = decor;
    }
    Ok(doc.to_string())
}

pub fn create(requested: Option<&str>) -> Result<()> {
    if !run("git", &["status", "--porcelain"])?.is_empty() {
        bail!("Working tree must be clean; commit or stash changes first.");
    }
    if run("git", &["rev-parse", "--is-shallow-repository"])? == "true" {
        bail!("Full Git history is required; run git fetch --unshallow.");
    }
    run("git", &["symbolic-ref", "--quiet", "HEAD"])
        .map_err(|_| "Releases must be created from a branch, not a detached HEAD.")?;

    let current = workspace_version()?;
    let current_v = parse_semver(&current)?;
    let tags = version_tags()?;
    let latest_tag = tags
        .iter()
        .max_by_key(|t| parse_semver(t).expect("filtered"))
        .cloned();
    if let Some(base) = &latest_tag
        && run("git", &["log", &format!("{base}..HEAD"), "--format=%H"])?.is_empty()
    {
        bail!("No commits since the latest release.");
    }

    let version = match requested {
        Some(v) => v.strip_prefix('v').unwrap_or(v).to_owned(),
        None => {
            let bumped = git_cliff(&["--bumped-version"])?;
            let bumped = bumped.lines().last().unwrap_or_default().trim();
            bumped.strip_prefix('v').unwrap_or(bumped).to_owned()
        }
    };
    let new_v = parse_semver(&version)?;
    let tag = format!("v{version}");
    if tags.contains(&tag) || tags.contains(&version) {
        bail!("Version {version} is already tagged.");
    }
    if new_v < current_v {
        bail!("Version must not be lower than {current}.");
    }
    if let Some(base) = &latest_tag
        && new_v <= parse_semver(base)?
    {
        bail!("Version must be greater than latest tag {base}.");
    }

    // Render before editing anything so an invalid history cannot half-bump.
    let changelog = render_changelog(&["--tag", &tag])?;
    let manifest_path = root().join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path)?;
    std::fs::write(&manifest_path, set_workspace_version(&manifest, &version)?)?;
    std::fs::write(root().join("CHANGELOG.md"), changelog)?;
    run(&cargo(), &["update", "--workspace"])?;

    let mut add = vec!["add", "--"];
    add.extend(RELEASE_FILES);
    run("git", &add)?;
    run("git", &["commit", "-m", &format!("chore(release): {tag}")])?;
    run("git", &["tag", "-a", &tag, "-m", &format!("Release {tag}")])?;
    println!("Created {tag} locally. Review, then publish with:");
    println!("  git push --atomic origin HEAD refs/tags/{tag}");
    Ok(())
}

/// Version recorded for the `framecover` package in Cargo.lock.
fn locked_app_version(lock: &str) -> Result<Option<String>> {
    let doc: toml_edit::DocumentMut = lock.parse()?;
    let Some(packages) = doc.get("package").and_then(|p| p.as_array_of_tables()) else {
        return Ok(None);
    };
    Ok(packages
        .iter()
        .find(|p| p.get("name").and_then(|n| n.as_str()) == Some("framecover"))
        .and_then(|p| p.get("version").and_then(|v| v.as_str()).map(str::to_owned)))
}

/// The `## [<tag>]...` section of a rendered changelog.
pub fn section_for(changelog: &str, tag: &str) -> Option<String> {
    let header = format!("## [{tag}]");
    let start = changelog
        .find(&format!("\n{header}"))
        .map(|i| i + 1)
        .or_else(|| changelog.starts_with(&header).then_some(0))?;
    let rest = &changelog[start..];
    let end = rest[header.len()..]
        .find("\n## ")
        .map(|i| i + header.len() + 1)
        .unwrap_or(rest.len());
    Some(format!(
        "{}\n",
        rest[..end].replace("<!-- insertion marker -->", "").trim()
    ))
}

pub fn notes(tag: &str, output_dir: Option<PathBuf>) -> Result<()> {
    let version = tag.strip_prefix('v').unwrap_or(tag);
    parse_semver(version)?;
    if tag != format!("v{version}") {
        bail!("Release tags must use the vX.Y.Z format.");
    }
    if workspace_version()? != version {
        bail!("Tag and Cargo.toml workspace version do not match.");
    }
    let lock = std::fs::read_to_string(root().join("Cargo.lock"))?;
    if locked_app_version(&lock)?.as_deref() != Some(version) {
        bail!("Tag and Cargo.lock version do not match (run `cargo update --workspace`).");
    }
    if run("git", &["rev-parse", &format!("{tag}^{{commit}}")])?
        != run("git", &["rev-parse", "HEAD"])?
    {
        bail!("Release tag must point at HEAD.");
    }
    let changelog = render_changelog(&[])?;
    let Some(section) = section_for(&changelog, tag) else {
        bail!("git-cliff did not produce an entry for {tag}.");
    };
    let out = output_dir.unwrap_or_else(|| root().join("release-assets"));
    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("CHANGELOG.md"), &changelog)?;
    std::fs::write(out.join("release-notes.md"), &section)?;
    print!("{section}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHANGELOG: &str = "# Changelog\n\nIntro.\n\n<!-- insertion marker -->\n\
## [v0.2.0](https://x/releases/tag/v0.2.0) - 2026-11-01\n\n### Features\n\n- new\n\n\
## [v0.1.0](https://x/releases/tag/v0.1.0) - 2026-10-24\n\n### Features\n\n- old\n";

    #[test]
    fn extracts_release_sections() {
        assert_eq!(
            section_for(CHANGELOG, "v0.2.0").unwrap(),
            "## [v0.2.0](https://x/releases/tag/v0.2.0) - 2026-11-01\n\n### Features\n\n- new\n"
        );
        assert_eq!(
            section_for(CHANGELOG, "v0.1.0").unwrap(),
            "## [v0.1.0](https://x/releases/tag/v0.1.0) - 2026-10-24\n\n### Features\n\n- old\n"
        );
        assert!(section_for(CHANGELOG, "v0.3.0").is_none());
        // `v0.1.0` must not match a `v0.1.0x`-style prefix of another tag.
        assert!(section_for("## [v0.1.00]\n", "v0.1.0").is_none());
    }

    #[test]
    fn bumps_only_the_workspace_version() {
        let manifest = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.1.0\" # keep\nedition = \"2024\"\n\n[dependencies]\nfoo = { version = \"0.1.0\" }\n";
        let out = set_workspace_version(manifest, "0.2.0").unwrap();
        assert!(out.contains("version = \"0.2.0\" # keep"), "{out}");
        assert!(out.contains("foo = { version = \"0.1.0\" }"));
        assert!(set_workspace_version("[package]\nversion = \"1.0.0\"\n", "2.0.0").is_err());
    }

    #[test]
    fn reads_app_version_from_lockfile() {
        let lock = "version = 4\n\n[[package]]\nname = \"framecover-core\"\nversion = \"9.9.9\"\n\n[[package]]\nname = \"framecover\"\nversion = \"0.1.0\"\n";
        assert_eq!(locked_app_version(lock).unwrap().as_deref(), Some("0.1.0"));
        assert_eq!(locked_app_version("version = 4\n").unwrap(), None);
    }

    #[test]
    fn current_lockfile_matches_workspace_version() {
        let lock = std::fs::read_to_string(root().join("Cargo.lock")).unwrap();
        assert_eq!(
            locked_app_version(&lock).unwrap(),
            Some(workspace_version().unwrap())
        );
    }
}
