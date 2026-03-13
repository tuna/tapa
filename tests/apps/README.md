<!--
Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
-->

## General

This directory contains multiple small example TAPA designs.

For large and complex designs, refer to the `tests/regression` directory.

To run the examples, build TAPA from source and follow the instruction below.
```bash
cd tapa/tests/apps/vadd
tapa g++ vadd.cpp vadd-host.cpp -o vadd
./vadd
```

The steps of building TAPA can be found at `https://tapa.readthedocs.io/en/main/dev/build.html`
