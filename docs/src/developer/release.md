# Releasing TAPA Builds

```admonish note
This section explains how to release TAPA builds. It is intended for
**maintainers** with write access to the TAPA repository.
```

## Automated Release Process

Releases are automated via GitHub Actions. The `publish-release.yml`
workflow builds and publishes a release to GitHub Releases.

To create a release:

1. Update the `VERSION` file on `main` with the desired version string
   (e.g. `0.1.20260721`). The nightly `Bump Version` job already sets its
   date part to the current date (UTC-10) when anything outside `docs/`,
   `tests/`, and `README.md` changed since the last bump, landing the commit
   on `main` through the `ci/next` branch.

2. Trigger the `Publish Release` workflow via `workflow_dispatch` from
   the GitHub Actions UI. Optionally override the version in the input field;
   if left blank, the contents of the `VERSION` file are used.

The workflow will:

- Build the release tarball on a self-hosted runner
- Create the tag `v<version>` and publish `tapa.tar.gz` to GitHub Releases

After the release is published, re-pin the hand-maintained `TAPA_VERSION`
examples (`README.md` and `docs/src/start/installation.md`) to the new tag;
nothing bumps them automatically.

## Staging Builds

Every push to `main` triggers the `staging-build.yml` workflow, which
builds the release artifact and runs the staging test matrix across supported
OS and Vitis version combinations. The artifact is retained as a workflow
artifact for 7 days; it is not published as a full release. A pushed tag
also runs the workflow; a tag whose name does not start with `v` then
publishes the artifact as a GitHub prerelease named `Staging Build <commit>`.

## Installing a Release

Users can install a published release with:

```bash
curl -fsSL https://raw.githubusercontent.com/tuna/tapa/main/install.sh | sh -s -- -q
```

To install a specific version by tag:

```bash
curl -fsSL https://raw.githubusercontent.com/tuna/tapa/main/install.sh | TAPA_VERSION=x.y.z sh -s -- -q
```

To install from a local release tarball:

```bash
TAPA_LOCAL_PACKAGE=./tapa.tar.gz ./install.sh -q
```
