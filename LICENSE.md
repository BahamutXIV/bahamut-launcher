# License

## MIT License

Copyright (c) 2026 Aeshur

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

The MIT grant covers the project-authored launcher code, documentation, and
assets. Third-party material retains the licenses and notices below.

## FINAL FANTASY XIV artwork

The following files under `src-tauri/ui/assets/` remain the property of
Square Enix and are outside the MIT grant:

- `background-day.png` and `background-night.png`
- `news-placeholder.jpg`
- `theme-day.png` and `theme-night.png`

&copy; SQUARE ENIX

The [FINAL FANTASY XIV Materials Usage Policy](https://support.na.square-enix.com/rule.php?id=5382&tag=authc)
governs use of these materials.

## Third-party notices

### Bundled fonts and native libraries

The self-hosted fonts retain their SIL Open Font License 1.1 notices:

- [Inter](src-tauri/ui/assets/licenses/Inter-OFL.txt)
- [Cinzel](src-tauri/ui/assets/licenses/Cinzel-OFL.txt)
- [JetBrains Mono](src-tauri/ui/assets/licenses/JetBrainsMono-OFL.txt)

The Cinzel and JetBrains Mono notices are reproduced from the Google Fonts
[Cinzel](https://github.com/google/fonts/blob/main/ofl/cinzel/OFL.txt) and
[JetBrains Mono](https://github.com/google/fonts/blob/main/ofl/jetbrainsmono/OFL.txt)
distributions. Portable archives carry these font notices in `licenses/`.

The x86 client module bundles
[MinHook](client/vendor/minhook/LICENSE.txt),
[Dear ImGui](client/vendor/imgui/LICENSE.txt),
[Lua 5.1.5](client/vendor/lua/COPYRIGHT), and
[Miniz 3.1.2](client/vendor/miniz/LICENSE). Their notices ship in every
archive's `licenses/` directory.

### llvm-mingw runtime

The Windows archive's client module is built with MSVC. The Linux and macOS
archives carry the same module cross-compiled with the pinned
[llvm-mingw](https://github.com/mstorsjo/llvm-mingw/releases) toolchain, which
statically links its runtime into
`bahamut-loader.exe`, `bahamut.dll`, `plugins/screenshot.dll`, and
`plugins/discord-rpc.dll`:

- The mingw-w64 CRT and headers. The mingw-w64 runtime notices required for
  binary distribution are reproduced unmodified in
  [MinGW-w64-runtime-COPYING.txt](src-tauri/ui/assets/licenses/MinGW-w64-runtime-COPYING.txt),
  copied from the release's
  `i686-w64-mingw32/share/mingw32/COPYING.MinGW-w64-runtime.txt`. The Linux
  and macOS archives carry it as `licenses/MinGW-w64-runtime-COPYING.txt`.
- LLVM libc++, libc++abi, libunwind, and compiler-rt. The release's
  `LICENSE.TXT` records their Apache License v2.0 with LLVM Exceptions.

### SeventhUmbral provenance

Early launcher development cross-checked FFXIV 1.x patch handling against the
SeventhUmbral launcher, distributed under the 2-clause BSD license reproduced
below. No SeventhUmbral source is copied into this repository; the notice is
retained as attribution for that behavioral reference.

### SeventhUmbral (2-clause BSD)

Copyright (c) 2013-2014, Jean-Philip Desjardins
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

 * Redistributions of source code must retain the above copyright notice,
   this list of conditions and the following disclaimer.
 * Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND ANY
EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE FOR ANY
DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH
DAMAGE.

### Lucide 0.548.0 (ISC and MIT)

ISC License

Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2023 as part
of Feather (MIT). All other copyright (c) for Lucide are held by Lucide
Contributors 2025.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY
AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM
LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR
OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR
PERFORMANCE OF THIS SOFTWARE.

The MIT License (MIT) (for portions derived from Feather)

Copyright (c) 2013-2023 Cole Bemis

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
