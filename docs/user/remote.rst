Remote Execution
================

TAPA can offload vendor-tool steps to a remote Linux machine over SSH.  This
is particularly useful when developing on a macOS workstation where
Xilinx/AMD tools are not available natively.

.. note::

   The ``tapa analyze`` step (running ``tapa-cpp`` and ``tapacc``) always
   executes locally.  Vendor-tool steps — including HLS synthesis,
   floorplanning, IP packaging, and co-simulation — are dispatched to the
   remote host when a remote configuration is provided.

Use Case: macOS Workstation + Linux Build Server
------------------------------------------------

A common workflow is:

1. Write and edit TAPA C++ source on macOS.
2. Run ``tapa analyze`` locally to extract the task graph.
3. Run ``tapa synth`` (and optionally ``tapa pack`` or ``tapa cosim``) with
   ``--remote-host`` to execute vendor-tool steps on a Linux machine that has
   Vitis HLS and/or Vivado installed.
4. Retrieve the finished ``.xo`` or ``.zip`` artifact back to the local
   machine automatically (TAPA handles the file transfer).

Remote Flags
------------

The following flags are accepted by the top-level ``tapa`` command and apply
to all sub-commands that perform remote execution.

``--remote-host user@host[:port]``
  Specifies the remote Linux host for vendor tools.  The format is
  ``[user@]host[:port]``.  If the user is omitted the current local username
  is used.  If the port is omitted port 22 is assumed.  This flag overrides
  the ``~/.taparc`` configuration file (see below).

``--remote-key-file PATH``
  Path to the SSH private key file used to authenticate with the remote host.
  If omitted, the SSH agent or default key (``~/.ssh/id_rsa``) is used.

``--remote-xilinx-settings PATH``
  Path to ``settings64.sh`` on the *remote* host.  TAPA sources this file
  before invoking Vitis HLS so that the tool is on ``PATH``.  Example:
  ``/opt/Xilinx/Vitis/2024.1/settings64.sh``.

``--remote-ssh-control-dir DIR``
  Directory on the local machine used to store OpenSSH multiplex control
  sockets.  By default a temporary directory is used.  Sharing a persistent
  directory across invocations allows the SSH master connection to be reused
  between ``tapa`` runs, which reduces connection overhead significantly.

``--remote-ssh-control-persist DURATION``
  OpenSSH ``ControlPersist`` duration.  After the last connection closes the
  master socket remains alive for this long so that subsequent invocations can
  reuse it without establishing a new TCP connection.  Accepts the same values
  as ``ssh_config`` ``ControlPersist``, e.g. ``30m`` or ``4h``.
  Default: ``30m``.

``--remote-disable-ssh-mux``
  Disable OpenSSH connection multiplexing entirely.  Each SSH or SCP
  invocation will open a fresh connection.  This is useful when the remote
  host or an intermediate proxy does not support ``ControlMaster``.

Persistent Configuration via ``~/.taparc``
------------------------------------------

Instead of passing ``--remote-host`` and related flags on every invocation,
you can store the remote configuration in ``~/.taparc`` using YAML syntax:

.. code-block:: yaml

   remote:
     host: build-server.example.com
     user: alice
     port: 22
     key_file: ~/.ssh/id_ed25519
     xilinx_settings: /opt/Xilinx/Vitis/2024.1/settings64.sh
     work_dir: /tmp/tapa-remote
     ssh_control_dir: ~/.ssh/tapa-mux
     ssh_control_persist: 4h
     ssh_multiplex: true

CLI flags always override the corresponding ``~/.taparc`` values.  The
``--remote-host`` flag in particular replaces the ``host``, ``user``, and
``port`` fields parsed from the config file.

``TAPA_CONCURRENCY`` and Remote Execution
-----------------------------------------

The ``TAPA_CONCURRENCY`` environment variable controls the number of parallel
software-simulation threads used by the TAPA host runtime during functional
simulation (``tapa::invoke`` with no bitstream).  It does **not** directly
control the number of parallel HLS jobs dispatched to the remote machine;
that is governed by the ``--jobs`` / ``-j`` flag of ``tapa synth``.

When running synthesis remotely with ``-j N``, TAPA launches up to ``N``
parallel Vitis HLS processes on the remote host.  Keep ``N`` at or below the
number of cores available on the remote machine to avoid resource contention.

Utility: ``tapa version``
------------------------

.. note::

   This command is not specific to remote execution; it works regardless of
   whether a remote configuration is present.

.. code-block:: console

   $ tapa version

Prints the installed TAPA version string to standard output and exits.  This
is useful for scripting and for verifying that the correct version is active
after installation or an upgrade.
