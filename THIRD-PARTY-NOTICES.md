# Third-Party Notices

This project includes software written by others. Each component is listed
with where it lives, where it came from, and the licence it is used under.

## bose-bmap

| | |
| --- | --- |
| Location | `vendor/bose-bmap/` |
| Source | https://github.com/aaronsb/bosectl (`rust/`), commit `70570b29c1d3c9ba775b08a29eaea72385de27b5` |
| Used for | The Bose BMAP protocol: packet codec, operators, error codes, per-model configuration and reply validation |
| Changes | Build configuration only, so that the crate compiles on Windows. No protocol logic was changed. See `vendor/bose-bmap/VENDORED.md`. |
| Licence | MIT |

The licence text below is the upstream `LICENSE` file, which is also kept
unmodified at `vendor/bose-bmap/LICENSE`.

```text
MIT License

Copyright (c) 2026 Aaron Bockelie

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
```

## Voice feedback: speech runtime and model

None of the following is copied into this repository. The runtime library is
fetched when the application is built; the model is fetched by
`tools/install-voice-model.ps1` when the user runs it. An installer built with
`npm run tauri:build` contains the two runtime DLLs and does not contain the
model.

| Component | Used as | Source | Licence |
| --- | --- | --- | --- |
| sherpa-onnx 1.13.8 | Rust crates `sherpa-onnx`, `sherpa-onnx-sys`; native library `sherpa-onnx-c-api.dll` | https://github.com/k2-fsa/sherpa-onnx | Apache-2.0 |
| ONNX Runtime 1.28.2 | `onnxruntime.dll`, shipped inside the sherpa-onnx library archive | https://github.com/microsoft/onnxruntime | MIT |
| eSpeak NG | Text-to-phoneme code compiled into `sherpa-onnx-c-api.dll`, and the `espeak-ng-data` folder inside the model package | https://github.com/espeak-ng/espeak-ng | GPL-3.0-or-later |
| Kokoro v0.19 (`kokoro-int8-en-v0_19`) | The voice model and its 11 voices | https://huggingface.co/hexgrad/Kokoro-82M, packaged by the sherpa-onnx project | Apache-2.0 |

The model package carries its own `LICENSE` file (Apache-2.0), which the
install script unpacks alongside the model. The sherpa-onnx crates carry
theirs in the Cargo registry.

The Android package contains the same components built for Android:
`libsherpa-onnx-c-api.so` and `libonnxruntime.so` from the sherpa-onnx 1.13.8
Android archive, and — unless built with `-NoVoice` — the Kokoro model and the
eSpeak NG data files, packed as `voice-model.zip`. What is said below about
eSpeak NG applies to an APK as it does to an installer.

### A consequence to know about before distributing a build

`sherpa-onnx-c-api.dll` includes eSpeak NG, which is licensed under the GNU
General Public License, version 3 or later. Building and using the application
on your own computer is unaffected. Giving a build that contains that DLL to
someone else is distribution of GPL-licensed code, and has to be done on the
GPL's terms — which reach the work it is combined with. This project has not
chosen a licence of its own, and that would need settling first. This is a
statement of the situation, not legal advice.

## Other dependencies

Rust crates and npm packages pulled in through `Cargo.toml` and `package.json`
are not copied into this repository. Their licences are distributed with them
by their package managers.
