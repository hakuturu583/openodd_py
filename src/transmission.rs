//! Versioning an ODD: the transmission it was read from, and its revision.
//!
//! ASAM OpenODD 1.0 has no notion of a version of an ODD. What it does define
//! is enough to build one on, and this module does no more than that (see
//! `docs/versioning.md`):
//!
//! * a **transmission** is the set of files sent from one stakeholder to
//!   another (6.1.4.3); every file in it has a name unique within it (class
//!   `File`, 7.2.1.6), files refer to each other by those names (`IMPORT`), and
//!   the whole may be wrapped in one archive;
//! * **MetaData** -- key/value pairs -- may be attached to a module (6.4.6.3),
//!   "for traceability by external references" (6.1.3.3).
//!
//! So an ODD's **revision** is the digest of its transmission: the name and
//! the SHA-256 of every file, and the file the ODD is entered from. The same
//! files give the same revision on any machine, whatever repository or commit
//! they came from; any change to any file -- a bucket, a module, a unit -- is a
//! new revision. A human-facing **version** is what the ODD's root modules
//! declare in their `METADATA` (`version: 1.2.0`), and where files came from
//! (a git URL, a commit) is recorded as their **origin**, outside the digest.
//!
//! A transmission packs into a deterministic archive (`.tar.gz`): the same
//! files always give the same bytes, so the archive can be stored by its
//! revision and the ODD read back from it exactly.

use std::collections::BTreeMap;
use std::io::Read;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// The schema of a transmission's manifest.
pub const MANIFEST_SCHEMA: &str = "openodd.transmission/1";
/// The manifest's name inside an archive.
pub const MANIFEST_NAME: &str = "openodd-transmission.json";
const REVISION_DOMAIN: &str = "openodd-transmission/1";

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// JSON with sorted keys and no blanks: the same value, the same text.
pub fn canonical_json(value: &Json) -> String {
    fn sort(v: &Json) -> Json {
        match v {
            Json::Object(m) => {
                let sorted: BTreeMap<&String, Json> = m.iter().map(|(k, v)| (k, sort(v))).collect();
                Json::Object(sorted.into_iter().map(|(k, v)| (k.clone(), v)).collect())
            }
            Json::Array(a) => Json::Array(a.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string(&sort(value)).expect("JSON serializes")
}

/// Where a file came from: a file in a git repository at a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub git: String,
    /// The revision asked for: a tag, a branch or a commit.
    pub rev: String,
    /// The commit it named.
    pub commit: String,
    /// The file's path in the repository.
    pub path: String,
}

/// One file of a transmission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransmissionFile {
    /// Unique within the transmission (OpenODD's class `File`).
    pub name: String,
    pub data: Vec<u8>,
    pub origin: Option<Origin>,
}

impl TransmissionFile {
    pub fn sha256(&self) -> String {
        sha256_hex(&self.data)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }
}

/// A file as the manifest lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

/// What an archive says of its transmission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: String,
    pub revision: String,
    /// The file the ODD is entered from: a binding file, or an OpenODD file.
    pub entry: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub files: Vec<FileEntry>,
}

/// The files an ODD was read from, in the order they were read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transmission {
    pub files: Vec<TransmissionFile>,
    /// The name of the file the ODD is entered from.
    pub entry: String,
    /// What the root modules' `METADATA` declare as `version`, if anything.
    pub version: Option<String>,
}

impl Transmission {
    /// Add a file read under `name`; a file read twice is kept once. Two
    /// different files under one name are refused, as OpenODD refuses them.
    pub fn add(&mut self, name: &str, data: Vec<u8>, origin: Option<Origin>) -> Result<()> {
        if let Some(f) = self.files.iter().find(|f| f.name == name) {
            if f.data == data {
                return Ok(());
            }
            return Err(Error::open_odd(format!(
                "two files named {name}: OpenODD makes file names unique within a transmission"
            )));
        }
        if name == MANIFEST_NAME || name.contains('/') || name.is_empty() {
            return Err(Error::open_odd(format!(
                "'{name}' cannot be a file of a transmission"
            )));
        }
        self.files.push(TransmissionFile {
            name: name.to_string(),
            data,
            origin,
        });
        Ok(())
    }

    pub fn file(&self, name: &str) -> Option<&TransmissionFile> {
        self.files.iter().find(|f| f.name == name)
    }

    /// The revision: the digest of the entry and of every file's name and content.
    pub fn revision(&self) -> String {
        let mut lines: Vec<String> = self
            .files
            .iter()
            .map(|f| format!("file\t{}\t{}\n", f.name, f.sha256()))
            .collect();
        lines.sort();
        let text = format!(
            "{REVISION_DOMAIN}\nentry\t{}\n{}",
            self.entry,
            lines.concat()
        );
        sha256_hex(text.as_bytes())
    }

    pub fn manifest(&self) -> Manifest {
        Manifest {
            schema: MANIFEST_SCHEMA.into(),
            revision: self.revision(),
            entry: self.entry.clone(),
            version: self.version.clone(),
            files: self
                .files
                .iter()
                .map(|f| FileEntry {
                    name: f.name.clone(),
                    sha256: f.sha256(),
                    size: f.data.len() as u64,
                    origin: f.origin.clone(),
                })
                .collect(),
        }
    }

    /// The transmission as a deterministic `.tar.gz`: the manifest, then the
    /// files by name, with fixed times, owners and modes.
    pub fn to_archive(&self) -> Result<Vec<u8>> {
        let manifest = serde_json::to_vec_pretty(&self.manifest()).expect("a manifest serializes");
        let mut entries: Vec<(&str, &[u8])> = vec![(MANIFEST_NAME, &manifest)];
        let mut files: Vec<&TransmissionFile> = self.files.iter().collect();
        files.sort_by(|a, b| a.name.cmp(&b.name));
        entries.extend(files.iter().map(|f| (f.name.as_str(), f.data.as_slice())));
        let mut tar = tar::Builder::new(Vec::new());
        tar.mode(tar::HeaderMode::Deterministic);
        for (name, data) in entries {
            let mut header = tar::Header::new_ustar();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_uid(0);
            header.set_gid(0);
            header.set_entry_type(tar::EntryType::Regular);
            tar.append_data(&mut header, name, data)?;
        }
        let tarball = tar.into_inner()?;
        let mut gz = flate2::GzBuilder::new()
            .mtime(0)
            .write(Vec::new(), flate2::Compression::best());
        std::io::Write::write_all(&mut gz, &tarball)?;
        Ok(gz.finish()?)
    }

    /// A transmission back from [`Transmission::to_archive`]'s bytes, checked
    /// against its manifest.
    pub fn from_archive(bytes: &[u8]) -> Result<Self> {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
        let mut manifest: Option<Manifest> = None;
        let mut data: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for entry in tar.entries()? {
            let mut entry = entry?;
            let name = entry.path()?.to_string_lossy().into_owned();
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            if name == MANIFEST_NAME {
                manifest = Some(
                    serde_json::from_slice(&buf)
                        .map_err(|e| Error::open_odd(format!("{MANIFEST_NAME}: {e}")))?,
                );
            } else {
                data.insert(name, buf);
            }
        }
        let manifest = manifest
            .ok_or_else(|| Error::open_odd(format!("no {MANIFEST_NAME} in the archive")))?;
        if manifest.schema != MANIFEST_SCHEMA {
            return Err(Error::open_odd(format!(
                "not a transmission of schema {MANIFEST_SCHEMA}: {}",
                manifest.schema
            )));
        }
        let mut t = Transmission {
            entry: manifest.entry.clone(),
            version: manifest.version.clone(),
            ..Default::default()
        };
        for f in &manifest.files {
            let bytes = data
                .remove(&f.name)
                .ok_or_else(|| Error::open_odd(format!("the archive lacks {}", f.name)))?;
            if sha256_hex(&bytes) != f.sha256 {
                return Err(Error::open_odd(format!(
                    "{} does not match its sha256",
                    f.name
                )));
            }
            t.add(&f.name, bytes, f.origin.clone())?;
        }
        if t.revision() != manifest.revision {
            return Err(Error::open_odd(
                "the archive's files do not make its revision",
            ));
        }
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Transmission {
        let mut t = Transmission {
            entry: "odd.yaml".into(),
            ..Default::default()
        };
        t.add("odd.yaml", b"openodd: [taxonomy.yaml]\n".to_vec(), None)
            .unwrap();
        t.add(
            "taxonomy.yaml",
            b"TAXONOMY: {rain: [none, heavy]}\n".to_vec(),
            Some(Origin {
                git: "https://x/odd.git".into(),
                rev: "v1".into(),
                commit: "a".repeat(40),
                path: "taxonomy.yaml".into(),
            }),
        )
        .unwrap();
        t
    }

    #[test]
    fn the_revision_is_the_files_not_where_they_came_from() {
        let a = sample();
        let mut b = sample();
        b.files[1].origin = None;
        b.files.reverse();
        assert_eq!(a.revision(), b.revision());
        let mut c = sample();
        c.files[1].data.push(b'\n');
        assert_ne!(a.revision(), c.revision());
    }

    #[test]
    fn an_archive_is_deterministic_and_reads_back() {
        let a = sample();
        let bytes = a.to_archive().unwrap();
        assert_eq!(bytes, a.to_archive().unwrap());
        let back = Transmission::from_archive(&bytes).unwrap();
        assert_eq!(back.revision(), a.revision());
        assert_eq!(
            back.file("taxonomy.yaml").unwrap().origin,
            a.files[1].origin
        );
    }

    #[test]
    fn two_files_under_one_name_are_refused() {
        let mut t = sample();
        assert!(t.add("odd.yaml", b"other".to_vec(), None).is_err());
        assert!(t
            .add("odd.yaml", b"openodd: [taxonomy.yaml]\n".to_vec(), None)
            .is_ok());
    }
}
