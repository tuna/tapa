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

- Update the documentation in the `docs/` directory for any new features
  or changes.
- Use Markdown format for documentation files.
- Run the following command in the `docs/` directory to build and preview
  documentation changes locally:

  ```bash
  bash build.sh
  ```

### Testing

- Add appropriate unit tests for new features or bug fixes.
- Ensure all existing tests pass before submitting your changes.
- Run the full test suite using the following command:

  ```bash
  bazel test //...
  ```

## Reporting Issues

- Use the GitHub issue tracker to report bugs or suggest new features.
- Provide a clear and concise description of the issue or feature request.
- Include steps to reproduce the issue, if applicable.
- Attach relevant log files or screenshots, if available.

## Community Guidelines

- Be respectful and considerate in all interactions with other contributors.
- Provide constructive feedback on pull requests and issues.
