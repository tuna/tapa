Advanced Synthesis Options
==========================

The ``tapa synth`` sub-command accepts several flags that go beyond basic RTL
generation.  This page describes the options that control post-synthesis
reports, FIFO pipeline behaviour, and graph generation for downstream
floorplanning tools.

Post-Synthesis Utilisation Report (``--enable-synth-util``)
-----------------------------------------------------------

.. code-block:: console

   $ tapa synth --enable-synth-util ...

When this flag is set, TAPA runs an additional RTL synthesis pass after HLS
to collect resource utilisation numbers for each task.  The results are
written to the work directory as:

- ``report.json`` — machine-readable JSON format.
- ``report.yaml`` — equivalent YAML format for human inspection.

Both files contain per-task LUT, FF, BRAM, and DSP counts.  The report can be
used to identify resource hot-spots before running full implementation.

The flag can also be negated with ``--disable-synth-util`` (the default).

Non-Pipelined FIFOs (``--nonpipeline-fifos``)
----------------------------------------------

.. code-block:: console

   $ tapa synth --nonpipeline-fifos fifos.json ...

By default, TAPA inserts pipeline registers into stream FIFOs to improve
timing closure.  In some floorplanning flows it is desirable to suppress
pipelining for specific FIFOs so that they can be grouped with their producer
or consumer logic inside a single floorplan region.

``--nonpipeline-fifos`` accepts a path to a JSON file listing the FIFO names
that should *not* be pipelined:

.. code-block:: json

   ["fifo_a", "fifo_b"]

After synthesis, TAPA writes a ``grouping_constraints.json`` file to the work
directory.  This file encodes the grouping hints and can be passed to
RapidStream or other floorplanning tools to place the non-pipelined FIFOs and
their adjacent logic in the same region.

AutoBridge Graph Generation (``--gen-ab-graph``)
-------------------------------------------------

.. code-block:: console

   $ tapa synth --gen-ab-graph \
                --floorplan-config floorplan.json \
                ...

When ``--gen-ab-graph`` is set, TAPA generates an ``ab_graph.json`` file in
the work directory.  This graph captures the task topology, port connectivity,
and estimated resource usage in the format expected by
AutoBridge/RapidStream for partition-based floorplanning.

``--floorplan-config`` is **required** when ``--gen-ab-graph`` is used.  It
specifies the path to a JSON file that describes the target device floorplan
regions.

GraphIR Generation (``--gen-graphir``)
---------------------------------------

.. code-block:: console

   $ tapa synth --gen-graphir \
                --device-config device.json \
                --floorplan-path floorplan.json \
                ...

``--gen-graphir`` produces a GraphIR representation of the floorplanned TAPA
program, written to ``graphir.json`` in the work directory, suitable for
consumption by RapidStream.  Both ``--device-config`` and ``--floorplan-path``
are required when this flag is active:

``--device-config PATH``
  Path to a JSON file describing the physical device (e.g. SLR layout, DSP
  column positions) used by the GraphIR conversion.

``--floorplan-path PATH``
  Path to the floorplan assignment file.  The floorplan is applied to the
  program before GraphIR is emitted.
