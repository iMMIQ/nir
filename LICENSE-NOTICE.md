# NIR license notice

Copyright (C) 2026 NIR contributors

NIR's original source code and accompanying original documentation are free software:
you can redistribute them and/or modify them under the terms of the GNU Lesser
General Public License as published by the Free Software Foundation, either
version 3 of the License, or (at your option) any later version.

NIR is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY;
without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR
PURPOSE. See the GNU Lesser General Public License for more details.

SPDX-License-Identifier: LGPL-3.0-or-later

The full LGPL v3 text is in [LICENSE](LICENSE); the incorporated GPL v3 text is in
[COPYING](COPYING). License texts originate from https://www.gnu.org/licenses/.

## Scope and third-party materials

- Original NIR engine, CLI, build scripts, tests, and documentation: LGPL-3.0-or-later.
- Original example graphics and generated audio: CC0-1.0, as stated in the example credits.
- Noto font subset: SIL Open Font License 1.1; see `examples/rain-letters/credits/FONT-LICENSE.txt`.
- `vendor/wgpu` and other dependencies retain their upstream licenses and notices.
  See `vendor/README.md` and the SDK's generated `THIRD-PARTY.txt`.

This license does not change the license of independently authored game content.
When distributing a player or SDK, retain the applicable license and attribution
notices and comply with the LGPL terms for the engine and any combined work.

## Corresponding source and rebuilding

The public source repository is https://github.com/iMMIQ/nir.
Each binary release is accompanied by a source archive with the same version,
including the modified wgpu source, dependency locks, and build scripts.
The pinned dependency sources can be obtained with `cargo fetch --locked`.
Follow `README.md` to rebuild the libraries, WASM player, and CLI, including with
modified library code. Run `resolve` with the rebuilt SDK before rebuilding game
content so the release refers to the new SDK. No signing key is required.
