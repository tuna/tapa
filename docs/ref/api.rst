TAPA Library (libtapa)
----------------------

Task Library
::::::::::::

.. doxygenstruct:: tapa::task
  :members:

.. doxygenstruct:: tapa::seq
  :members:

Stream Library
::::::::::::::

* A *blocking operation* blocks if the stream is not available (empty or full)
  until the stream becomes available.
* A *non-blocking operation* always returns immediately.

* A *destructive operation* changes the state of the stream.
* A *non-destructive operation* does not change the state of the stream.

.. _api istream:

.. doxygenclass:: tapa::istream
  :members:

.. _api istreams:

.. doxygenclass:: tapa::istreams
  :members:

.. _api ostream:

.. doxygenclass:: tapa::ostream
  :members:

.. _api ostreams:

.. doxygenclass:: tapa::ostreams
  :members:

.. _api stream:

.. doxygenclass:: tapa::stream
  :members:

.. _api streams:

.. doxygenclass:: tapa::streams
  :members:

MMAP Library
::::::::::::

.. _api async_mmap:

.. doxygenclass:: tapa::async_mmap
  :members:

.. _api mmap:

.. doxygenclass:: tapa::mmap
  :members:

.. _api mmaps:

.. doxygenclass:: tapa::mmaps
  :members:

.. _api hmap:

``tapa::hmap<T, chan_count, chan_size>``
''''''''''''''''''''''''''''''''''''''''

``tapa::hmap`` is a channelised view of a :ref:`tapa::mmap <api mmap>` that
logically partitions the underlying memory into *chan_count* channels, each
containing *chan_size* elements of type ``T``.  It is used in task signatures
and in host code wherever a fixed-channel memory layout is required.

The total number of elements accessed through an ``hmap`` must equal
``chan_count * chan_size``; a runtime assertion enforces this invariant.

.. code-block:: cpp

   // Host side – pass an ordinary mmap; the kernel receives it as hmap
   tapa::invoke(MyKernel, bitstream, mem);

   // Task signature – declare the parameter as hmap
   void MyKernel(tapa::hmap<float, 4, 1024> mem);

.. _api directional mmaps:

Directional MMAP Wrappers
'''''''''''''''''''''''''

``tapa::read_only_mmaps<T, N>``, ``tapa::write_only_mmaps<T, N>``, and
``tapa::read_write_mmaps<T, N>`` are type-safe directional wrappers for arrays
of mmaps.  They are used in :cpp:func:`tapa::task().invoke()` to pass multiple
mmaps with explicit direction hints, which lets the compiler apply more
aggressive optimisations and produces clearer interface documentation.

Each wrapper type inherits from ``tapa::mmaps<T, N>`` and adds a direction tag.
The direction affects how the Vitis HLS interface is synthesised (``m_axi``
ports get ``bundle`` and ``offset`` attributes inferred from the tag).

.. code-block:: cpp

   // Host side
   int64_t t = tapa::invoke(VecAdd, bitstream,
                            tapa::read_only_mmaps<float, 4>(a),
                            tapa::read_only_mmaps<float, 4>(b),
                            tapa::write_only_mmaps<float, 4>(c),
                            n);

   // Task signature – kernel parameters use plain tapa::mmaps<T, N>
   void VecAdd(tapa::mmaps<float, 4> a,
               tapa::mmaps<float, 4> b,
               tapa::mmaps<float, 4> c,
               uint64_t n);

A ``placeholder_mmaps<T, N>`` variant also exists for cases where no direction
hint is desired.

Utility Library
:::::::::::::::

.. _api widthof:

.. doxygenfunction:: tapa::widthof()
.. doxygenfunction:: tapa::widthof(T)

TAPA Compiler (tapa)
--------------------

.. _api tapa:

.. click:: tapa.__main__:entry_point
  :prog: tapa
  :nested: full
