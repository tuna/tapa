Releasing TAPA Builds
=====================

.. note::

   This section explains how to release TAPA builds. It is intended for
   **maintainers** with write access to the TAPA repository.

To create a release build:

1. Build the binary distribution:

   .. code-block:: bash

      bazel build --config=release //:tapa-pkg-tar

2. Tag the release and create a GitHub release with the tarball from
   ``bazel-bin/tapa-pkg-tar.tar``.

3. Users can install from the tarball using the ``install.sh`` script:

   .. code-block:: bash

      TAPA_LOCAL_PACKAGE=./tapa-pkg-tar.tar ./install.sh
