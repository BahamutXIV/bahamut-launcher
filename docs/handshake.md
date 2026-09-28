# Handshake

[Back to the documentation index](README.md)

The launcher starts the FFXIV 1.23b client with an encrypted argument, a
validated session ID, two PE patches, and a platform-specific launch path.
Bahamut owns lobby admission and downstream server behavior.

## Current contract status

| Surface | Status | Canonical implementation |
|---|---|---|
| Launch argument | Implemented Bahamut contract. Complete retail grammar unresolved. | `src/launcher/launch_args.rs` |
| Session id | Implemented Bahamut contract. Retail shape unresolved. | `src/auth/types.rs`, `src/login/dev_token.rs` |
| Server UTC patch | Implemented. Command line and memory values are distinct. | `src/launcher/pe_patch.rs` |
| Lobby host patch | Implemented. Retail slot width and consumer unresolved. | `src/launcher/pe_patch.rs` |
| Platform launch | Implemented | `src/platform/windows.rs`, `src/platform/linux.rs`, `src/platform/macos.rs` |

The public [client structure manifest](https://github.com/XIVLegacy/xivl-client-structs/blob/main/manifests/server_utc_paths.json)
also records the distinction between the command-line server UTC value and the
patched client number.

## Launch argument

The client receives one encoded argument after the executable path:

```text
sqex0002<base64-url-safe encrypted payload>!////
```

The plaintext is:

```text
 T =<tick> /LANG =en-us /REGION =2 /SERVER_UTC =1356916742 /SESSION_ID =<sessionId>\0
```

The encryption key is the lowercase eight-character hexadecimal form of:

```text
tick & ~0xFFFF
```

Only complete 8-byte Blowfish blocks are encrypted. Trailing bytes remain
unchanged. The result is base64 encoded, then `+` becomes `-` and `/` becomes
`_`.

## Session token

The launcher accepts login responses only when the session ID contains exactly
56 lowercase hexadecimal characters. Manually supplied developer tokens accept
56 ASCII hexadecimal characters in either case. These are launcher rules, not
verified retail token requirements.

## PE patches

| Patch | RVA | Size | Contract |
|---|---:|---:|---|
| Server UTC immediate load | `0x9A15E3` | 5 bytes | Write `B8 12 E8 E0 50`, replacing the `_time64(NULL)` call with client number `1356916754`. |
| Lobby host slot | `0xB90110` | `0x14` bytes | Write a NUL-terminated hostname and reject values that do not fit in 20 bytes including the NUL. |

The launch argument's `SERVER_UTC =1356916742` and the PE patch value
`1356916754` are separate facts. Their mismatch is unresolved and must not be
normalized into one value.

## Related retail references

The public [opcode catalog](https://github.com/XIVLegacy/xivl-opcodes/blob/main/opcodes.json)
records client routing for lobby opcodes `0x0001`, `0x01F5`, and `0x01F6`.
`0x0010` remains likely and undecoded. The public
[lobby decrypt study](https://github.com/XIVLegacy/xivl-captures/blob/main/studies/lobby-handshake-triage/derived/decrypt-recipe.md)
confirms MD5 over the complete 44-byte key input and Blowfish ECB for its two
captured lobby connections. Those captures do not establish server
acceptance, rejection, skew in client numbers, or every lobby behavior.

## Windows launch

With the client module disabled, the launcher creates `ffxivgame.exe`
suspended, resolves its image base, writes and reads back both patches, and
resumes the primary thread.

With the client module enabled, the packaged x86 helper owns the suspended
launch. It creates the ready event and metrics mapping, applies and verifies the
patches, injects `bahamut.dll`, resolves the owner-relative initializer, and
invokes it outside the loader lock. The runtime repeats the client identity
check, accepts the system `apphelp.dll` boundary when Windows compatibility
handling routes the D3D9 entry point through it, opens metrics, and loads the
exact packaged Screenshot and DiscordRPC DLLs when each plugin is enabled. It
validates the private ABI and fixed plugin identities before installing the
render boundary and signaling readiness. A missing or incompatible enabled
plugin DLL terminates the suspended launch. A later plugin callback fault
disables only that plugin and leaves the game and other runtime services running.

For Windows borderless mode, the launcher passes the optional OS monitor
identity through `BAHAMUT_RUNTIME_BORDERLESS_MONITOR`. The runtime resolves it
against active monitors at placement/reset time. An absent identity uses the
display nearest the game window. A disconnected explicit identity selects the primary
display, then an available display. This transport does not change the private
runtime ABI or original window style/rectangle restoration.

The retained process handle is queried for the final numeric exit code only
after it signals, then closed once. A query failure is reported separately and
does not undo confirmed exit. A nonzero code is diagnostic information, not an
automatic crash classification.

After process creation, a helper failure requests termination and waits up to
two seconds. If termination cannot be confirmed, the helper reports
`termination_unconfirmed` with the process id, retains live remote state, and
does not claim the target stopped.

## Helper wait mode and image patches

A caller that cannot hold a Win32 handle, such as a Wine backend running as a
native process, uses these helper options:

| Option | Contract |
|---|---|
| `--wait-for-client` | The helper keeps the client handle instead of transferring it to its parent. `SUCCESS` carries `pid=` without `process_handle=`, each protocol line is flushed as it is written, and the helper waits for the client, prints one `EXIT` line, and exits with the client's exit code. |
| `--timeout-ms <1-60000>` | Decimal milliseconds for the ready deadline, default `1500`. Outside the test stub, any other value requires `--wait-for-client`. In wait mode the value can also raise, never lower, the 5000 ms deadline for the remote `LoadLibraryW`, API-version, and initializer threads. |
| `--patch <rva>:<hex>` | Repeatable up to eight times: one to eight hex digits of RVA, then one to 64 bytes. Each range must lie inside one section of the identity-checked client file, or the helper reports `PatchApplyFailed no_target_created`. The helper writes the patches after the two contract patches, before loading the runtime, at image base `0x00400000` only, with the same read-back and instruction-cache flush. |

`--patch` changes only process memory, so `ffxivgame.exe` on disk keeps its
recorded identity for the helper and runtime checks.

| `EXIT` line | Meaning | Helper exit code |
|---|---|---|
| `EXIT pid=<pid> code=<code>` | The client exited with `<code>`. | `<code>` |
| `EXIT pid=<pid> code_unavailable` | The client exited. Its exit code could not be read. | `1` |
| `EXIT pid=<pid> unconfirmed` | The wait failed. The client may still be running. | `1` |

## Wine launch

Linux uses system Wine. macOS uses the managed Sikarugir Wine engine. Both
backends:

- Resolve or initialize a Wine prefix.
- Derive the launch tick from `CLOCK_BOOTTIME` on Linux and `CLOCK_MONOTONIC`
  on macOS. The tick must agree with the client's `GetTickCount`, which Wine
  11.0 derives from `CLOCK_BOOTTIME` on Linux in
  [`monotonic_counter`](https://github.com/wine-mirror/wine/blob/wine-11.0/dlls/ntdll/unix/sync.c).
- Copy `ffxivgame.exe` to `ffxivgame.patched.exe`.
- Apply the two contract patches and three Wine stability patches to the copy,
  never the original executable.
- Start Wine detached with output redirected to a log for that launch.
- While the launcher is still running, append the client's exit status to that
  log when the client exits and report a nonzero exit, with the log's last
  lines, in the launcher log.

Linux also provisions DXVK for D3D9 with a WineD3D fallback. Its
`enable_verbose_wine_debug` preference expands the `WINEDEBUG` filter. The
macOS backend ignores that preference and uses its default filter.

### Wine extension launch

When `bahamut-loader.exe` and `bahamut.dll` are present in the
[install root](configuration.md#macos-app-layout) (beside the binary in a
portable tree, under `Contents/Resources` in the macOS app) and the launch
carries extension artifacts, the Wine backends run the helper, under the
managed engine on macOS and under system Wine on Linux, instead of the launch
from the working copy above:

- The launcher checks the [client identity](extensions.md#client-compatibility)
  before planning the helper. A mismatch stops the launch.
- The helper receives the original `ffxivgame.exe` as `--client`, the two
  contract patches through `--server-utc` and `--lobby-host`, the three Wine
  stability patches through `--patch`, `--timeout-ms 10000`, and
  `--wait-for-client`. No `ffxivgame.patched.exe` is written.
- Every path-valued `BAHAMUT_RUNTIME_*` variable is rewritten to the drive
  letter form of the prefix's `dosdevices` links, list values per element, and
  a path that no drive reaches stops the launch. The other variables match the
  Windows plan, except that a Borderless request is planned as Windowed with
  no monitor identity.
- The launcher reads the helper's `SUCCESS`, `ERROR`, and `EXIT` lines from
  `helper.log` and treats the first `SUCCESS` or `ERROR` line as the readiness
  result, with a 90 second deadline. `wine.log` beside it keeps Wine's own
  output, and both sit in the launcher data directory's `logs/` folder
  (`~/.bahamut-launcher/logs/` on macOS and Linux, or
  `$BAHAMUT_LAUNCHER_HOME/logs/` when that variable holds an absolute path).
- The launcher's game process is the `wine` process hosting the helper, which
  exits with the client.

Linux applies the same DXVK overrides as its launch from the working copy. When
either file is missing, the launcher logs a warning and uses that launch path.
[Platform support](extensions.md#platform-support) records the status and
limits for players on this path.

## Authentication and server profile

Registration, login, session validation, and logout are defined by the
[Authentication](auth.md). A developer token pasted by the operator bypasses
HTTP auth for local launch testing and is not retail behavior.

The active profile and its lobby host come from `config/bahamut.ini`. The game
receives only the lobby host through the PE patch. Auth ports, lobby ports, and
downstream service routing are not written into the client. See
[Configuration](configuration.md) for the canonical profile schema.
