# Contributing to TAPA

## Pull Request Process

1. Fork the TAPA repository and create a new branch for your feature or bug fix.
2. Ensure all tests pass and pre-commit hooks run successfully.
3. Write a clear and concise description of your changes in the pull request.
4. Request a review from the TAPA maintainers.

### Continuous Integration

TAPA uses GitHub Actions for continuous integration. The CI pipeline:

1. Builds binary distributions on Ubuntu 18.04 self-hosted runners.
2. Performs code quality checks using pre-commit hooks on every commit.
3. Runs functional and integration tests via staging workflows across a matrix
   of platforms and Vitis versions for every main branch push.

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

- Add appropriate unit tests for new features or bug fixes.
- Ensure all existing tests pass before submitting your changes.
- Run the full test suite using the following command:

  ```bash
  bazel test //...
  ```

- If you change the task-graph schema, note that it has **two** implementations
  that must agree: `tapacc/tapacc.cpp` emits the JSON, and the `tapa-ir` Rust
  crate parses it with `deny_unknown_fields`. The fixtures under
  `tapa-core/testdata/` are hand-written and cannot catch the two drifting
  apart. `//tapa-core:tapacc_conformance_test` is what does: it runs the real
  `tapacc` on `tests/apps/vadd/vadd.cpp` and strict-parses its verbatim
  stdout. `tapacc` is a Clang tool that only builds on Linux, so the test is
  gated on `@platforms//os:linux` and is skipped everywhere else:

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
