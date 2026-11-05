<!--
Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
-->

This design was collected from [Sextans](https://github.com/linghaosong/Sextans).

The `//tests/regression:sextans-xo` Bazel target builds
`src/sextans-no-const-para.cpp`, which avoids aggressively passing constants
through handshake interfaces. The constants variant (`src/sextans.cpp`) sits
next to it for reference.
