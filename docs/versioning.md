# Versioning an ODD

ASAM OpenODD 1.0 defines how to *write* an Operational Design Domain, but not how to tell
two versions of one apart. When runs, coverage or approvals are recorded against "the
urban ODD", it must be settled *which* urban ODD -- and a git commit of whatever repository
the files happened to live in does not settle it: the commit moves with every unrelated
change to that repository, and the same files reach different machines by different
routes.

This library versions an ODD with nothing beyond what the standard defines. This page is
that profile.

## What the standard gives

| ASAM OpenODD 1.0 | Says |
| --- | --- |
| 6.1.4.3 *Files and transmissions* | "A single transmission, that is sending files from one stakeholder to another stakeholder, could include multiple files"; "the collection of all files in a single transmission can be further encapsulated in a single archive file"; "the name of every file shall be unique within the scope of a single transmission". |
| 7.2.1.6 class `File` | A file of a transmission, identified by its `name`. |
| 6.4.6.3 *Module MetaData*, 7.2.1.5 class `MetaData` | Key/value pairs a module (or a section, a condition, a value) may carry. In YAML, `METADATA:` under the module. |
| 6.1.3.3 *ODD features* | "Support for traceability by external references, for example using metadata which may contain permalinks". |

Files refer to each other by name (`IMPORT`), so a transmission is closed under its
imports, and its names are its own namespace.

## The profile

**Transmission.** The files an ODD is read from: the binding file (how each concept is
measured -- outside the standard, but part of what the ODD *is* for an application, since
it fixes probes, units and buckets), every OpenODD file it names, and every file those
`IMPORT`. Names are unique; two different files under one name are refused, as the
standard requires.

**Revision** -- the identity. The SHA-256 of

```
openodd-transmission/1
entry\t<name of the file the ODD is entered from>
file\t<name>\t<sha256 of its bytes>        (one line per file, sorted)
```

So:

- the same files are the same revision on any machine, whatever repository, branch,
  commit or directory they were read from;
- any change to any file -- a bucket edge, a module, a unit conversion, a title -- is a new
  revision; a change elsewhere in the repository is not;
- the revision is computed by the reader, never written by hand, so it cannot be wrong.

**Version** -- the name people use. The `version` key of the root modules' `METADATA`
(the modules under `ODD:`):

```yaml
ODD:
  urban:
    METADATA: {version: 2.1.0, owner: odd-team}
    INCLUDE_AND: {...}
```

It is the standard's own key/value metadata, so other OpenODD tools keep it. Root modules
that declare different versions are refused. A version names a revision for people; it
does not identify one: nothing stops two revisions from declaring the same version, which
is why records keep the revision.

**Origin** -- where a file came from, for traceability (6.1.3.3): the git URL, the revision
asked for (`rev`, a tag or a branch), the commit it named, and the path in the repository.
Recorded per file, *outside* the revision, so the same file fetched from a mirror is the
same ODD.

**Archive.** A transmission packs into one `.tar.gz` (the standard's "single archive
file"): `openodd-transmission.json` (the manifest: schema, revision, entry, version, every
file's name, size, SHA-256 and origin) and the files, with fixed times, owners and modes.
The same files always make the same bytes, so an archive can be stored under its revision,
and the ODD is read back from it exactly -- no network, no repository -- with
`load_odd_archive`.

```python
odd = openodd.load_odd_binding("urban.yaml", backend=backend)
odd.revision              # 64 hex digits
odd.version               # "2.1.0", or None
odd.files                 # [{"name", "sha256", "text", "origin"?}, ...]
archive = odd.archive()   # bytes; read_archive_manifest(archive)["revision"] == odd.revision
same = openodd.load_odd_archive(archive, backend=backend)
```

## Structure digest

An ODD built in code has no files, so no revision. `structure_digest` is the digest of
what an ODD *is* once built -- its modules, every attribute's buckets and coverage criteria,
the buckets outside it -- whatever it was read from. Two ODDs with the same structure digest
judge values and count coverage alike; two revisions may share one (a reworded title, a
reformatted file). Aggregating coverage by revision is the safe default; by structure
digest, the generous one.

## Where this goes next

The profile asks nothing of OpenODD files beyond `METADATA`, which the standard already has,
so it can be proposed as is: a *transmission manifest* and a *transmission digest* are the
two pieces a future revision of the standard would need to make ODD versions exchangeable.
