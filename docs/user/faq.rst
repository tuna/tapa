Frequently Asked Questions
==========================

Does TAPA support Alveo U55C?
-----------------------------

Yes, TAPA supports the Alveo U55C FPGA accelerator card.
For examples, refer to our `test suite`_. You can find a specific example
in the `Sextans benchmark`_.

.. _test suite: https://github.com/tuna/tapa/tree/main/tests
.. _Sextans benchmark: https://github.com/tuna/tapa/tree/main/tests/regression/spmm/sextans-u55c-3x3floorplan

Where is ``tapac``?
-------------------

The ``tapac`` command has been replaced by ``tapa compile``.
The ``tapa compile`` command compiles TAPA source files and supports multiple
output targets, selected with the ``--target`` flag. Most of the arguments and
options are the same as the original ``tapac`` command. You may refer to the
``--help`` option for more information.

What does ``tapa compile`` produce?
------------------------------------

The output format depends on the selected target:

- ``xilinx-vitis`` (the default): produces a ``.xo`` Xilinx object file.
  This file is consumed by the Vitis ``v++`` linker to generate an ``.xclbin``
  hardware binary for on-board execution.

- ``xilinx-hls``: produces a ``.zip`` RTL archive. This archive contains the
  synthesized RTL and can be used for RTL inspection, standalone simulation, or
  integration into custom downstream toolchains. It does not require Vivado to
  generate.

Select the target with the ``--target`` option:

.. code-block:: bash

   tapa compile --target xilinx-hls --top VecAdd -f vadd.cpp -o vadd_rtl.zip

Is there a command-line utility for quick help?
-----------------------------------------------

Yes, we provide a convenient command-line utility for quick reference. You
can access command information about our commands by typing:

.. code-block:: bash

   tapa --help
   tapa compile --help
