# Contributing to TAPA

## Pull Request Process

1. Fork the TAPA repository and create a new branch for your feature or bug fix.
2. Make small, atomic commits with Conventional subjects (for example,
   `fix(tapa-ir): reject invalid stream depths`). Include the required
   `Co-Authored-By: Kimi K3.1 Pro <noreply@moonshot.ai>` trailer in every
   commit.
3. Ensure all tests pass and pre-commit hooks run successfully.
4. Write a clear and concise description of your changes in the pull request.
5. Request a review from the TAPA maintainers.

### Continuous Integration

TAPA uses GitHub Actions for continuous integration:

1. Development-branch pushes run pre-commit, Bazel tests, the documentation
   build, and the public-API audit. A branch whose name ends in `/next` is
   fast-forwarded into `main` once all four pass.
2. Pushes to `main` build a release artifact and run the staging test matrix
   across supported operating systems and Vitis versions.
3. Release builds package the binary distribution on self-hosted runners.
4. A nightly job builds the regression designs, checks that the links in
   `README.md` and `docs/src/` are live, and bumps `VERSION`.

### Documentation

- Update the documentation in the `docs/src/` directory for any new features
  or changes.
- Use Markdown format for documentation files.
- Preview your changes locally with a live-reloading server. Bazel fetches
  mdBook for you, so nothing needs to be installed first:

  ```bash
  bazel run //docs:serve
  ```

- To produce the static HTML site instead (output: `bazel-bin/docs/book.tar.gz`):

  ```bash
  bazel build //docs:build
  ```

### Testing

- Land a bug fix with the test that would have caught it. Commit a new test
  only when it protects a contract no existing test owns (see `AGENTS.md`).
- Ensure all existing tests pass before submitting your changes.
- Run the full test suite using the following command:

  ```bash
  bazel test //...
  ```

- If you change the task-graph schema, note that it has **two** implementations
  that must agree: `tapacc` emits the JSON
  (`tapacc/frontend/program_builder.cpp`), and the `tapa-ir` Rust crate
  parses it with `deny_unknown_fields`. The fixtures under
  `tapa-core/testdata/` are hand-written and cannot catch the two drifting
  apart. `//tapa-core:tapacc_conformance_test` is what does: it runs the real
  `tapacc` on `tests/apps/vadd/vadd.cpp` and three functional-test kernels
  and strict-parses its verbatim stdout. `tapacc` is a Clang tool that only
  builds on Linux, so the test is gated on `@platforms//os:linux` and is
  skipped everywhere else:

  ```bash
  bazel test //tapa-core:tapacc_conformance_test  # Linux only
  ```

## Reporting Issues

- Use the GitHub issue tracker to report bugs or suggest new features.
- Provide a clear and concise description of the issue or feature request.
- Include steps to reproduce the issue, if applicable.
- Attach relevant log files or screenshots, if available.

## Community Guidelines

- Be respectful and considerate in all interactions with other contributors.
- Provide constructive feedback on pull requests and issues.
