Installation
============

.. note::

   This guide walks you through installing TAPA.
   The recommended installation method is from pre-built releases.

Installing from Releases
~~~~~~~~~~~~~~~~~~~~~~~~

The easiest way to install TAPA is from a pre-built release:

.. code-block:: bash

   curl -fsSL https://raw.githubusercontent.com/tuna/tapa/main/install.sh | sh -s -- -q

This downloads and installs the latest release. With root privileges, TAPA
installs to ``/opt/tapa`` with symlinks in ``/usr/local/bin``. Otherwise it
installs to ``~/.tapa`` and adds it to your ``PATH`` via your shell profile.

To install a specific version:

.. code-block:: bash

   TAPA_VERSION=0.1.20260319 \
     curl -fsSL https://raw.githubusercontent.com/tuna/tapa/main/install.sh | sh -s -- -q

Releases are available at
`github.com/tuna/tapa/releases <https://github.com/tuna/tapa/releases>`_.

Verify the installation:

.. code-block:: bash

   tapa --version

System Prerequisites
~~~~~~~~~~~~~~~~~~~~

TAPA requires the following dependencies:

+-------------------+-----------------+----------------------------------------------+
| Dependency        | Version         | Notes                                        |
+===================+=================+==============================================+
| GNU C++ Compiler  | 7.5.0 or newer  | For simulation and deployment only           |
+-------------------+-----------------+----------------------------------------------+
| Xilinx Vitis      | 2022.1 or newer |                                              |
+-------------------+-----------------+----------------------------------------------+

TAPA has been tested on the following operating systems. Use the
appropriate package manager to install the required dependencies if using a
different OS.

Ubuntu / Debian
^^^^^^^^^^^^^^^

.. note::

   For **Ubuntu 18.04 and newer**, or **Debian 10 and newer**.

.. code-block:: bash

  sudo apt-get install g++

RHEL / Amazon Linux
^^^^^^^^^^^^^^^^^^^

.. note::

   For **Red Hat Enterprise Linux 9 and newer**, derivatives like **AlmaLinux
   9 and newer** and **Rocky Linux 9 and newer**, or **Amazon Linux 2023**.

.. code-block:: bash

  sudo yum install gcc-c++ libxcrypt-compat

Fedora
^^^^^^

.. note::

   For **Fedora 34 and newer**. Fedora 39 and newer may have minor issues due
   to system C library changes and Vitis HLS tool incompatibility.

.. code-block:: bash

  sudo yum install gcc-c++ libxcrypt-compat

Building from Source
~~~~~~~~~~~~~~~~~~~~

For detailed build instructions, see
:ref:`Building from Source <dev/build:Building from Source>`.

Quick start:

.. code-block:: bash

  # Install Bazel (see https://bazel.build/install)

  git clone https://github.com/tuna/tapa.git
  cd tapa
  bazel build //...

Verify the installation by running:

.. code-block:: bash

  bazel-bin/tapa/tapa --version

.. note::

   Pre-built releases are available at
   `github.com/tuna/tapa/releases <https://github.com/tuna/tapa/releases>`_
   and can be installed with the one-liner above.
