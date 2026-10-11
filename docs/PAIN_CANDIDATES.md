# Pain candidate bridge (isolated implementation, 2026-10-11)

Base: `fee2efb4a11db06d35b1f1ac1deb5e863bb29ecb`.
The bridge preserves HuFu's original candidate window and publishes a separate
read-only candidate UIElement for Pain. Pain and TigerClaw sources are unchanged.

## Protocol

New TSF clients add this optional member to `key` and `state` requests:

```json
{"candidate_ui":{"protocol":1,"context":"process-thread-focus-nonce","instance":"last-server-instance","version":12}}
```

The optional `state.candidate_ui` response contains `protocol`, `context`,
`instance`, and monotonically increasing `version`. A changed snapshot includes
`menu: {texts, pages, selected, page}`. Page starts and selection are absolute
indices; `page` is zero based. An unchanged response omits `menu`. `menu:null`
ends the current selectable menu. A missing extension means an old server and
never creates a UIElement. Version zero in a request asks for a complete menu.

Only a key claims ownership. Queries never take ownership. Legacy and HTTP keys
clear ownership; reset/focus clear it as well. A process-instance nonce, owner
context, version, local focus generation, actual TSF document/context check and
retired-instance fence reject obsolete responses. The local context changes on
key context replacement, focus loss, push/pop, deactivation and connection failure.
A request context is not a security credential; it prevents accidental cross-field
session adoption by cooperative clients.

The menu comes from **live Session.candidates**, not the commit response's
confirmation-animation `outcome.state`. Residual raw input retains its own menu;
empty raw never republishes a confirmation frame. Original candidates and exported
text use the same dynamic-word/history conversion. Hidden candidates adjust the
exported selected index and page starts. An empty transformed page, hidden selected
candidate, invalid index, or conservative size limit disables the export. The
engine's existing page-navigation cursor can stay on the previous page; the bridge
matches the original window's first-item highlight on the new page. It does not
change navigation or commitment behavior.

Candidate extensions are bounded to 1 MiB (conservative text bound plus final JSON
size check). Existing length-prefix/partial-read transport and the independent
32 MiB skin-response limit are unchanged. All candidate text is fetched in one
response; getters never request text over IPC. Existing state polling applies
asynchronous reranking as before; the export itself neither decodes nor learns.
Page number, page count and export version now participate in poll change detection.

## COM and original windows

`candidate_ui.rs` implements only `ITfCandidateListUIElement`, with a retained
DocumentMgr agile reference and local snapshot. The windows-rs macro exposes
IAgileObject; snapshots therefore use Arc/RwLock, and document references resolve
through COM marshaling. IsShown enumerates the original TSF thread’s windows. It supports Pain's zero-capacity page-count query
followed by a second `GetPageIndex` call. The first notification has all six flags;
later flags describe actual differences. Ended objects retain their last snapshot
while the host holds a COM reference. `SetPageIndex` is read-only (`E_NOTIMPL`).
There is no behavior interface or host mouse selection.

Begin/Update/End execute without Shared mutexes or RefCell borrows. Reentrant
updates coalesce; focus changes during Begin cause the returned UIElement ID to
be ended instead of installed into a newer generation.

FALSE from Begin means the host may render, per
[Microsoft's contract](https://learn.microsoft.com/en-us/windows/win32/api/msctf/nf-msctf-itfuielementmgr-beginuielement).
This implementation deliberately retains the original D2D/server window alongside
Pain. Show does not change the original window or gate updates. IsShown checks
original-window visibility locally. This is **not complete UILess support**.

## GNU builds and isolated checks

Run Cargo from the respective workspace directory. The default linker names are
`i686-w64-mingw32-gcc` and `x86_64-w64-mingw32-gcc`; override using standard
`CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER` /
`CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER` environment variables.
Use a matching GNU CRT/toolchain throughout. The author's custom LLVM entry shim
is now opt-in with `llvm-mingw-entry`; ordinary GNU builds do not use it.

```sh
cd platform/windows
CARGO_INCREMENTAL=0 cargo build -p hufu-tsf --target i686-pc-windows-gnu
CARGO_INCREMENTAL=0 cargo test -p hufu-tsf --target i686-pc-windows-gnu --no-run
# Repeat for x86_64-pc-windows-gnu. Execute the resulting tests on Windows.
# Only for the isolated Pain fixture:
CARGO_INCREMENTAL=0 cargo build -p hufu-tsf --target i686-pc-windows-gnu --features candidate-probe
```

`candidate-probe` exports CreateTestCandidate/UpdateTestCandidate/ProbeWindow.
These exports are absent from normal builds. Stage the probe DLL as
`evidence/<arch>/hufu_tsf.dll`, and the normal DLL under `evidence/<arch>/normal/`.
Run `tools/test-pain.ps1 -Arch x86` / `-Arch x64`, then `tools/test-load.ps1` for
both architectures. The Pain script compiles its existing presenter and a copied
fixture, verifies original source hashes, and sets the upstream process-local
`HUFU_TSF_SMOKE=1` foreground exception to make window visibility deterministic.
This exception is not an assertion of real foreground/focus acceptance.

The x86/x64 COM test executables can be run with filter `candidate_ui`.
The server test executable must be waited on even though it has the GUI subsystem:
`& .\ServerTests.exe --nocapture | Out-Host; exit $LASTEXITCODE`.
The test-only document check activates TSF in its own isolated process; it never
registers or activates the HuFu input processor. The DLL load test does not call
ITfTextInputProcessor.Activate.

From `engine`, run `cargo test -p hufu-engine --lib`; build server/tests for
`x86_64-pc-windows-gnu` with the GNU linker override and run on Windows. Native
Linux ARM64 server compilation is blocked by upstream rerank AVX2 guards; no
unrelated rerank port was attempted. Evidence here uses debug builds, no models.

See [verification results](PAIN_CANDIDATES_VERIFICATION.md) for results and
acceptance limits. Raw build/test and local installation records are retained in
the ignored `evidence/` directory.

`tools/build-isolated.sh` stages both architectures; `tools/verify-isolated.ps1`
runs all staged checks on Windows and fails on any nonzero exit status.
