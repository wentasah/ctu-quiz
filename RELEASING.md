# Releasing

The CLI (`quiz`) and the server (`quiz-server`) are released together under
one version, the `version` in `[workspace.package]` of `Cargo.toml`. The Nix
package reads it from there too. A release is an annotated tag `vX.Y.Z` on
`main`, plus a section about it in `CHANGELOG.md`.

## Choosing the version

The project follows [Semantic Versioning](https://semver.org). While the
version is 0.x:

- **Minor** (0.1.0 → 0.2.0): any change that breaks something, i.e. any
  change that needs the CLI and the server upgraded together, removes or
  renames an option, or changes the question format or the saved state
  file in an incompatible way. New features also go into a minor release.
- **Patch** (0.2.0 → 0.2.1): only fixes and changes that are compatible both
  ways between the CLI and the server.

Say in the changelog when the CLI and the server have to be upgraded
together.

## Commit messages

The changelog is generated from commit subjects by
[git-cliff](https://git-cliff.org) (configuration in `cliff.toml`), so:

- Prefix the subject with the part the commit changes: `Server:`, `CLI:`,
  `README:`, `Nix:`, `CI:`, `CSS:`. Leave out the prefix when a single
  change spans the CLI and the server.
- Start the subject with `Add` for a new feature and with `Fix` for a bug
  fix. These end up under *Added* and *Fixed*, everything else under
  *Changed*.
- Write the subject for users: it becomes the changelog entry.

## Steps

1. **Check `main`.** The newest pipeline on `main` passed and
   `nix flake check` passes locally.

2. **Look at what is unreleased** and choose the version:

   ```sh
   git cliff --unreleased
   ```

3. **Prepare the release:**

   ```sh
   scripts/release.sh X.Y.Z
   ```

   The script checks that you are on an up-to-date `main` with no
   uncommitted changes. It sets the version in `Cargo.toml` and
   `Cargo.lock` and adds a section for the release to `CHANGELOG.md`
   (generating the whole file for the first release). Then it opens
   `CHANGELOG.md` in `$VISUAL` / `$EDITOR`, runs the tests, commits
   "Release vX.Y.Z" and creates the annotated tag `vX.Y.Z`.

   When the editor opens, edit the new section by hand. Move misplaced
   entries, merge entries that describe one change, and drop the ones that
   do not matter to users. Add a line on top saying what needs attention
   when upgrading (the CLI and the server upgraded together, a new option in
   the NixOS module, …). The generated text is only a draft, and the
   section becomes the release notes.

   If something fails, undo with `git reset --hard origin/main` and
   `git tag -d vX.Y.Z`.

4. **Publish** by pushing the commit and the tag:

   ```sh
   git push --atomic origin main vX.Y.Z
   ```

   The tag pipeline does the rest:

   - `check-release` fails the pipeline unless the tag matches the version
     in `Cargo.toml` and `CHANGELOG.md` has a section for it.
   - The build jobs upload the binaries to the
     [package registry](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/packages),
     where they do not expire: `quiz-vX.Y.Z-<platform>.tar.gz`, and
     `quiz-vX.Y.Z-windows-x86_64.exe` for Windows.
   - `release` creates the
     [GitLab release](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/releases)
     with the changelog section as notes and links to the binaries.

   If the pipeline fails, see below.

5. **Deploy the server** from the tag (NixOS: point the flake input at
   `?ref=vX.Y.Z`; Docker: `git checkout vX.Y.Z && docker compose up -d
   --build`) and check that it runs the new version:

   ```sh
   quiz-server --version
   ```

   Deploy between lectures: a restart drops open questions unless the server
   runs with `--state-file`.

6. **Tell the teachers** when they have to upgrade the CLI, with a link to
   the release.

## When the release pipeline fails

A temporary failure (a runner, the network) only needs the failed job
retried in the pipeline.

If `check-release` fails, nothing has been uploaded yet. Delete the tag
(`git push origin :vX.Y.Z` and `git tag -d vX.Y.Z`), fix `main`, and run
the script again with the same version.

If a later job fails because of a mistake, do not reuse the version, because
some binaries may already be in the package registry. Fix it on `main` and
release the next patch version.

## Fixing a release

Do not move or delete a published tag. Fix the problem on `main` and release
a new patch version.
