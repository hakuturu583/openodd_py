//! OpenODD files kept in git repositories, pinned to a revision.
//!
//! A binding file names them next to local files:
//!
//! ```yaml
//! openodd:
//!   - git: https://example.com/odd/taxonomy.git
//!     rev: v1.2.0                 # a tag, a branch or a commit
//!     path: taxonomy.yml          # in the repository; a list for several
//!   - odd.yml                     # relative to the binding file
//! ```
//!
//! Each repository is fetched once into a cache, and each commit is checked
//! out once: the files of a commit never change, so a commit already checked
//! out is used without the network. A tag or a branch is fetched on every
//! load, to learn which commit it names now.
//!
//! The cache is `$OPENODD_CACHE`, or `~/.cache/openodd`.

use std::path::{Path, PathBuf};
use std::process::Command;

use fs2::FileExt;
use regex::Regex;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

pub const CACHE_ENV: &str = "OPENODD_CACHE";
/// Git refuses `ext::` by default; this keeps it refused whatever the user's
/// configuration says, so a URL never runs a command.
const GIT: [&str; 3] = ["git", "-c", "protocol.ext.allow=never"];

/// OpenODD files in a git repository at a revision.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitSource {
    pub url: String,
    pub rev: String,
    pub path: String,
}

impl GitSource {
    pub fn new(
        url: impl Into<String>,
        rev: impl Into<String>,
        path: impl Into<String>,
    ) -> Result<Self> {
        let s = GitSource {
            url: url.into(),
            rev: rev.into(),
            path: path.into(),
        };
        for (key, value) in [("url", &s.url), ("rev", &s.rev), ("path", &s.path)] {
            if value.trim().is_empty() {
                return Err(Error::GitSource(format!(
                    "git source: {key} must be a non-empty string"
                )));
            }
            if value.starts_with('-') || value.chars().any(|c| matches!(c, '\n' | '\r' | '\0')) {
                return Err(Error::GitSource(format!("git source: bad {key} '{value}'")));
            }
        }
        Ok(s)
    }
}

/// A commit's files in the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub root: PathBuf,
    pub commit: String,
}

/// Where repositories and checkouts are cached.
pub fn cache_dir() -> PathBuf {
    if let Ok(env) = std::env::var(CACHE_ENV) {
        if !env.is_empty() {
            if let Some(rest) = env.strip_prefix("~/") {
                if let Some(home) = std::env::var_os("HOME") {
                    return PathBuf::from(home).join(rest);
                }
            }
            return PathBuf::from(env);
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cache").join("openodd")
}

fn git(args: &[&str], cwd: &Path) -> Result<String> {
    let out = Command::new(GIT[0])
        .args(&GIT[1..])
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::GitSource("git source: git is not installed".into())
            } else {
                Error::GitSource(format!("git: {e}"))
            }
        })?;
    if !out.status.success() {
        return Err(Error::GitSource(format!(
            "git {}: {}",
            args.iter().take(2).copied().collect::<Vec<_>>().join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Fetch `rev` from `url` into the bare `repo`; the commit it names.
fn resolve(repo: &Path, url: &str, rev: &str) -> Result<String> {
    if git(&["fetch", "--depth", "1", "--", url, rev], repo).is_ok() {
        if let Ok(commit) = git(&["rev-parse", "--verify", "FETCH_HEAD^{commit}"], repo) {
            return Ok(commit);
        }
    }
    // An abbreviated commit id, or a server refusing to fetch one.
    let mut args = vec!["fetch"];
    if repo.join("shallow").exists() {
        args.push("--unshallow");
    }
    args.extend([
        "--",
        url,
        "+refs/heads/*:refs/remotes/origin/*",
        "+refs/tags/*:refs/tags/*", // forced: a tag moved upstream moves here
    ]);
    git(&args, repo)?;
    for candidate in [rev.to_string(), format!("origin/{rev}")] {
        if let Ok(commit) = git(
            &["rev-parse", "--verify", &format!("{candidate}^{{commit}}")],
            repo,
        ) {
            return Ok(commit);
        }
    }
    Err(Error::GitSource(format!(
        "git source {url}: no revision '{rev}'"
    )))
}

/// Write `commit`'s files to `dest`, atomically.
fn extract(repo: &Path, commit: &str, dest: &Path) -> Result<()> {
    if dest.exists() {
        return Ok(());
    }
    let parent = dest.parent().expect("a tree has a parent");
    std::fs::create_dir_all(parent)?;
    let tmp = tempdir_in(parent, &format!(".{}-", &commit[..commit.len().min(12)]))?;
    let result = (|| -> Result<()> {
        let out = Command::new(GIT[0])
            .args(&GIT[1..])
            .args(["archive", "--format=tar", commit])
            .current_dir(repo)
            .output()?;
        if !out.status.success() {
            return Err(Error::GitSource(format!(
                "git source: cannot check out {commit}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        if out.stdout.iter().any(|b| *b != 0) {
            let mut archive = tar::Archive::new(out.stdout.as_slice());
            for entry in archive.entries()? {
                // unpack_in refuses paths that would leave `tmp`
                entry?.unpack_in(&tmp)?;
            }
        }
        if let Err(e) = std::fs::rename(&tmp, dest) {
            if !dest.exists() {
                return Err(e.into());
            }
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

fn tempdir_in(parent: &Path, prefix: &str) -> Result<PathBuf> {
    for n in 0..1000u32 {
        let candidate = parent.join(format!("{prefix}{}-{n}", std::process::id()));
        if std::fs::create_dir(&candidate).is_ok() {
            return Ok(candidate);
        }
    }
    Err(Error::Io(format!(
        "cannot make a directory in {}",
        parent.display()
    )))
}

/// The files of `url` at `rev`, fetched into the cache if need be.
pub fn fetch(url: &str, rev: &str) -> Result<Checkout> {
    GitSource::new(url, rev, ".")?;
    let key = &hex::encode(Sha256::digest(url.as_bytes()))[..16];
    let root = cache_dir();
    let trees = root.join("trees").join(key);
    let full_sha = Regex::new(r"^[0-9a-f]{40}$").expect("a valid regex");
    let lower = rev.to_lowercase();
    if full_sha.is_match(&lower) && trees.join(&lower).is_dir() {
        return Ok(Checkout {
            root: trees.join(&lower),
            commit: lower,
        });
    }
    let repos = root.join("repos");
    std::fs::create_dir_all(&repos)?;
    let lock = std::fs::File::create(repos.join(format!("{key}.lock")))?;
    lock.lock_exclusive()?; // runs in parallel share the cache
    let repo = repos.join(format!("{key}.git"));
    let result = (|| -> Result<Checkout> {
        if !repo.join("HEAD").exists() {
            std::fs::create_dir_all(&repo)?;
            git(&["init", "--quiet", "--bare"], &repo)?;
        }
        let commit = resolve(&repo, url, rev)?;
        extract(&repo, &commit, &trees.join(&commit))?;
        Ok(Checkout {
            root: trees.join(&commit),
            commit,
        })
    })();
    let _ = FileExt::unlock(&lock);
    result
}

/// `path` with `.` and `..` worked out without the file system (it may not exist).
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// The file `source` names, and the checkout it is in.
pub fn checkout(source: &GitSource) -> Result<(PathBuf, Checkout)> {
    let co = fetch(&source.url, &source.rev)?;
    let root = co.root.canonicalize()?;
    let path = root.join(&source.path);
    let resolved = path.canonicalize().unwrap_or_else(|_| lexical(&path));
    if !resolved.starts_with(&root) {
        return Err(Error::GitSource(format!(
            "git source {}: {} leaves the repository",
            source.url, source.path
        )));
    }
    if !resolved.is_file() {
        return Err(Error::GitSource(format!(
            "git source {}@{}: no file {}",
            source.url, source.rev, source.path
        )));
    }
    Ok((resolved, co))
}
