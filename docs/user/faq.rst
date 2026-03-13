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
The ``tapa compile`` command is used to compile TAPA source files into a
Vitis object file. Most of the arguments and options are the same as the
original ``tapac`` command. You may refer to the ``--help`` option for more
information.

Is there a command-line utility for quick help?
-----------------------------------------------

Yes, we provide a convenient command-line utility for quick reference. You
can access command information about our commands by typing:

.. code-block:: bash

   tapa --help
   tapa compile --help
