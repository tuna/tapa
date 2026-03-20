Releasing TAPA Builds
=====================

.. note::

   This section explains how to release TAPA builds. It is intended for
   **maintainers** with write access to the TAPA repository.

Automated Release Process
~~~~~~~~~~~~~~~~~~~~~~~~~

Releases are automated via GitHub Actions. The ``publish-release.yml``
workflow builds and publishes a release to GitHub Releases.

To create a release:

1. Update the ``VERSION`` file on ``main`` with the desired version string
   (e.g. ``0.1.20260319``).

2. Trigger the ``Publish Release`` workflow via ``workflow_dispatch`` from
   the GitHub Actions UI. Optionally override the version in the input field;
   if left blank, the contents of the ``VERSION`` file are used.

The workflow will:

- Build the release tarball on a self-hosted runner
- Create the git tag ``v<version>`` on ``main``
- Publish ``tapa.tar.gz`` and ``tapa-visualizer.tar.gz`` to GitHub Releases

Staging Builds
~~~~~~~~~~~~~~

Every push to ``main`` triggers the ``staging-build.yml`` workflow, which
runs the full test matrix across all supported OS and Vitis version
combinations. Staging builds are uploaded as workflow artifacts (retained
for 7 days) but are not published as releases.

Installing a Release
~~~~~~~~~~~~~~~~~~~~

Users can install a published release with:

.. code-block:: bash

   curl -fsSL https://raw.githubusercontent.com/tuna/tapa/main/install.sh | sh -s -- -q

To install a specific version:

.. code-block:: bash

   TAPA_LOCAL_PACKAGE=./tapa.tar.gz ./install.sh -q
