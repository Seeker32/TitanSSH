 # Release process

Releases are cut by pushing a `vX.Y.Z` tag. The tag push triggers `.github/workflows/release.yml`, which builds installers for macOS-arm64, macOS-x64, and Windows (~13 min), then publishes a GitHub release whose notes are extracted from `CHANGELOG.md`.

## Pre-tag checklist (all in the commit the tag points to)

1. **CHANGELOG.md** — add a section at the top:

   ```markdown
   ## [X.Y.Z] - YYYY-MM-DD

   ### Features
   ### Changed
   ### Fixed
   ```

   - English bullets, grouped by category, date = tag creation date.
   - Source material: `git log v<previous>..HEAD --format='%s'`.
2. **Version bump** — exactly 4 files, only the app's own version string:
   - `package.json` → `"version"`
   - `src-tauri/Cargo.toml` → `version`
   - `src-tauri/Cargo.lock` → only the `name = "titanssh"` package entry (never touch the many other `0.1.x` dependency entries)
   - `src-tauri/tauri.conf.json` → `"version"`
3. **Tag**:

   ```bash
   git push origin dev          # branch first; branch pushes do NOT trigger the workflow
   git tag vX.Y.Z && git push origin vX.Y.Z
   ```

Golden rule: the tag must point to a commit containing **both** the changelog section and the version bump. Missing either makes the release job fail or ship wrong-versioned binaries.

## Workflow facts

- Only `v*` tag pushes (and `workflow_dispatch`) trigger the workflow. Pushing branches never does.
- `release` job extracts notes via `node scripts/release-notes.mjs <tag> CHANGELOG.md release-notes.md`; if the changelog section for the tag is missing, this step fails after all builds succeeded.
- The version baked into binaries comes from `src-tauri/tauri.conf.json` at the tagged commit — not from the tag name.

## Failure handling

- **Check artifact versions before any manual shortcut.** Download artifacts with `gh run download <run-id>` and look at the file names; verify with `git show <tag>:src-tauri/tauri.conf.json`. If the binaries are the old version, they cannot be released under the new tag.
- **Wrong-version binaries → full rebuild is mandatory.** Bump the version, commit, then re-tag and re-push (this re-triggers the build):

  ```bash
  git tag -f vX.Y.Z && git push origin vX.Y.Z -f
  ```

- **Builds fine, only the release job failed → rebuild is avoidable.** Download artifacts from the failed run and publish manually:

  ```bash
  gh run download <run-id> --dir release-assets
  node scripts/release-notes.mjs vX.Y.Z CHANGELOG.md release-notes.md
  gh release create vX.Y.Z --notes-file release-notes.md release-assets/**/*
  ```

- Force-moving an already-pushed tag always triggers a full rebuild. Only do it as the rebuild path above, never to "fix" a tag that already shipped.
