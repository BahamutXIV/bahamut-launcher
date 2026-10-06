# Handshake

[Back to the documentation index](README.md)

Use this reference when working on client launch or the Bahamut server
interface. The launcher starts FFXIV 1.23b with an encrypted argument, a
validated session ID, two PE patches, and a platform-specific launch path.
Bahamut controls lobby admission and downstream server behavior.

## Current contract status

| Surface | Status | Canonical implementation |
|---|---|---|
| Launch argument | Bahamut contract implemented; complete retail grammar unresolved. | `src/launcher/launch_args.rs` |
| Session id | Bahamut contract implemented; retail shape unresolved. | `src/auth/types.rs`, `src/login/dev_token.rs` |
| Server UTC patch | Implemented; command-line and memory values are distinct. | `src/launcher/pe_patch.rs` |
| Lobby host patch | Implemented; retail slot width and consumer unresolved. | `src/launcher/pe_patch.rs` |
| Platform launch | Implemented. | `src/platform/windows.rs`, `src/platform/linux.rs`, `src/platform/macos.rs` |

The public [client structure manifest](https://github.com/XIVLegacy/xivl-client-structs/blob/main/manifests/server_utc_paths.json)
also distinguishes the command-line server UTC value from the patched client
number.

## Launch argument

The client receives one encoded argument after the executable path:

```text
sqex0002<base64-url-safe encrypted payload>!////
```

Its plaintext is:

```text
 T =<tick> /LANG =en-us /REGION =2 /SERVER_UTC =1356916742 /SESSION_ID =<sessionId>\0
```

The encryption key is the lowercase, eight-character hexadecimal form of:

```text
tick & ~0xFFFF
```

Encrypt only complete 8-byte Blowfish blocks; leave trailing bytes unchanged.
Base64-encode the result, then replace `+` with `-` and `/` with `_`.

## Session token

Login responses must contain a session ID of exactly 56 lowercase hexadecimal
characters. Manually supplied developer tokens accept 56 ASCII hexadecimal
characters in either case. These launcher rules have not been verified as
retail token requirements.

## PE patches

| Patch | RVA | Size | Contract |
|---|---:|---:|---|
| Server UTC immediate load | `0x9A15E3` | 5 bytes | Write `B8 12 E8 E0 50`, replacing the `_time64(NULL)` call with client number `1356916754`. |
| Lobby host slot | `0xB90110` | `0x14` bytes | Write a NUL-terminated hostname. Reject values longer than 20 bytes including the NUL. |

Keep the launch argument's `SERVER_UTC =1356916742` separate from the PE patch
value `1356916754`. The mismatch is unresolved; do not normalize them to one
value.

## Related retail references

The public [opcode catalog](https://github.com/XIVLegacy/xivl-opcodes/blob/main/opcodes.json)
records client routing for lobby opcodes `0x0001`, `0x01F5`, and `0x01F6`.
`0x0010` remains likely and undecoded.

The public [lobby decrypt study](https://github.com/XIVLegacy/xivl-captures/blob/main/studies/lobby-handshake-triage/derived/decrypt-recipe.md)
confirms MD5 over the full 44-byte key input and Blowfish ECB for two captured
lobby connections. Those captures do not establish server acceptance,
rejection, skew in client numbers, or all lobby behavior.

## Windows launch

With the client module disabled, the launcher creates `ffxivgame.exe`
suspended, resolves its image base, writes and reads back both patches, then
resumes the primary thread.

With the module enabled, the packaged x86 helper manages the suspended launch:

1. Create the ready event and metrics mapping.
2. Apply and verify the patches, then inject `bahamut.dll`.
3. Resolve the owner-relative initializer and invoke it outside the loader lock.

The runtime repeats the client identity check. If Windows compatibility
handling routes the D3D9 entry point through the system `apphelp.dll`, the
runtime accepts that boundary. It opens metrics and loads the exact packaged
Screenshot and DiscordRPC DLLs for enabled plugins. Before installing the
render boundary and signaling readiness, it validates the private ABI and fixed
plugin identities.

A missing or incompatible enabled plugin DLL terminates the suspended launch.
A later plugin callback fault disables only that plugin; the game and other
runtime services keep running.

For borderless mode, the launcher passes an optional OS monitor identity in
`BAHAMUT_RUNTIME_BORDERLESS_MONITOR`. The runtime resolves it against active
monitors when placing or resetting the window. Without an identity, it uses
the display nearest the game window. If an explicit identity is disconnected,
it tries the primary display, then an available display. This transport leaves
the private runtime ABI and restoration of the original window style and
rectangle unchanged.

The launcher reads the final numeric exit code from the retained process handle
only after the handle signals, then closes it once. A query failure is reported
separately and does not invalidate a confirmed exit. A nonzero exit code is
diagnostic information; it does not automatically classify the exit as a crash.

If the helper fails after process creation, it requests termination and waits
up to two seconds. If it cannot confirm termination, it reports
`termination_unconfirmed` with the process ID and retains live remote state.
It does not claim that the target stopped.

## Helper wait mode and image patches

Callers that cannot hold a Win32 handle, such as a native Wine backend, use
these helper options:

| Option | Contract |
|---|---|
| `--wait-for-client` | Keep the client handle rather than transferring it to the parent. `SUCCESS` includes `pid=` without `process_handle=`. Flush each protocol line as it is written, wait for the client, print one `EXIT` line, and exit with the client's exit code. |
| `--timeout-ms <1-60000>` | Ready deadline in decimal milliseconds; defaults to `1500`. Outside the test stub, another value requires `--wait-for-client`. In wait mode, this can raise but never lower the 5000 ms deadline for remote `LoadLibraryW`, API-version, and initializer threads. |
| `--patch <rva>:<hex>` | Repeat up to eight times. Supply an RVA of one to eight hex digits followed by one to 64 bytes. Each range must fit inside one section of the identity-checked client file; otherwise the helper reports `PatchApplyFailed no_target_created`. Patches are written after the two contract patches and before runtime loading, only at image base `0x00400000`, with the same read-back and instruction-cache flush. |

`--patch` changes process memory only. The on-disk `ffxivgame.exe` retains its
recorded identity for helper and runtime checks.

| `EXIT` line | Meaning | Helper exit code |
|---|---|---|
| `EXIT pid=<pid> code=<code>` | The client exited with `<code>`. | `<code>` |
| `EXIT pid=<pid> code_unavailable` | The client exited, but its exit code could not be read. | `1` |
| `EXIT pid=<pid> unconfirmed` | The wait failed; the client may still be running. | `1` |

## Wine launch

Linux uses the managed Wine engine unless `BAHAMUT_WINE` overrides it. It
falls back to system Wine if the engine cannot be installed or the host is
not x86_64; see [Linux Wine engine](configuration.md#linux-wine-engine).
macOS uses the managed Sikarugir Wine engine.

Both backends follow these steps for launches without the helper:

1. Resolve or initialize a Wine prefix.
2. Derive the launch tick from `CLOCK_BOOTTIME` on Linux or `CLOCK_MONOTONIC`
   on macOS. The tick must agree with the client's `GetTickCount`. Wine 11.0
   derives that value from `CLOCK_BOOTTIME` on Linux in
   [`monotonic_counter`](https://github.com/wine-mirror/wine/blob/wine-11.0/dlls/ntdll/unix/sync.c).
3. Copy `ffxivgame.exe` to `ffxivgame.patched.exe`.
4. Apply the two contract patches and three Wine stability patches to the
   copy. Never modify the original executable.
5. Start Wine detached, with output redirected to a log for that launch.
6. If the launcher is still running when the client exits, append its exit
   status to that log. Report a nonzero exit and the log's last lines in the
   launcher log.

Linux also provisions DXVK for D3D9, with WineD3D as a fallback. Its
`enable_verbose_wine_debug` preference expands the `WINEDEBUG` filter.
macOS ignores that preference and uses its default filter.

### Wine extension launch

When a launch carries extension artifacts and both `bahamut-loader.exe` and
`bahamut.dll` are present in the install root, the Wine backends use the helper
instead of the patched working copy. The files sit beside the launcher in a
portable tree or [Linux package](configuration.md#linux-package-layout), and
under `Contents/Resources` in the [macOS app](configuration.md#macos-app-layout).
The helper runs under the managed engine, or the Wine selected by
`BAHAMUT_WINE` or the Linux fallback.

- Before planning the helper launch, the launcher checks the
  [client identity](extensions.md#client-compatibility). A mismatch stops it.
- The helper receives the original `ffxivgame.exe` through `--client`, the
  contract patches through `--server-utc` and `--lobby-host`, and the three
  Wine stability patches through `--patch`. It also receives
  `--timeout-ms 10000` and `--wait-for-client`. This path never writes
  `ffxivgame.patched.exe`.
- Each path-valued `BAHAMUT_RUNTIME_*` variable is converted to a drive-letter
  path using the prefix's `dosdevices` links. Lists are converted element by
  element. An unreachable path stops launch. Other variables match the Windows
  plan, except Borderless is planned as Windowed without a monitor identity.
- The launcher reads `SUCCESS`, `ERROR`, and `EXIT` lines from `helper.log`.
  The first `SUCCESS` or `ERROR` is the readiness result, with a 90-second
  deadline. Adjacent `wine.log` contains Wine's own output. Both live under
  the launcher data directory's `logs/`: `~/.bahamut-launcher/logs/` on Linux
  and macOS, or `$BAHAMUT_LAUNCHER_HOME/logs/` when that variable is an absolute
  path.
- The launcher tracks the `wine` process hosting the helper as its game
  process. That process exits with the client.

Linux uses the same DXVK overrides as the patched-copy launch. If either
helper file is missing, the launcher warns and uses the patched-copy path.
See [Platform support](extensions.md#platform-support) for player-facing
status and limits.

## Authentication and server profile

[Authentication](auth.md) defines registration, login, session validation, and
logout. Pasting a developer token bypasses HTTP auth for local launch tests;
this is not retail behavior.

The active profile and lobby host come from `config/bahamut.ini`. Only the
lobby host reaches the game through the PE patch. Auth ports, lobby ports,
and downstream service routing are not written into the client. See
[Configuration](configuration.md) for the profile schema.
